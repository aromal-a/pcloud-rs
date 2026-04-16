# pcloud-secret

Zeroize-on-drop secret wrappers for pcloud-rs: `SecretString` and `SecretBytes`,
both with redacted `Debug` implementations and constant-time comparison.

## What this crate does

- Wraps sensitive byte/UTF-8 material so it is zeroized on drop.
- Redacts `Debug` / `Display` output so secrets never leak into logs.
- Provides constant-time equality via `subtle`.

## Public API entry points

- `SecretString`, `SecretBytes`.
- `ct_eq`.

## Usage

```rust
use pcloud_secret::SecretString;

let s = SecretString::from("hunter2");
assert_eq!(format!("{s:?}"), "SecretString(***)");
```

## Features

None.

## Security posture

- Zeroization is unconditional on drop.
- No `Serialize` impl for secrets — callers must opt into explicit handling.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).
