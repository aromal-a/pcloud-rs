# pcloud-sdk

The stable, blocking Rust SDK for pCloud drive operations. It connects to the
owner-authenticated local `pcloudd` endpoint and exposes only SDK-owned public
types for metadata, listing, bounded reads, resumable upload/download, copy,
move, delete, mkdir, and folder sharing.

```rust,no_run
use pcloud_sdk::Client;

let client = Client::new("/run/user/1000/pcloud/pcloud-rs/pcloud.sock");
let root = client.remote().list("/")?;
for entry in root.entries {
    println!("{}", entry.name);
}
# Ok::<(), pcloud_sdk::Error>(())
```

The daemon owns authentication, transport policy, retry, durability, and the
canonical ID-first `RemoteFs` service. On Windows the socket-path argument is
ignored and the owner-specific named pipe is derived from the current SID.

The broad in-process compatibility API is intentionally separate and remains
unpublished as `pcloud-embedded-sdk`.

The 1.0 package manifest is release-ready in source, but the crate has not yet
been published. The staged registry order is `pcloud-model`, `pcloud-ipc`, then
`pcloud-sdk`.
