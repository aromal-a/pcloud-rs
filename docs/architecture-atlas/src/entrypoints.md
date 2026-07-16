# Entrypoints and public surfaces

## Choose the interface

| Need | Preferred entrypoint | Process model | Stability |
|---|---|---|---|
| Human administration and file operations | `pcloudc` | short-lived CLI → daemon IPC | evolving product CLI |
| Third-party Rust application | `pcloud_sdk::Client` | application → daemon IPC | focused SemVer 1.x source contract; registry release pending |
| First-party in-process integration | `pcloud-embedded-sdk::EmbeddedDaemon` | daemon runtime linked into process | unpublished compatibility API |
| Mounted filesystem | `pcloudc mount` / daemon mount request | OS adapter inside daemon | Linux locally verified; other native gates pending |
| Operator browser UI | `pcloud-web` | HTTP UI → daemon IPC | MVP/evolving |
| WebDAV client | `pcloud-webdav` | HTTP adapter → daemon IPC | experimental, unshipped, no RFC compliance claim |
| Direct pCloud protocol work | `pcloud-proto` | library → pCloud TLS | internal; use only when building core backends |

## Binary entrypoints

### `pcloudd`

Source: `crates/pcloud-daemon/src/main.rs`.

The ordinary public runtime is the cross-platform `pcloudd serve` process. On
Unix it binds owner-only local IPC; on Windows it serves an owner-specific
named pipe. The separate `pcloud-daemon-win` binary is an experimental SCM
host and is not the public per-user Windows path.

### `pcloudc`

Source: `crates/pcloud-cli/src/main.rs`.

The CLI performs global-option extraction, command parsing, request creation,
native IPC send, response formatting, and exit-code mapping. It should not
open the pCloud remote protocol or SQLite store directly.

Important remote-drive commands:

```bash
pcloudc remote ls /
pcloudc remote cat /Docs/readme.txt
pcloudc remote get /Docs/report.pdf ./report.pdf
pcloudc remote put ./report.pdf /Docs/report.pdf
pcloudc remote cp /Docs/report.pdf /Archive/report.pdf
pcloudc remote mv /Archive/report.pdf /Archive/final.pdf
pcloudc remote mkdir /Archive/2026
pcloudc remote rm /Archive/final.pdf
```

### `pcloud-web`

Source: `crates/pcloud-web/src/main.rs`.

This is an optional operator-facing facade. Its non-loopback bind mode is for
controlled deployments and requires explicit host handling. It is not the
daemon and does not own remote credentials.

## Public SDK entrypoint

```rust
use pcloud_sdk::Client;

let client = Client::new("/run/user/1000/pcloud/ipc.sock");
let listing = client.remote().list("/")?;
for entry in listing.entries {
    println!("{} {:?}", entry.name, entry.id);
}
# Ok::<(), pcloud_sdk::Error>(())
```

`Client::new` performs no I/O. Each call opens an IPC request to an already
running, authenticated daemon. On Windows the path parameter is retained for
configuration symmetry while native IPC derives the owner-specific named
pipe.

The public surface owns its result types rather than re-exporting daemon IPC
enums. Start at `Client::remote()` and the `RemoteDrive` methods in
`crates/pcloud-sdk-public/src/lib.rs`.

## Embedded entrypoint

`pcloud-embedded-sdk` links the broad daemon runtime into the caller and
exposes historical auth, crypto, public-link, account, share, transfer, raw
dispatch, and upload-session helpers. It is useful for first-party
composition and tests, but it is intentionally `publish = false` and is not
the external SemVer contract.

## Core extension entrypoints

| Change | Primary files |
|---|---|
| New CLI command | CLI `app.rs`/`commands.rs`, IPC methods, daemon runtime/dispatch |
| New daemon-mediated operation | `pcloud-ipc/src/methods.rs`, daemon runtime, owning backend |
| New pCloud API command | `pcloud-proto/src/methods/` plus family API module |
| New drive-like operation | `pcloud-backends/src/remote_fs.rs`, IPC, SDK/CLI consumers |
| New sync behavior | `pcloud-engine`, daemon sync loop/runtime, store repositories |
| New mount operation | `pcloud-fs` portable traits first, then native adapter |
| New durable state | `pcloud-store` migration + repository + recovery tests |
| New plugin | plugin API/host and a separate bounded plugin crate |

The generated crate pages list every current Rust source file and public item.
