# pcloud-ipc

Local IPC codec and transport for pcloud-rs daemon and CLI: owner-only UNIX
sockets with explicit peer checks.

## What this crate does

- Defines the request/response wire types shared by `pcloudd` and `pcloudc`.
- Provides a framed codec with malformed/slow-client isolation.
- Verifies peer UID and socket-path permissions before any secret-bearing
  exchange.

## Public API entry points

- `Request`, `Response`, `Method`.
- `codec::encode`, `codec::decode`.
- `transport::listen`, `transport::connect`.

## Usage

```rust,no_run
use pcloud_ipc::{Method, Request};

let req = Request::new(Method::Health);
let _bytes = pcloud_ipc::codec::encode(&req);
```

## Features

None.

## Security posture

- Socket mode is `0600`; parent directory is `0700`.
- Peer UID is compared to the daemon's UID on every connection.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).
