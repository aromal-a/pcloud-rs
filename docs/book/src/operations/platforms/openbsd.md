# OpenBSD

OpenBSD 7.9 is an explicitly supported Tier 1 qualification target. The native
VM job installs Rust and `pkgconf`, runs locked workspace check/clippy/tests,
verifies `/dev/fuse0`, and executes the strict journaled FUSE round trip.

```sh
doas pkg_add rust pkgconf
cargo build --release --locked -p pcloud-daemon -p pcloud-cli
target/release/pcloudc start
```

IPC uses AF_UNIX with `getpeereid(3)`, secrets use the owner-only file vault,
mount discovery uses `getmntinfo(3)`, and the filesystem adapter uses the
native FUSE device through `fuser`.

The in-tree rc.d asset is `packaging/openbsd/pcloudd`. A ports package is a
downstream deliverable and must not be advertised until it is published and
has passed install/upgrade/service testing.

Support claims are limited to the OS version and architecture shown by a
successful retained native job for the release commit.
