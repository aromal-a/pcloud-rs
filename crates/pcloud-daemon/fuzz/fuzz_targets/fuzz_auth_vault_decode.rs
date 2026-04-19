#![no_main]
//! Audit-06 wave-2 (bd-pcloud-rs-ncx.70) — fuzz the auth-vault token
//! parser. The daemon's `auth_vault::load_token` shim delegates to
//! `vault::file::load_token` which:
//!
//! 1. stats the file (owner/mode checks — skipped here because we write
//!    the file ourselves so it always has the right mode),
//! 2. reads the raw bytes into an owned buffer that it zeroizes on
//!    failure,
//! 3. trims ASCII whitespace without allocating a second copy,
//! 4. decodes the trimmed window as UTF-8.
//!
//! Any panic inside steps 2–4 would crash the daemon during token
//! load on a corrupted or truncated vault file, so the fuzzer feeds
//! arbitrary bytes through the full pipeline by writing them to a
//! temp file and calling the public shim.

use libfuzzer_sys::fuzz_target;
use std::io::Write;

use pcloud_daemon::auth_vault::load_token;

fuzz_target!(|data: &[u8]| {
    // Write the fuzzer-provided bytes to a temporary file at mode 0600
    // inside a 0700 parent directory so `validate_vault_file` accepts
    // the metadata and the parser actually runs against `data`. This
    // isolates the byte-level parser from the permissions check.
    let dir = match tempfile::Builder::new()
        .prefix("pcloud-fuzz-vault-")
        .tempdir()
    {
        Ok(d) => d,
        Err(_) => return,
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Best-effort: force the parent dir to 0700 so the vault
        // validator accepts it. If this fails the test simply returns
        // without exercising the parser.
        let _ = std::fs::set_permissions(
            dir.path(),
            std::fs::Permissions::from_mode(0o700),
        );
    }

    let vault_path = dir.path().join("vault");
    let mut file = match std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&vault_path)
    {
        Ok(f) => f,
        Err(_) => return,
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }

    if file.write_all(data).is_err() {
        return;
    }
    drop(file);

    // `load_token` must never panic — it must return `Ok(None)`,
    // `Ok(Some(_))`, or an `Err(AuthVaultError::*)`.
    let _ = load_token(&vault_path);
});
