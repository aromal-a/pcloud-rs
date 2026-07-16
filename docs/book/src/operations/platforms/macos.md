# macOS

macOS is a Tier 1 target for the library, CLI, daemon, and fuse-t mounted
drive. A release is supported only after the labelled native fuse-t and
signed-package job passes for that commit.

## Release gate

The `macos-installer` job in `release-packaging.yml`:

1. validates the tag and workspace version;
2. requires Developer ID and notarization credentials;
3. imports identities into an ephemeral keychain;
4. runs strict fuse-t readdir, create/write/fsync, and unmount tests;
5. signs the binaries and installer, notarizes, staples, checks the package
   signature, and runs Gatekeeper assessment;
6. removes the ephemeral keychain before uploading the package.

The workflow definition is not passing evidence. Retain the native job logs
and notarization result with each promoted release.

## Install

Install fuse-t first, then verify and install the release package:

```bash
pkgutil --pkg-info io.fuse-t.pkg.core
pkgutil --check-signature pcloud-rs-<version>-macos-<arch>.pkg
spctl --assess --type install --verbose=4 \
  pcloud-rs-<version>-macos-<arch>.pkg
sudo installer -pkg pcloud-rs-<version>-macos-<arch>.pkg -target /
```

The package installs `pcloudc` and `pcloudd` under `/usr/local/bin` and a
per-user LaunchAgent template under `/usr/local/share/pcloud-rs/macos`.
Materialize and load it as the user who owns the pCloud account:

```bash
/usr/local/share/pcloud-rs/macos/configure-user.sh
launchctl print "gui/$(id -u)/com.pcloud.pcloud-rs"
```

Do not run that helper with `sudo`. The daemon, Keychain item, IPC socket, and
mount must share the interactive user's identity.

## Build from source

```bash
brew install rust fuse-t pkg-config
cargo build --release --locked -p pcloud-daemon -p pcloud-cli
target/release/pcloudc start
```

For a local signed package use `packaging/macos/build-pkg.sh` with explicit
application and installer identities plus `--notarize`.

## Paths and secrets

`PcloudDirs` uses Apple's standard roots:

- config and state: `~/Library/Application Support/com.pcloud.pcloud-rs`;
- cache: `~/Library/Caches/com.pcloud.pcloud-rs`;
- runtime/IPC: `<cache>/pcloud-rs-runtime`.

`PCLOUD_ROOT` replaces these with `<root>/{config,state,runtime,cache}` for
isolated deployments. `VaultBackend::Auto` uses the login Keychain. Selecting
`PCLOUD_VAULT=file` is an explicit downgrade to the owner-only file vault and
should be documented by the operator.

## IPC

The daemon listens on an owner-only AF_UNIX socket. It authenticates clients
with `getpeereid(3)` and compares the peer effective UID with the daemon owner.
macOS does not use Linux `SO_PEERCRED` or `LOCAL_PEERCRED` here.

## Mounted drive

The adapter loads fuse-t through direct libfuse-compatible FFI. The daemon's
canonical `RemoteFs` supplies live metadata, range reads, mutations, and
durable write sessions; the mount never treats the local metadata cache as
authoritative.

```bash
mkdir -p "$HOME/pCloud"
pcloudc mount "$HOME/pCloud"
pcloudc mount status
pcloudc unmount "$HOME/pCloud"
```

Missing or incompatible fuse-t returns a surfaced unsupported error. Never
replace this backend with an unreviewed dylib path or enable `allow_other` to
bypass an ownership problem.

## Known qualification limits

- Only architectures and macOS releases exercised by the retained native job
  may be listed in release notes.
- Abnormal-stop recovery and sleep/wake behavior need periodic native soak
  testing in addition to the ordinary live gate.
- Homebrew/MacPorts manifests are separate channels and do not inherit package
  qualification automatically.
