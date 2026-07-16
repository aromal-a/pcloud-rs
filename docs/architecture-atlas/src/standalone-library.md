# Using pcloud-rs standalone or as a library

## Standalone daemon + CLI

Build only the ordinary user-facing binaries:

```bash
cargo build --release --locked -p pcloud-daemon -p pcloud-cli
```

Start the daemon:

```bash
target/release/pcloudd serve
```

Then authenticate and use `pcloudc`. The daemon must remain running for CLI,
SDK, web, mount, and experimental WebDAV IPC operations.

The exact profile, runtime path, log level, and platform service lifecycle are
configuration/package concerns. Consult the main handbook and operations
runbook before deploying beyond a development account.

## Stable Rust library

Package: `pcloud-sdk`, source under `crates/pcloud-sdk-public`.

```rust
use pcloud_sdk::{Client, SharePermissions};

fn run() -> Result<(), pcloud_sdk::Error> {
    let client = Client::new("/run/user/1000/pcloud/ipc.sock");
    let drive = client.remote();

    let docs = drive.list("/Docs")?;
    for entry in docs.entries {
        println!("{}: {:?}", entry.name, entry.id);
    }

    drive.mkdir("/Docs/Shared")?;
    drive.share_folder(
        "/Docs/Shared",
        "person@example.com",
        SharePermissions {
            create: true,
            modify: true,
            delete: false,
            manage: false,
        },
        None,
    )?;
    Ok(())
}
```

The daemon owns login and lifecycle. The SDK contract is intentionally
filesystem-focused and uses SDK-owned types. Its source version is 1.0.0, but
registry publication and install-from-registry verification remain external
release gates.

## Embedded first-party library

Package: `pcloud-embedded-sdk`, source under `crates/pcloud-sdk`.

Use this only when your application intentionally embeds the daemon runtime
and accepts an evolving, unpublished API. It provides broader compatibility
helpers but also couples the process to daemon bootstrap, state, and runtime
semantics. It is not a substitute name for the stable public SDK.

## Direct protocol library

Core implementers may use `pcloud-proto` to add or test typed pCloud API
families. This bypasses daemon-owned policy, persistence, RemoteFs, and local
IPC. An end-user application should prefer `pcloud-sdk` unless it is
deliberately implementing its own full runtime.

## Filesystem mount

Mounting is a daemon operation backed by `pcloud-fs` and native platform
adapters. The portable CLI/API remains usable on platforms without a kernel
mount. Never infer API support from mount availability or vice versa.

## Web and WebDAV

`pcloud-web` is an evolving operator UI over daemon IPC. `pcloud-webdav` is
experimental and unshipped; its implemented subset has an IPC adapter to
RemoteFs, but there is no RFC 4918 compliance-class claim. Do not expose
either on an untrusted network without an explicit authentication, TLS, host,
and reverse-proxy design.
