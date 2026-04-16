# pcloud-mockserver

Local in-process HTTP mock of the pCloud REST API for integration tests. No
network access required; no production secrets involved.

## What this crate does

- Stands up an in-process HTTP server that returns canned responses keyed on
  method and query parameters.
- Lets `pcloud-proto` and `pcloud-daemon` exercise full request/response flows
  without hitting pcloud.com.

## Public API entry points

- `MockServer::start`.
- Builder helpers for common endpoints (login, userinfo, listfolder, ...).

## Usage

```rust,no_run
use pcloud_mockserver::MockServer;

let server = MockServer::start();
let base = server.base_url();
# let _ = base;
```

## Features

None.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).
