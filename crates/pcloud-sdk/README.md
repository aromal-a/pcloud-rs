# pcloud-embedded-sdk

Internal compatibility API for embedding the entire daemon runtime. The public,
SemVer-stable drive client now lives in `crates/pcloud-sdk-public` under the
package name `pcloud-sdk`; its registry release is still pending.

## What this crate does

- Embeds the daemon runtime in-process; it does not require a separately
  launched `pcloudd`.
- Exposes the focused `EmbeddedDaemon::remote()` contract with SDK-owned types
  for stat/list, range-read, durable upload/download, copy, move, delete,
  mkdir, and folder sharing.
- Keeps raw IPC and broad compatibility helpers available for first-party
  integrations, but those are not the preferred filesystem API.

## Public API entry points

- `EmbeddedDaemon::builder(...).build()`
- `EmbeddedDaemon::login(...)` and the TFA/token helpers
- `EmbeddedDaemon::remote()` returning `RemoteDrive`
- `UploadSession` for observable chunked uploads

## Usage

```rust,no_run
use std::path::PathBuf;
use pcloud_embedded_sdk::EmbeddedDaemon;

let mut daemon = EmbeddedDaemon::builder(PathBuf::from("./pcloud-state"))
    .build()?;
daemon.login("account@example.test", "password-from-a-secret-store")?;

let root = daemon.remote().list("/")?;
for entry in root.entries {
    println!("{}", entry.name);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Features

None.

The supported TLS backend is rustls with webpki roots. No native-TLS feature
is advertised.

## Security posture

- All secret-bearing fields use `pcloud-secret` wrappers.
- Auth token persistence is opt-in and inherits the daemon vault policy.
- Streaming transfers use durable resume state and verified checksums.

## Stability and publication

This compatibility crate is workspace-internal, version `0.1.0`, and
`publish = false`. It deliberately preserves the broad historical embedded API
while applications migrate to the focused `pcloud-sdk` crate.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).
