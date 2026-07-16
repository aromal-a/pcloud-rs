//! Durable Windows file replacement with a protected owner-scoped DACL.

#![cfg(windows)]

use std::fs;
use std::io::{self, Write};
use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    SetFileSecurityW,
};
use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows::core::PCWSTR;

pub(super) fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        apply_owner_only_acl(parent)?;
    }
    let tmp = path.with_extension("dpapi.tmp");
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
    }
    apply_owner_only_acl(&tmp)?;
    let tmp_wide = path_to_wide(&tmp);
    let path_wide = path_to_wide(path);
    // SAFETY: both paths are NUL-terminated. REPLACE_EXISTING gives repeated
    // token stores atomic replacement semantics on Windows; WRITE_THROUGH
    // does not return until the move reaches durable storage.
    unsafe {
        MoveFileExW(
            PCWSTR(tmp_wide.as_ptr()),
            PCWSTR(path_wide.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|error| io::Error::other(error.to_string()))?;
    // The temporary file already has this protected DACL. Reapplying after
    // the rename is defense in depth against filesystem-specific ACL merge
    // behavior.
    apply_owner_only_acl(path)?;
    Ok(())
}

/// Apply a protected DACL granting FullControl only to the object owner,
/// LocalSystem, and local Administrators. `OW` is Windows' Owner Rights SID;
/// because object ownership is separately enforced by the CLI doctor this is
/// equivalent to embedding the current user's machine-specific SID.
fn apply_owner_only_acl(path: &Path) -> io::Result<()> {
    const SDDL: &str = "D:P(A;;FA;;;OW)(A;;FA;;;SY)(A;;FA;;;BA)";

    let sddl_wide: Vec<u16> = SDDL.encode_utf16().chain(std::iter::once(0)).collect();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `sddl_wide` is NUL-terminated and `descriptor` is writable.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl_wide.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .map_err(|error| io::Error::other(error.to_string()))?;
    let _descriptor = SecurityDescriptorGuard(descriptor);

    let path_wide = path_to_wide(path);
    let security_information = windows::Win32::Security::OBJECT_SECURITY_INFORMATION(
        DACL_SECURITY_INFORMATION.0 | PROTECTED_DACL_SECURITY_INFORMATION.0,
    );
    // SAFETY: the path and self-relative security descriptor remain live for
    // the call. The descriptor contains a present, valid DACL.
    let applied =
        unsafe { SetFileSecurityW(PCWSTR(path_wide.as_ptr()), security_information, descriptor) };
    if !applied.as_bool() {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct SecurityDescriptorGuard(PSECURITY_DESCRIPTOR);

impl Drop for SecurityDescriptorGuard {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            // SAFETY: conversion from SDDL allocated this descriptor through
            // LocalAlloc and transferred ownership to the caller.
            let _ = unsafe { LocalFree(HLOCAL(self.0.0.cast())) };
        }
    }
}

fn path_to_wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
