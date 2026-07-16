# FreeBSD

FreeBSD is an explicitly supported Tier 1 target for the portable API/CLI and
the native FUSE mounted drive. The native VM job is the release gate; its
definition is not evidence that a release commit passed.

## Native gate

The CI job installs Rust, `fusefs-libs3`, and `pkgconf`, loads `fusefs`, enables
user mounts, runs locked workspace check/clippy/tests, verifies `/dev/fuse`,
and executes the strict small-file journaled FUSE round trip.

## Build and run

```sh
sudo pkg install rust git fusefs-libs3 ca_root_nss pkgconf
sudo kldload fusefs
sudo sysctl vfs.usermount=1
cargo build --release --locked -p pcloud-daemon -p pcloud-cli
target/release/pcloudc start
```

IPC uses AF_UNIX plus `getpeereid(3)`. Automatic secret storage uses the
owner-only file vault. The mount table is read with `getmntinfo(3)` and the
filesystem adapter uses `fuser` against the native FUSE device.

An rc.d asset lives at `packaging/freebsd/pcloudd.rc`, but a downstream port
or package is not part of this repository. Do not publish `pkg install
pcloud-rs` instructions until that package actually exists and passes an
install/upgrade/service test.

## Qualification limits

- Release notes must name the FreeBSD version and architecture that passed.
- rc.d install/start/stop and package upgrade testing remain separate from the
  runtime/mount VM gate.
- FreeBSD derivatives inherit no automatic support claim; qualify their base
  version, package environment, and FUSE policy explicitly.
