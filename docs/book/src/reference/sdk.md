# Rust SDK

`pcloud-sdk` 1.0 is the focused, blocking Rust API for remote-drive
operations. It is a client of the long-running `pcloudd` process; it does not
embed the sync engine, own credentials, or expose daemon/backend request types.

> **Publication status (2026-07-16).** The 1.0 source contract and package
> manifests exist in this repository, but no crates.io release exists yet.
> Publication must occur in dependency order: `pcloud-model`, `pcloud-ipc`,
> then `pcloud-sdk`. Until that release is completed, use a pinned Git revision
> or workspace path and do not claim registry availability.

Maintainers use the approval-gated `Publish Rust SDK` workflow. Its default
mode performs package-content and test/doc checks only; `execute=true` requires
the protected `crates-io` environment, publishes in dependency order, waits for
registry indexing, and verifies a fresh registry-only project.

## Contract

The public contract is deliberately small:

- `Client` configures the owner-authenticated local IPC endpoint.
- `RemoteDrive` exposes `stat`, `list`, `read_range`, `upload`, `download`,
  `copy`, `move_path`, `delete`, `mkdir`, and `share_folder`.
- Metadata, receipts, share options, and errors are SDK-owned types. No
  `pcloud-ipc`, `pcloud-model`, daemon, or backend type appears in a public
  signature.
- APIs are blocking and safe Rust. The crate forbids unsafe code and denies
  missing public documentation.
- The 1.x SemVer promise covers this focused surface. Adding enum variants or
  struct fields is permitted because public data types are non-exhaustive.

Authentication, retry policy, checksums, durable resume state, remote ID/path
resolution, and secret ownership remain daemon responsibilities. Every SDK
operation reaches the canonical ID-first `RemoteFs` service through local IPC.

## Dependency and startup

For a workspace checkout:

```toml
[dependencies]
pcloud-sdk = { path = "crates/pcloud-sdk-public" }
```

Start and authenticate the daemon first:

```console
$ pcloudd serve
$ pcloudc login --user account@example.test --password-stdin
```

Pass the daemon's `pcloud.sock` path to `Client::new`. With `PCLOUD_ROOT` set,
the endpoint is `$PCLOUD_ROOT/runtime/pcloud.sock`. Under normal Unix defaults
it is `<runtime-dir>/pcloud.sock`; Windows derives the owner-specific named pipe
from the current SID and retains the path argument only for API symmetry.

## Example

```rust,no_run
use std::path::Path;
use pcloud_sdk::{Client, ShareOptions, SharePermissions};

let client = Client::new("/run/user/1000/pcloud/pcloud-rs/pcloud.sock");
let drive = client.remote();

for entry in drive.list("/Docs")?.entries {
    println!("{}", entry.name);
}

let receipt = drive.upload(
    Path::new("./report.pdf"),
    "/Docs/report.pdf",
)?;
println!("uploaded {} bytes as {:?}", receipt.bytes, receipt.file_id);

drive.share_folder(
    "/Docs",
    &ShareOptions::new("recipient@example.test")
        .permissions(SharePermissions::READ_ONLY),
)?;
# Ok::<(), pcloud_sdk::Error>(())
```

All remote paths must be absolute. `read_range` rejects a zero length and the
daemon caps a single range response at 8 MiB; use consecutive reads or
`download` for larger content.

## Error handling

`pcloud_sdk::Error` separates transport, invalid request, authentication,
conflict, temporary unavailability, policy refusal, backend failure, and
malformed-response cases. It deliberately stores daemon details as redacted
messages instead of exposing wire enums. Treat `Unavailable` as retryable;
do not retry `InvalidRequest`, `Unauthorized`, `Conflict`, or `Policy` without
changing state or user input.

## Embedded compatibility API

The historical broad in-process surface is a different package:
`pcloud-embedded-sdk` under `crates/pcloud-sdk`. It links the daemon runtime and
retains auth, crypto, account, public-link, raw-dispatch, and upload-session
helpers for first-party compatibility. It is version 0.1, evolving, and
`publish = false`; third-party applications should not build new integrations
against it.
