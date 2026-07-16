# illumos, OmniOS, and Oracle Solaris

OmniOS/illumos and Oracle Solaris 11.4 are Tier 1 targets for the portable
library, canonical `RemoteFs`, SDK, CLI, daemon, transfer/share operations, and
local IPC. Kernel mounting is explicitly unsupported.

## Native gates

- OmniOS CI uses the r151058 image, enables the official `extra.omnios`
  publisher, installs its current Rust and build-tool packages, enforces the
  workspace MSRV, checks the locked workspace, tests IPC/backends/CLI, and
  runs CLI help.
- Solaris CI uses the 11.4 GCC image and the official rustup installer, then
  enforces the same MSRV and portable API/CLI checks.

These workflow definitions become support evidence only after successful
release-commit runs.

## Runtime behavior

IPC uses AF_UNIX plus `getpeerucred(3)` and checks the peer effective UID. The
credential object is freed after extracting UID and PID. Automatic secret
storage uses the owner-only file vault.

Remote operations remain available without a mount:

```sh
pcloudc remote ls /
pcloudc remote get /remote/file ./file
pcloudc remote put ./file /remote/file
pcloudc remote cp /remote/file /remote/copy
```

Mount attempts return `MountError::UnsupportedPlatform`. `fuser 0.16` does not
implement the Solaris-family mount/unmount ABI, so exposing a half-working
FUSE path would be less safe than a deterministic error. The experimental
WebDAV crate has a canonical daemon IPC adapter, but its listener is not
bootstrapped or shipped and is not a mounted-namespace fallback.

## Service and package candidates

`packaging/solarish` contains a disabled-by-default SMF child service. It runs
as the dedicated `pcloudd` identity, uses the standard manifest/method paths,
sends SIGTERM on stop, and SIGHUP on refresh. Each native job validates the
manifest with `svccfg`, builds release binaries, and retains a deterministic
tar candidate with internal and adjacent SHA-256 manifests.

No IPS repository is published. Source builds and workflow definitions do not
constitute a supported installer: native install, enable, start/stop, upgrade,
and uninstall evidence is required before advertising turnkey installation.
