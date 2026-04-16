# pcloud-sdk

Embeddable high-level SDK surface for pcloud-rs: auth, upload/download helpers,
and typed wrappers over the daemon runtime.

## What this crate does

- Exposes a stable, ergonomic API for embedders who want pcloud-rs functionality
  inside their own process without launching `pcloudd`.
- Bundles `upload_data`, `upload_data_as`, `upload_file`, `upload_file_as`, and
  the auth/TFA flows.

## Public API entry points

- `Sdk::new`, `Sdk::login`, `Sdk::upload_file`, `Sdk::upload_data`.
- Builders for session and transfer configuration.

## Usage

```rust,no_run
use pcloud_sdk::Sdk;

let sdk = Sdk::builder().build()?;
# let _ = sdk;
# Ok::<(), pcloud_sdk::Error>(())
```

## Features

None.

## Security posture

- All secret-bearing fields use `pcloud-secret` wrappers.
- Auth token persistence is opt-in and inherits the daemon vault policy.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).
