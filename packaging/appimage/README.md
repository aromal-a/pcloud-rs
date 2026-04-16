<!-- PLATFORM: Linux -->
# pcloud-rs AppImage

Portable single-file Linux distribution of the Rust rewrite of pcloud-rs.

## Contents

| File | Purpose |
| --- | --- |
| `AppRun` | Entrypoint script. Launches `pcloudc` by default; `--daemon` switches to `pcloudd`. |
| `pcloud-rs.desktop` | Desktop entry for integration with AppImage launchers. |
| `build-appimage.sh` | Builds the workspace and assembles `pcloud-rs-<arch>.AppImage`. |

## Build

Prerequisites on Debian/Ubuntu:

```bash
sudo apt-get install -y curl file libfuse2 libfuse3-dev pkg-config libsqlite3-dev
rustup toolchain install 1.82.0
```

Build (from the repo root):

```bash
./packaging/appimage/build-appimage.sh --arch x86_64
```

This produces `./pcloud-rs-x86_64.AppImage`.

The script:

1. `cargo build --release --workspace --locked` inside ``.
2. Lays out `AppDir/usr/{bin,share/applications,share/icons/hicolor/256x256/apps}`.
3. Copies `pcloudd`, `pcloudc`, the desktop file, and an icon (or placeholder).
4. Downloads `appimagetool` if missing and runs
   `ARCH=x86_64 appimagetool AppDir pcloud-rs-x86_64.AppImage`.

## Manual verification

```bash
chmod +x pcloud-rs-x86_64.AppImage
./pcloud-rs-x86_64.AppImage --version
./pcloud-rs-x86_64.AppImage --help
./pcloud-rs-x86_64.AppImage --daemon --help
```

If FUSE mounting is needed, the host must provide `/dev/fuse` and
`fusermount3` (usually installed with `fuse3`).

## Notes

* AppImages are unsigned by default. Consumers should verify the SHA256
  published alongside release artifacts.
* For aarch64, re-run the script with `--arch aarch64` on an ARM64 host
  (or under QEMU user emulation).
