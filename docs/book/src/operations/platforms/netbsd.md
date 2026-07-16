# NetBSD

NetBSD 10.1 is an explicitly supported Tier 1 qualification target. The native
VM job runs locked workspace checks/tests and a strict journaled FUSE round
trip after verifying `/dev/puffs` or `/dev/fuse`.

```sh
sudo /usr/sbin/pkg_add rust pkgconf
cargo build --release --locked -p pcloud-daemon -p pcloud-cli
target/release/pcloudc start
```

IPC uses AF_UNIX with `getpeereid(3)`, secrets use the owner-only file vault,
mount discovery uses `getmntinfo(3)`, and the mount adapter uses the native
FUSE-compatible device through `fuser`.

The in-tree rc.d asset is `packaging/netbsd/pcloudd`. A pkgsrc package is a
downstream deliverable; do not document `pkgin install pcloud-rs` as available
until it is published and has passed install/upgrade/service testing.

Support claims are limited to the OS version and architecture shown by a
successful retained native job for the release commit.
