# pcloud-auth

Authentication primitives and session state types for the pcloud-rs Rust rewrite.

## What this crate does

- Defines typed credentials, session tokens, and TFA challenge state.
- Provides serializable auth state consumed by `pcloud-daemon` and the
  internal `pcloud-embedded-sdk`.
- Owns no network or I/O logic: transports live in `pcloud-proto` and the daemon's `auth_backend`.

## Public API entry points

- `Credentials`, `SessionToken`, `TfaChallenge`, and related state enums.
- Re-exports of `pcloud-secret::SecretString` for password-bearing fields.

## Usage

```rust,no_run
use pcloud_auth::Credentials;
use pcloud_secret::SecretString;

let _creds = Credentials::new(
    "alice@example.com".into(),
    SecretString::from("correct horse battery staple"),
);
```

## Features

None. Single default build.

## Security posture

- Passwords and tokens are held in `SecretString` so they zeroize on drop.
- No logging or `Display` impls leak secret material.

## License

Dual-licensed under `MIT OR Apache-2.0`. See workspace root.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).
