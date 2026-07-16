# NAS packages (tier 2)

This directory contains native package inputs for Synology DSM, QNAP QTS /
QuTS hero, and Asustor ADM. All three packages install `pcloudc`, `pcloudd`,
and the same tested POSIX supervisor. The supervisor keeps state under the
vendor package's persistent data directory, selects the headless file vault,
and asks the CLI for a durable drain before using bounded signal fallbacks.

These are tier-2 package candidates until each output has passed install,
upgrade, stop/start, reboot, uninstall/reinstall, and live pCloud transfer tests
on vendor hardware. Package-format validation on Linux is not a substitute for
that device matrix.

## Runtime and mount behavior

The daemon and all non-mount commands work without FUSE. Packages deliberately
do not auto-mount. A mounted drive additionally requires the NAS firmware to
provide `/dev/fuse` and grant the package identity access to it; vendor kernels
and security policies vary by model. Never run the daemon as root merely to
work around a missing FUSE permission.

Authenticate with the packaged `pcloudc` under the same package identity and
`PCLOUD_ROOT` used by the service. The persistent roots are:

- Synology: `${SYNOPKG_PKGVAR}/root` (`/var/packages/pcloud-rs/var/root`).
- QNAP: `<Install_Path>/var/root` from `/etc/config/qpkg.conf`.
- Asustor: `${APKG_PKG_DIR}/var/root`.

## Build inputs

Supply release binaries built for the NAS ABI and CPU. Static musl binaries are
preferred where the vendor toolchain accepts them; do not relabel an ordinary
desktop GNU/Linux binary without checking its minimum glibc and dynamic-library
requirements.

```sh
# Fully buildable on a Linux packaging host (DSM 7, one SPK per architecture):
packaging/nas/synology/build-spk.sh \
  --version 0.1.0 --arch x86_64 \
  --pcloudd dist/x86_64/pcloudd --pcloudc dist/x86_64/pcloudc

# Run on Ubuntu or QNAP with QDK 2.5.3+ and qbuild enabled:
packaging/nas/qnap/build-qpkg.sh \
  --version 0.1.0 --arch arm_64 \
  --pcloudd dist/arm64/pcloudd --pcloudc dist/arm64/pcloudc

# Run with ASUSTOR's ADM 5 APKG build tool. The current guide defines x86-64
# and arm64 native payloads and requires a 90x90 PNG for manual packages:
sudo APKG_TOOL=/path/to/apkg-tool.py packaging/nas/asustor/build-apk.sh \
  --version 0.1.0 --arch x86-64 \
  --pcloudd dist/x86_64/pcloudd --pcloudc dist/x86_64/pcloudc
```

The Asustor builder uses the checked-in 90x90 `icon.png` by default. Pass
`--icon` only to substitute a release-approved brand asset with the same
dimensions and format. ASUSTOR's official APKG 2.0 utility changes package
ownership to root while creating the archive, so its builder intentionally
requires root (normally `sudo` on the isolated packaging host).

Run the host-side checks with `packaging/nas/validate.sh`.
