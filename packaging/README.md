# `packaging/` — In-tree packaging assets

This directory is the **source of truth for OS-native packaging recipes**
for the `pcloud-rs` Rust rewrite (the binaries produced out of
``). Each subdirectory owns one distribution channel (deb, rpm,
AppImage, Flatpak, Snap, Homebrew, Chocolatey, Scoop, winget, Docker,
MSI, Flathub metainfo, macOS launchd, *BSD rc.d) or one cross-cutting
concern (code signing, notarisation, man pages).

If you are looking for:

- an **operator-facing** summary of which channel targets which OS, see
  [`docs/book/src/operations/packaging-matrix.md`](../docs/book/src/operations/packaging-matrix.md).
- the **deep per-channel reference** (flags, CI wiring, caveats), see
  [`docs/book/src/reference/packaging.md`](../docs/book/src/reference/packaging.md).
- the **reproducible-builds** methodology, see the operations chapter
  in the mdBook — a dedicated chapter is pending; for now see the
  "Reproducible builds" section in this README and in
  `docs/book/src/reference/packaging.md`.

> **Honesty note (pre-alpha).** Several formats in this tree are
> **scaffolding**: they carry placeholder URLs (`vX.Y.Z`), placeholder
> SHA256s (`0000…`), placeholder GUIDs, and/or `have_secrets=false`
> fallbacks in CI. They are wired structurally so a maintainer can mint
> a real release by replacing placeholders, but no live channel publish
> has occurred yet. The "Status" column in the table below flags each
> one.

## Subtree index

| Path           | Format            | What it builds / registers                              | Status        |
|----------------|-------------------|---------------------------------------------------------|---------------|
| `appimage/`    | Linux AppImage    | Single-file portable `.AppImage` bundle (FUSE2)         | Working       |
| `bsd/`         | Docs              | Shared BSD deployment notes (no artefacts)              | Docs only     |
| `chocolatey/`  | Windows .nupkg    | `choco install pcloud-rs` package referencing the MSI    | Scaffolding   |
| `docker/`      | OCI image         | Multi-arch `ghcr.io/.../pcloud-rs:<tag>` container       | Working, cosign-signed in CI |
| `flatpak/`     | Flatpak (.flatpak) | `com.pcloud.pcloud-rs` app bundle for Flathub           | Local-build working; Flathub PR pending |
| `freebsd/`     | rc.d script       | `/usr/local/etc/rc.d/pcloudd` service                   | Working on FreeBSD |
| `homebrew/`    | Ruby formula      | `brew install pcloud-rs` (source build) + `fuse-t` cask  | Scaffolding   |
| `systemd/`     | systemd unit      | Canonical unit at `packaging/systemd/pcloudd.service`. Drop-ins: `override.conf.example` (API access — required before first start), `override-fuse.conf.example` (FUSE mount), `override-user.conf.example` (required for `--user` installs). | Working |
| `macos/`       | launchd plists    | User LaunchAgent + system LaunchDaemon + entitlements   | Plists working; notarisation pending |
| `man/`         | troff             | `pcloudc(1)`, `pcloudd(1)`, `pcloud.conf(5)`            | Working (owned by another agent) |
| `netbsd/`      | rc.d script       | `/etc/rc.d/pcloudd` service                             | Scaffolding   |
| `openbsd/`     | rc.d script       | `/etc/rc.d/pcloudd` service                             | Scaffolding   |
| `scoop/`       | Scoop manifest    | `scoop install pcloud-rs` (ZIP + winfsp dep)             | Scaffolding   |
| `signing/`     | Shell / PS        | `sign-macos.sh`, `notarize-macos.sh`, `sign-windows.ps1`| Working; EV cert vendor-bound |
| `snap/`        | snapcraft.yaml    | Strict-confined snap (classic needed for FUSE mount)    | Scaffolding   |
| `windows/wix/` | WiX (.wxs)        | `pcloud-rs-X.Y.Z-x64.msi` with Windows service install   | Scaffolding   |
| `winget/`      | winget manifest   | `winget install pcloud-rs` referencing the MSI          | Scaffolding   |

> The systemd units (under `packaging/systemd/` and
> `packaging/init/systemd/`) and the `man/` pages here are
> maintained by sibling packaging agents; they are cross-referenced
> here for completeness but this file's author does not own them.

## Install-layout reference

Every packaging channel in this tree must agree on these paths.

