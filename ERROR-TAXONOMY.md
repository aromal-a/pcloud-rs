# Error Taxonomy

Status: additive. Public API preserved. Not a breaking change.

The pcloud-rs Rust workspace historically grew one `*Error` enum per crate or
subsystem (`AuthHelperError`, `DownloadHelperError`, `CryptoHelperError`,
`ShareKindError`, `ValueKvError`, `SettingKvError`, `PublicLinkBackendError`,
`TransportError`, etc.). Each one is fine in isolation but the enterprise API
boundary (CLI exit codes, IPC status packets, SDK return types, structured
logs) wants a single typed entry point.

This document describes the unified `pcloud_error::Error` taxonomy that every
helper error in the workspace funnels into, while the per-helper enums are
retained as the public API for back-compat.

## Design rules

1. **Additive only.** No existing `Error` enum is removed or renamed.
2. **Boundary layer, not a replacement.** Internal code keeps using the
   concrete, per-helper error. Only the outermost SDK/CLI/IPC boundary
   converts into the unified type.
3. **Cause chain preserved.** Every conversion goes through
   `pcloud_error::IntoUnified`, which boxes the original error as the
   unified error's `source()`.
4. **Stable numeric codes.** Scripts may rely on them; a snapshot test
   (`crates/pcloud-error/tests/code_stability.rs`) pins them.
5. **No secrets in messages.** The unified type carries a `String` built from
   the inner error's `Display`. Helper errors are already audited not to
   include secrets.

## Categories and stable codes

| Category        | Code | Slug            | Used for                                                |
|-----------------|------|-----------------|---------------------------------------------------------|
| `Auth`          | 1000 | `auth`          | login, TFA, session, vault load/save                    |
| `Permission`    | 1100 | `permission`    | authorization / access-denied                           |
| `Api`           | 1200 | `api`           | pCloud API result errors, quota, upload/download, KV    |
| `Transport`     | 1300 | `transport`     | TCP, TLS, framing                                       |
| `Ipc`           | 1400 | `ipc`           | local daemon unix-socket IPC                            |
| `Protocol`      | 1500 | `protocol`      | request/response schema mismatch                        |
| `Crypto`        | 1600 | `crypto`        | E2E crypto: locked, not set up, wrong password          |
| `Storage`       | 1700 | `storage`       | SQLite, vault, migrations, settings KV                  |
| `Config`        | 1800 | `config`        | invalid config, secure-default violations               |
| `LocalIo`       | 1900 | `local_io`      | local filesystem I/O                                    |
| `NotFound`      | 2000 | `not_found`     | logical entity lookup miss                              |
| `InvalidInput`  | 2100 | `invalid_input` | caller-supplied argument rejected                       |
| `Busy`          | 2200 | `busy`          | resource locked / already-in-progress                   |
| `Plugin`        | 2300 | `plugin`        | plugin registration or dispatch                         |
| `Internal`      | 9000 | `internal`      | bug / invariant violation                               |

Numeric codes are exposed via `Error::code() -> u32` and the slug via
`Error::category() -> &'static str`.

## Helper error -> category mapping

Helper errors owned by `pcloud-sdk` are wired by
`crates/pcloud-sdk/src/lib.rs` at the bottom of the file.

| Helper error               | Source crate       | Category      | Code |
|----------------------------|--------------------|---------------|------|
| `EmbeddedDaemonError`      | `pcloud-sdk`       | `Internal`    | 9000 |
| `UploadHelperError`        | `pcloud-sdk`       | `Api`         | 1200 |
| `BackupHelperError`        | `pcloud-sdk`       | `Api`         | 1200 |
| `ValueKvError`             | `pcloud-sdk`       | `Storage`     | 1700 |
| `SettingKvError`           | `pcloud-sdk`       | `Storage`     | 1700 |
| `AccountUtilityError`      | `pcloud-sdk`       | `Api`         | 1200 |
| `CryptoHelperError`        | `pcloud-sdk`       | `Crypto`      | 1600 |
| `AuthHelperError`          | `pcloud-sdk`       | `Auth`        | 1000 |
| `DownloadHelperError`      | `pcloud-sdk`       | `Api`         | 1200 |
| `ConfigError`              | `pcloud-config`    | `Config`      | 1800 |
| `std::io::Error`           | std                | `LocalIo`     | 1900 |

Daemon-internal errors (`TransportError`, `CryptoError`, `PublicLinkBackendError`,
`SharesBackendError`, `CryptoShareError`, `BackupBackendError`,
`AccountBackendError`, `SyncBackendError`, `AuthVaultError`,
`AuthBackendError`, `TransferBackendError`, `MountError`, `MetadataCryptoError`,
`ContentCryptoError`, `PluginError`, `ProtocolError`, `IpcTransportError`,
`StoreError`, `SettingsError`, `MigrationError`, ...) are intentionally NOT
wired in this first pass. They are internal to the daemon and do not cross
the enterprise API boundary; wiring them follows the same pattern and can be
added incrementally without breaking anything.

## How to add a new mapping

In the crate that owns `FooError`, add `pcloud-error = { path = "../pcloud-error" }`
to `Cargo.toml`, then at the bottom of the file:

```rust
impl From<FooError> for pcloud_error::Error {
    fn from(err: FooError) -> Self {
        use pcloud_error::IntoUnified;
        err.into_unified(pcloud_error::Category::Api) // pick the right category
    }
}
```

Add a test that the new mapping yields the expected `code()`.

## Test coverage

- `crates/pcloud-error/src/lib.rs` (unit tests): constructors, codes,
  `From<io::Error>`, `IntoUnified` cause chain, leaf variants drop source.
- `crates/pcloud-error/tests/code_stability.rs`: snapshot test covering
  every category's numeric code and slug. **Never update the expected
  values without a deliberate version bump.**
- `crates/pcloud-sdk/src/lib.rs` (`unified_error_tests`): round-trip tests
  for every wired SDK helper error.

## Non-goals (deliberately out of scope)

- Replacing per-crate error enums. They are the public API.
- Touching FUSE, crypto runtime, upload runtime, or notifications feature
  code. Only their `Cargo.toml` would be modified if/when they opt in.
- Serialising the unified error across IPC. That can be added later; the
  stable `code()` is enough for CLI exit codes today.
