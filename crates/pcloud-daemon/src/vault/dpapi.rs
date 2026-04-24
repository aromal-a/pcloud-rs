//! **PLATFORM: Windows 10/11.** DPAPI-backed auth token vault.
//!
//! Encrypts the auth token with `CryptProtectData` under the current
//! user's DPAPI master key and persists the ciphertext blob on disk.
//! `CryptUnprotectData` reverses the operation on load. No password,
//! entropy, or description is supplied; decryption is therefore bound
//! to the current user account.

#![cfg(windows)]

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use windows::Win32::Foundation::LocalFree;
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData,
};

use super::{AuthToken, PlatformVault, Result as VaultResult, VaultError};

/// Windows DPAPI-backed vault.
#[derive(Debug, Clone)]
pub struct DpapiVault {
    /// Path where the DPAPI-encrypted ciphertext blob lives.
    ciphertext_path: PathBuf,
}

impl DpapiVault {
    /// Construct a `DpapiVault` writing to the given ciphertext path.
    ///
    /// Typical path: `<LOCALAPPDATA>/pcloud-rs/vault/auth_token.dpapi`.
    pub fn new(ciphertext_path: impl Into<PathBuf>) -> Self {
        Self {
            ciphertext_path: ciphertext_path.into(),
        }
    }

    fn map_io(err: io::Error) -> VaultError {
        VaultError::Io(err)
    }

    fn map_win(context: &'static str) -> VaultError {
        VaultError::Io(io::Error::new(io::ErrorKind::Other, context))
    }
}

/// RAII guard that releases a DPAPI-allocated `pbData` buffer via
/// `LocalFree` when dropped. DPAPI documents that the caller owns the
/// output blob's `pbData` and must free it with `LocalFree`.
struct LocalFreeGuard {
    blob: CRYPT_INTEGER_BLOB,
}

impl LocalFreeGuard {
    fn new(blob: CRYPT_INTEGER_BLOB) -> Self {
        Self { blob }
    }

    fn as_slice(&self) -> &[u8] {
        if self.blob.pbData.is_null() || self.blob.cbData == 0 {
            return &[];
        }
        // SAFETY: DPAPI guarantees `pbData` points to `cbData` bytes of
        // valid memory owned by this guard for the duration of its
        // lifetime. The slice is immutable and does not outlive `self`.
        unsafe { std::slice::from_raw_parts(self.blob.pbData, self.blob.cbData as usize) }
    }
}

impl Drop for LocalFreeGuard {
    fn drop(&mut self) {
        if !self.blob.pbData.is_null() {
            // SAFETY: `pbData` was allocated by DPAPI (LocalAlloc-compatible)
            // and has not been freed yet. `LocalFree` is the documented
            // deallocation routine. After this call the pointer is dead;
            // we null it out to keep the guard idempotent in case `drop`
            // is somehow re-entered.
            unsafe {
                let _ = LocalFree(windows::Win32::Foundation::HLOCAL(
                    self.blob.pbData as *mut _,
                ));
            }
            self.blob.pbData = std::ptr::null_mut();
            self.blob.cbData = 0;
        }
    }
}

fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("dpapi.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

impl PlatformVault for DpapiVault {
    fn load(&self) -> VaultResult<Option<AuthToken>> {
        let bytes = match fs::read(&self.ciphertext_path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Self::map_io(e)),
        };

        let mut input = CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        // SAFETY: `input.pbData` points to `bytes` which lives through
        // this call. `output` is a valid writable blob; on success DPAPI
        // fills it with a LocalAlloc'd buffer that we immediately hand
        // to `LocalFreeGuard` so it will be freed exactly once. All
        // other pointer arguments are `None`.
        let status = unsafe {
            CryptUnprotectData(
                &mut input as *mut _,
                None,
                None,
                None,
                None,
                0,
                &mut output as *mut _,
            )
        };
        status.map_err(|_| Self::map_win("CryptUnprotectData failed"))?;

        let guard = LocalFreeGuard::new(output);
        let plain = guard.as_slice().to_vec();
        drop(guard);

        let s = String::from_utf8(plain)
            .map_err(|_| VaultError::InsecureMetadata("dpapi plaintext not utf-8"))?;
        Ok(Some(SecretString::new(s)))
    }

    fn store(&self, token: &AuthToken) -> VaultResult<()> {
        let plaintext = token.expose_secret().as_bytes();

        let mut input = CRYPT_INTEGER_BLOB {
            cbData: plaintext.len() as u32,
            pbData: plaintext.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        // SAFETY: `input.pbData` aliases the borrowed `plaintext` slice
        // which outlives this call. DPAPI only reads from `input`. On
        // success `output.pbData` is a LocalAlloc'd buffer which is
        // transferred into `LocalFreeGuard` so it will be freed exactly
        // once (after we've copied the ciphertext into a `Vec`).
        let status = unsafe {
            CryptProtectData(
                &mut input as *mut _,
                None,
                None,
                None,
                None,
                0,
                &mut output as *mut _,
            )
        };
        status.map_err(|_| Self::map_win("CryptProtectData failed"))?;

        let guard = LocalFreeGuard::new(output);
        let ciphertext = guard.as_slice().to_vec();
        drop(guard);

        atomic_write(&self.ciphertext_path, &ciphertext).map_err(Self::map_io)?;
        Ok(())
    }

    fn clear(&self) -> VaultResult<()> {
        match fs::remove_file(&self.ciphertext_path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Self::map_io(e)),
        }
    }

    fn backend_name(&self) -> &'static str {
        "dpapi"
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn store_load_clear_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth_token.dpapi");
        let vault = DpapiVault::new(&path);

        assert!(vault.load().unwrap().is_none());

        let token = SecretString::from(String::from("hunter2-token-xyz"));
        vault.store(&token).unwrap();
        assert!(path.exists());

        let loaded = vault.load().unwrap().expect("token present");
        assert_eq!(loaded.expose_secret(), "hunter2-token-xyz");

        vault.clear().unwrap();
        assert!(!path.exists());
        // Idempotent.
        vault.clear().unwrap();
        assert!(vault.load().unwrap().is_none());
    }
}