| Artefact                    | Linux deb/rpm                | Linux snap / Docker image          | macOS Homebrew          | macOS pkg              | Windows MSI                                      |
|-----------------------------|------------------------------|------------------------------------|--------------------------|------------------------|--------------------------------------------------|
| Daemon binary (`pcloudd`)   | `/usr/bin/pcloudd`           | `/usr/local/bin/pcloudd`           | `$(brew --prefix)/bin/pcloudd` | `/usr/local/libexec/pcloudd` | `C:\Program Files\pcloud-rs\pcloudd.exe` |
| CLI binary (`pcloudc`)      | `/usr/bin/pcloudc`           | `/usr/local/bin/pcloudc`           | `$(brew --prefix)/bin/pcloudc` | `/usr/local/bin/pcloudc`    | `C:\Program Files\pcloud-rs\pcloudc.exe` |
| State root (`$PCLOUD_ROOT`) | `~/.config/pcloud/` (user)   | `/var/lib/pcloud-rs/` (container)   | `~/Library/Application Support/pcloud` | `/var/lib/pcloudd/`  | `%APPDATA%\pcloud-rs\`                 |
| Config file                 | `~/.config/pcloud/config.toml` | `/etc/pcloud-rs/pcloud-rs.toml`      | `~/.config/pcloud/config.toml` | `/etc/pcloud/pcloudd.toml` | `%APPDATA%\pcloud-rs\config.toml`        |
| Auth vault                  | `$PCLOUD_ROOT/auth.vault` (0600 file, 0700 dir) — opt-in only              |||||

> **Systemd ExecStart paths must match the installed location.** The
> current `packaging/systemd/pcloudd.service` uses
> `ExecStart=/usr/bin/pcloudd serve`, which matches the deb/rpm install
> layout. Docker image / AppImage / local `cargo install` layouts put
> the binary at `/usr/local/bin/pcloudd`; those deployments must supply
> their own override unit or rewrite `ExecStart=` at package-build time.
> The user unit install (`systemctl --user`) additionally requires
> `override-user.conf.example` to strip system-only directives.

## Environment-variable surface

These are the variables the Rust daemon and CLI **actually read** (cross-
checked against `pcloud-config/src/env.rs`, `pcloud-daemon/src/*.rs`, and
`pcloud-cli/src/config.rs`). Anything not on this list is inert at
runtime, regardless of what a plist or systemd drop-in may try to set.

| Variable                              | Scope       | Effect |
|---------------------------------------|-------------|--------|
| `PCLOUD_ROOT`                         | daemon+CLI  | Re-roots config/state/runtime/cache/plugins under a single directory. |
| `PCLOUD_ENV`                          | daemon      | Selects bootstrap profile (`dev`/`test`/`prod`); snaps `api.mode` to the secure default for that env unless `PCLOUD_API_MODE` is also set. |
| `PCLOUD_API_MODE`                     | daemon      | `plaintext` / `tls`. **Production rejects `plaintext`.** |
| `PCLOUD_API_HOST` / `_PORT`           | daemon      | Override API endpoint (still TLS-validated). |
| `PCLOUD_API_SERVER_NAME`              | daemon      | TLS SNI / cert verification name. |
| `PCLOUD_API_CONNECT_TIMEOUT_MS`       | daemon      | Connect timeout (ms). `0` is rejected. |
| `PCLOUD_API_READ_TIMEOUT_MS`          | daemon      | Read timeout (ms). `0` is rejected. |
| `PCLOUD_CONFIG`                       | CLI         | Path to the user config TOML. |
| `PCLOUD_LOG_LEVEL`                    | daemon+CLI  | `trace`/`debug`/`info`/`warn`/`error`. |
| `PCLOUD_DURABLE_AUTH_TOKENS`          | daemon      | Opt-in to the on-disk auth vault (default off). |
| `PCLOUD_VAULT`                        | daemon (Linux) | `secret-service` (default on GNOME/KDE) / `file`. |
| `PCLOUD_PLUGINS_ENABLED`              | daemon      | Gate plugin registry (default off). |
| `PCLOUD_PLUGIN_ALLOW_NETWORK`         | daemon      | Allow plugins to make outbound network calls. |
| `PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL`    | daemon      | Allow plugins to manage sync roots. |
| `PCLOUD_PLUGIN_ALLOW_CRYPTO`          | daemon      | Allow plugins to access crypto ops. |
| `PCLOUD_MIGRATE_LEGACY_PATHS`         | daemon (Linux) | One-shot XDG migration opt-in. |
| `PCLOUD_CACHE_SIZE_GB`                | daemon      | Cache-size hint exported by `pcloudc start`. |
| `PCLOUD_AUDIT_HMAC_KEY`               | daemon      | HMAC key for tamper-evident audit log. |
| `PCLOUD_METRICS_BIND_ALL` / `_PORT`   | daemon (dev)| Bind the metrics server to `0.0.0.0` (dev-only; rejected in prod). |
| `PCLOUD_FORCE_UMOUNT`                 | daemon      | Equivalent to `pcloudc mount --force-umount`. |
| `PCLOUD_FS_EVENT_LOG` / `PCLOUD_FUSE_OPTS` | daemon | FS event log target and FUSE mount options passthrough. |

Variables referenced in older packaging files but **not read** by the
daemon (kept for readability; they are silently ignored):

- `PCLOUD_HOME` — shadowed by `PCLOUD_ROOT`.
- `PCLOUD_MOUNT_POINT` — mount point is per-call CLI, not env.
- `PCLOUD_IPC_SOCKET` — IPC path is derived from `PCLOUD_ROOT`.
- `PCLOUD_AUTH_VAULT` — the vault path is derived from `PCLOUD_ROOT`.
- `PCLOUD_API_SERVER` — superseded by `PCLOUD_API_HOST` + `PCLOUD_API_SERVER_NAME`.

Packaging files that set these legacy names will not break the daemon;
they are just cosmetic. Prefer the table above for anything new.

## Build recipes

All recipes assume you are at the **repository root** (`pcloud-rs/`)
unless otherwise noted.

### Debian / Ubuntu `.deb`

The `.deb` is produced by a sibling agent via `cargo deb` against
``. The in-tree recipe lives under the sibling's `debian/`
directory (outside this README's scope). A representative invocation:

```bash
cd . && cargo deb -p pcloud-daemon && cargo deb -p pcloud-cli
dpkg -i target/debian/pcloudd_*_amd64.deb target/debian/pcloudc_*_amd64.deb
```

### Fedora / RHEL / openSUSE `.rpm`

Also sibling-owned (`rpm/.spec`). Representative invocation:

```bash
cd . && cargo generate-rpm -p pcloud-daemon
rpm -ivh target/generate-rpm/pcloudd-*.x86_64.rpm
```

### Linux AppImage (this tree)

```bash
./packaging/appimage/build-appimage.sh --arch x86_64
# Output: ./pcloud-rs-x86_64.AppImage
```

### Flatpak (this tree)

```bash
flatpak-builder --user --install --force-clean \
    build-dir packaging/flatpak/com.pcloud.pcloud-rs.yaml
flatpak run com.pcloud.pcloud-rs --help
```

### Snap (this tree)

```bash
snapcraft --use-lxd
sudo snap install --dangerous ./pcloud-rs_*.snap
```

> Strict confinement blocks FUSE; switch to `classic` (and submit for
> Snap Store review) if you need the mounted drive.

### Docker (this tree)

```bash
docker build -f packaging/docker/Dockerfile -t pcloud-rs:dev 
```

### macOS `.pkg` (sibling agent, signed via this tree's `signing/`)

```bash
# Build universal binary first (sibling job), then:
./packaging/signing/sign-macos.sh ./build/pcloud-rs.app \
    "Developer ID Application: <Your Org> (TEAMID)"
./packaging/signing/notarize-macos.sh ./dist/pcloud-rs-X.Y.Z.pkg
```

### macOS Homebrew

```bash
brew install --build-from-source ./packaging/homebrew/pcloud-rs.rb
brew services start pcloud-rs
```

> The formula currently installs two binaries via `cargo install`
> separately; see `packaging/homebrew/README.md` for the caveat about
> matching `cargo-install`-produced binary names against the `service`
> stanza.

### Arch PKGBUILD

Not in-tree; point users to AUR via the reference doc.

### Nix flake

The flake lives at the repository root (`flake.nix`), not under this
directory. Run `nix build .#pcloud-rs` from the repo root.

### Windows WiX MSI

```powershell
cargo install cargo-wix
cargo wix --install-version X.Y.Z
# Output: target\wix\pcloud-rs-X.Y.Z-x86_64.msi
```

### Windows `winget`

```powershell
winget install pCloud.pcloud-rs
```

### Windows Chocolatey

```powershell
choco install pcloud-rs
```

### Windows Scoop

```powershell
scoop install pcloud-rs
```

## Signing posture

| Target       | Mechanism                                          | Status / Notes                                   |
|--------------|----------------------------------------------------|--------------------------------------------------|
| Linux tarball, `.deb`, `.rpm` | GPG detached `.asc` alongside artefacts | Working; key is the release maintainer's key    |
| Linux tarball | `cosign sign-blob` (sigstore keyless OIDC)        | Working in CI                                    |
| Docker image | `cosign sign` (sigstore keyless OIDC)              | Working; signature + Rekor transparency-log entry |
| macOS `.pkg` | `codesign --options runtime --timestamp` + `notarytool` + `stapler` | **Vendor-bound on Apple Developer ID enrolment** (`$99/year`). See `signing/README.md` §7 for the first-time runbook. |
| Windows MSI  | `signtool sign /fd sha256 /tr /td sha256`          | **OV cert path wired; EV cert vendor-bound** on DigiCert / Sectigo / SSL.com HSM token or cloud HSM (`~$400-700/year`). SmartScreen reputation warm-up is weeks for OV vs instant for EV. |

See `packaging/signing/README.md` for the full operator guide,
certificate acquisition, CI secret inventory, and disaster-recovery
procedures.

## Reproducible builds

Every packaging job in CI pins:

- the Rust toolchain via `rust-toolchain.toml` at the repo root,
- `CARGO_LOCKED=1` / `cargo --locked` for deterministic dep graph,
- `SOURCE_DATE_EPOCH` from the tag's commit timestamp,
- OS images (`ubuntu-22.04`, `macos-14`, `windows-latest`) pinned by
  digest or by major version,
- WinFSP, fuse-t, FUSE3 library versions.

Two independent builds of the same tag therefore produce byte-identical
artefacts (modulo signing-timestamp bytes, which are embedded *after*
reproducibility is measured). The reproducibility methodology is in the
operations chapter of the mdBook; CI includes a `diffoscope` job on
release tags.

## Known gaps

- **macOS notarisation.** Wired end-to-end, but the `build-macos` job
  currently carries `continue-on-error: true` and the signing secrets
  are absent. Swap to the real secrets and flip `continue-on-error:
  false` once Apple Developer Program enrolment completes. First-time
  runbook: `signing/README.md` §7.
- **Windows EV Authenticode.** OV cert signing works today; EV cert
  signing requires a hardware token (self-hosted runner) or a cloud
  HSM (provider-specific tooling). See `signing/README.md` §2.
- **macOS FUSE.** `fuse-t` is not yet in `homebrew-cask`; the fallback
  cask under `homebrew/Casks/fuse-t.rb` is a pinned scaffolding copy.
- **Snap + FUSE.** Strict confinement blocks mounts; classic
  confinement requires Snap Store review.
- **BSD builds.** rc.d scripts are in place but the Rust daemon does
  not yet build cleanly on \*BSD hosts (see `PLAN_CROSSPLATFORM.md`).

## Cross-cutting security posture

- Every channel installs binaries with user-owned mode bits only
  (`0755`) and leaves state directories at `0700`.
- No channel persists passwords by default (auth vault is opt-in via
  `PCLOUD_DURABLE_AUTH_TOKENS=1`).
- No channel loosens the TLS-only production transport policy; attempts
  to set `PCLOUD_API_MODE=plaintext` in a production build are rejected
  at daemon startup.
- macOS entitlements explicitly pin JIT, dyld env overrides, and
  library validation **off** (see `macos/entitlements.plist`).
- Docker image runs as a non-root user (`pcloud-rs`, uid/gid 1000)
  under `tini` for correct signal handling.
- Windows service is installed as `LocalSystem` by the WiX MSI; a
  follow-up ticket will switch it to a dedicated low-privilege
  service account once the daemon's mount-on-behalf-of-user story is
  finalised.

## How to add a new packaging channel

1. Create a new subdirectory under `packaging/<channel>/`.
2. Add a `README.md` stating the platform, status (working /
   scaffolding), and the release process.
3. Pin the install layout to the **Install-layout reference** table
   above, or update that table with an explicit rationale.
4. Reference the secrets and signing requirements from
   `signing/README.md`; never invent a new signing pipeline inline.
5. Add a row to the **Subtree index** table above and to
   `docs/book/src/operations/packaging-matrix.md`.
6. Open a bead under `bd-1du.10` (final parity proof) if the new
   channel introduces a user-visible behaviour change.
