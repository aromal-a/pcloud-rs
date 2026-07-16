# DragonFly BSD

DragonFly BSD 6.4.2 is an explicitly supported Tier 1 qualification target.
The native VM job installs Rust, `pkgconf`, and `fusefs-libs3`, runs locked
workspace check/clippy/tests, verifies the FUSE device, and executes the strict
journaled small-file mount round trip.

IPC uses AF_UNIX with `getpeereid(3)`, secrets use the owner-only file vault,
mount discovery uses `getmntinfo(3)`, and the filesystem adapter uses `fuser`.

`packaging/dragonfly/pcloudd` is the native supervised rc.d definition.
DragonFly's `daemon(8)` owns a locked supervisor PID, restarts the foreground
daemon after a bounded delay, lowers privileges, forwards SIGTERM, and sends
output to syslog. The native job builds release binaries into a deterministic
tar candidate with service assets, man pages, licenses, and SHA-256 manifests.

The retained candidate is not a downstream dport package. Installation,
upgrade, reboot, stop/drain, and uninstall must pass on the release commit
before it is presented as a supported native package.

Support claims are limited to the OS version and architecture shown by a
successful retained native job for the release commit.
