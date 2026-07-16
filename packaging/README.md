# `packaging/` — In-tree packaging assets

This directory is the **source of truth for in-tree packaging recipes**
for the `pcloud-rs` Rust rewrite (the shipped binaries are `pcloudc`
and `pcloudd`). Each subdirectory owns one distribution channel (deb, rpm,
AppImage, Flatpak, Snap, Homebrew, Chocolatey, Scoop, winget, Docker,
MSI, Flathub metainfo, macOS launchd, *BSD rc.d) or one cross-cutting
concern (code signing, notarisation, man pages). `nas/` owns Tier-2
Synology, QNAP, and ASUSTOR candidates.

If you are looking for:

- an **operator-facing** summary of which channel targets which OS, see
  [`docs/book/src/operations/packaging-matrix.md`](../docs/book/src/operations/packaging-matrix.md).
- the **deep per-channel reference** (flags, CI wiring, caveats), see
  [`docs/book/src/reference/packaging.md`](../docs/book/src/reference/packaging.md).
- the **reproducible-builds** methodology, see the operations chapter
  in the mdBook — a dedicated chapter is pending; for now see the
  "Reproducible builds" section in this README and in
  `docs/book/src/reference/packaging.md`.

> **Evidence note (2026-07-15).** Several community-channel manifests still
> carry future-version URLs or hashes. The macOS and Windows release jobs are
> strict, credential-gated pipelines, while NAS outputs are intentionally
> candidate-only. A workflow definition is not proof that its native job or
> hardware matrix passed for a release commit.

## Subtree index

| Path           | Format            | What it builds / registers                              | Status        |
|----------------|-------------------|---------------------------------------------------------|---------------|
| `appimage/`    | Linux AppImage    | Single-file portable `.AppImage` bundle (FUSE2)         | Local scaffold |
| `bsd/`         | Docs              | Shared BSD deployment and lifecycle contract             | Working reference |
| `chocolatey/`  | Windows .nupkg    | `choco install pcloud-rs` package referencing the MSI    | Scaffolding   |
| `docker/`      | OCI image         | Local Dockerfile / compose recipe                        | Local scaffold; no GHCR publish workflow |
| `flatpak/`     | Flatpak (.flatpak) | `com.pcloud.pcloud-rs` app bundle for Flathub           | Local scaffold; Flathub PR pending |
| `freebsd/`     | rc.d script       | `/usr/local/etc/rc.d/pcloudd` service                   | In-tree asset; native install test pending |
| `dragonfly/`   | rc.d script       | `/usr/local/etc/rc.d/pcloudd` supervised service        | In-tree asset + native candidate; install test pending |
| `homebrew/`    | Ruby formula      | `brew install pcloud-rs` (source build) + `fuse-t` cask  | Scaffolding   |
| `systemd/`     | systemd unit      | System unit at `packaging/systemd/pcloudd.service`; user unit at `packaging/systemd/pcloudd-user.service`. Drop-ins: `override.conf.example` (optional strict egress allow-list), `override-fuse.conf.example` (FUSE mount), `override-user.conf.example` (legacy user-unit compatibility). | Working |
| `macos/`       | pkg / launchd     | Signed/notarized package, safe user LaunchAgent helper  | Strict release workflow; credentials/native runner required |
| `man/`         | troff             | `pcloudc(1)`, `pcloudd(1)`, `pcloud.conf(5)`            | Working (owned by another agent) |
| `nas/`         | SPK/QPKG/APK      | Synology, QNAP, and ASUSTOR native package candidates   | CI-built candidates; hardware qualification required |
| `netbsd/`      | rc.d script       | `/etc/rc.d/pcloudd` service                             | In-tree asset; native install test pending |
| `openbsd/`     | rc.d script       | `/etc/rc.d/pcloudd` service                             | In-tree asset; native install test pending |
| `solarish/`    | SMF manifest/method | `svc:/site/pcloud-rs:default`                          | In-tree asset + native candidates; install test pending |
| `scoop/`       | Scoop manifest    | `scoop install pcloud-rs` (ZIP + winfsp dep)             | Scaffolding   |
| `signing/`     | Shell / PS        | `sign-macos.sh`, `notarize-macos.sh`, `sign-windows.ps1`| Wired into strict macOS/Windows release jobs |
| `snap/`        | snapcraft.yaml    | Strict-confined snap (classic needed for FUSE mount)    | Scaffolding   |
| `unix/`        | deterministic `.tar.gz` | DragonFly, OmniOS, and Solaris binary/service candidates | CI-built candidates; not downstream OS packages |
| `windows/wix/` | WiX MSI/Burn      | Signed MSI plus WinFSP bootstrapper; per-user daemon binaries | Strict release workflow; credentials required |
| `winget/`      | winget manifest   | `winget install pcloud-rs` referencing the MSI          | Scaffolding   |

> The systemd units under `packaging/systemd/` and the `man/` pages here are
> maintained by sibling packaging agents; they are cross-referenced
> here for completeness but this file's author does not own them.

## Install-layout reference

Every packaging channel in this tree must agree on these paths.

| Artefact                    | Linux deb/rpm                | Linux snap / Docker image          | macOS Homebrew          | macOS pkg              | Windows MSI                                      |
|-----------------------------|------------------------------|------------------------------------|--------------------------|------------------------|--------------------------------------------------|
| Daemon binary (`pcloudd`)   | `/usr/bin/pcloudd`           | `/usr/local/bin/pcloudd`           | `$(brew --prefix)/bin/pcloudd` | `/usr/local/libexec/pcloudd` | `C:\Program Files\pcloud-rs\pcloudd.exe` |
| CLI binary (`pcloudc`)      | `/usr/bin/pcloudc`           | `/usr/local/bin/pcloudc`           | `$(brew --prefix)/bin/pcloudc` | `/usr/local/bin/pcloudc`    | `C:\Program Files\pcloud-rs\pcloudc.exe` |
| Unit / supervisor            | `/lib/systemd/system/pcloudd.service` | container entrypoint | launchd LaunchAgent | packaged user LaunchAgent | per-user `pcloudc start` |
| State root (`$PCLOUD_ROOT`) | operator-set, commonly `/var/lib/pcloud-rs` | `/var/lib/pcloud-rs/` | `~/Library/Application Support/pcloud-rs` | `/var/lib/pcloud-rs/` | `%APPDATA%\pcloud-rs\` |
| Config / env seed            | `/etc/pcloud-rs/pcloudd.env.example` | env / secret files | plist env block | plist env block | per-user config / inherited environment |
| Auth token vault             | `$PCLOUD_ROOT/config/auth_token` when durable tokens are enabled              |||||

> **Systemd ExecStart paths must match the installed location.** The
> current `packaging/systemd/pcloudd.service` uses
> `ExecStart=/usr/bin/pcloudd serve`, which matches the deb/rpm install
> layout. Docker image / AppImage / local `cargo install` layouts put
> the binary at `/usr/local/bin/pcloudd`; those deployments must supply
> their own override unit or rewrite `ExecStart=` at package-build time.
> Per-user installs should use `packaging/systemd/pcloudd-user.service`.
> Do not install the system unit under `systemctl --user`: it contains
> `DynamicUser=` and managed-directory directives that user managers reject.

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
| `PCLOUD_CONFIG`                       | daemon+CLI  | Mandatory explicit path to a JSON config envelope when set. Do not point it at `config.toml`; the current loader expects JSON. |
| `PCLOUD_LOG_LEVEL`                    | daemon+CLI  | `trace`/`debug`/`info`/`warn`/`error`. |
| `PCLOUD_DURABLE_AUTH_TOKENS`          | daemon      | Opt-in to the on-disk auth vault (default off). |
| `PCLOUD_VAULT`                        | daemon      | `auto` / `file` / `keychain` / `dpapi` / `secret-service`; incompatible explicit choices fail. |
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
| `PCLOUDRS_TOKEN_FILE`                 | daemon bootstrap | File containing an auth token for non-interactive startup; regular file, owner-only `0600`. |
| `PCLOUDRS_USERNAME_FILE` / `PCLOUDRS_PASSWORD_FILE` | daemon bootstrap | First-boot login credentials from files. Must be set together; same permission rules as token files. |
| `PCLOUDRS_TFA_CODE_FILE` / `PCLOUDRS_RECOVERY_CODE_FILE` | daemon bootstrap | Mutually exclusive second-factor files for non-interactive username/password login. |
| `PCLOUDRS_TRUST_DEVICE`               | daemon bootstrap | Boolean; asks the TFA flow to trust the device after successful bootstrap. |
| `CREDENTIALS_DIRECTORY`               | daemon bootstrap | systemd credential directory. The daemon falls back to `pcloud-rs-token`, `pcloud-rs-username`, `pcloud-rs-password`, `pcloud-rs-tfa-code`, and `pcloud-rs-recovery-code` inside this directory when the matching `PCLOUDRS_*_FILE` var is unset. |

The `PCLOUDRS_*_FILE` variables are **secret bootstrap inputs**, not
general config overrides. They are consumed while establishing a session;
password and second-factor contents must never be placed directly in
`Environment=`.

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

The `.deb` is produced in `.github/workflows/release-packaging.yml` via
`cargo deb` against the metadata in `crates/pcloud-daemon/Cargo.toml`.
A representative invocation:

```bash
cargo build --release --workspace --locked -p pcloud-daemon -p pcloud-cli
cargo deb --no-build --no-strip --package pcloud-daemon
sudo dpkg -i target/debian/pcloud-rs_*_amd64.deb
```

### Fedora / RHEL / openSUSE `.rpm`

The `.rpm` is produced in `.github/workflows/release-packaging.yml` via
`cargo-generate-rpm` against the metadata in
`crates/pcloud-daemon/Cargo.toml`. Representative invocation:

```bash
cargo build --release --workspace --locked -p pcloud-daemon -p pcloud-cli
cargo generate-rpm --package crates/pcloud-daemon --auto-req auto
sudo rpm -ivh target/generate-rpm/pcloud-rs-*.x86_64.rpm
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
docker build -f packaging/docker/Dockerfile -t pcloud-rs:dev .
```

### macOS signed/notarized `.pkg`

```bash
# The release job runs the strict equivalent with ephemeral credentials.
./packaging/macos/build-pkg.sh \
  --application-sign "Developer ID Application: <Your Org> (TEAMID)" \
  --installer-sign "Developer ID Installer: <Your Org> (TEAMID)" \
  --notarize
```

### macOS Homebrew

The formula is scaffolding with a future tag URL and checksum. It cannot be
installed until those placeholders are replaced by a qualified release. Its
service stanza is kept testable and invokes the real long-running command,
`pcloudd serve`.

### Arch PKGBUILD

Not in-tree and not published in the AUR. Use a source build.

### Nix flake

The flake lives at the repository root (`flake.nix`), not under this
directory. Run `nix build .#pcloud-rs` from the repo root.

### Windows WiX MSI and WinFSP bootstrapper

```powershell
# See packaging/windows/wix/README.md for the WiX v3 commands and signing
# sequence used by release-packaging.yml.
```

### Windows `winget`

Scaffolding only. No manifest is published in `microsoft/winget-pkgs`.

### Windows Chocolatey

Scaffolding only. No `pcloud-rs` package is published in the community feed.

### Windows Scoop

Scaffolding only. No project Scoop bucket or upstream manifest is published.

## Signing posture

| Target       | Mechanism                                          | Status / Notes                                   |
|--------------|----------------------------------------------------|--------------------------------------------------|
| Raw Linux binaries + SBOMs | `cosign sign-blob` (sigstore keyless OIDC by default) | Working in `.github/workflows/release.yml`; emits `.sig` and keyless `.pem` |
| `.deb`, `.rpm`, `SHA256SUMS` | GPG detached signatures when release secrets exist | Workflow permits visibly unsigned dry runs; public policy must require signatures |
| Docker image | none | No GHCR publish/sign workflow exists today |
| macOS `.pkg` | `codesign`, `productsign`, `notarytool`, `stapler`, `spctl` | Strict release job requires Apple credentials and native fuse-t gate |
| Windows executables/MSI/Burn | `signtool` SHA-256 + RFC 3161 timestamp | Strict release job requires a signing PFX; final Burn engine and bundle are signed |
| NAS SPK/QPKG/APK candidates | `SHA256SUMS.<arch>.txt` | Actions-only until vendor hardware qualification |

See `packaging/signing/README.md` for the full operator guide,
certificate acquisition, CI secret inventory, and disaster-recovery
procedures.

## Reproducible builds

The raw binary release and reproducibility jobs in CI pin:

- the Rust toolchain via `rust-toolchain.toml` at the repo root,
- `cargo --locked` for deterministic dep graph,
- `cargo auditable build --profile release-repro` for the binaries that
  are signed and uploaded,
- `SOURCE_DATE_EPOCH` from the tag's commit timestamp,
- path remapping and `-Wl,--build-id=none`,
- the Ubuntu runner image selected by GitHub Actions,
- FUSE3 build dependencies.

Two independent raw-binary builds of the same tag are intended to produce
byte-identical artefacts. The `.deb` / `.rpm` packaging workflow still
uses `cargo build --release` and must not be described as byte-reproducible
until that workflow is switched to the reproducible profile and verified.

## Known gaps

- **Native release evidence.** The macOS and Windows jobs exist, but a release
  is supported only after those strict jobs pass with real credentials.
- **Docker publish/signing.** The Dockerfile is used for local builds and
  scheduled Trivy scanning only. No GHCR publish or cosign OCI signature
  workflow exists.
- **macOS FUSE.** `fuse-t` is not yet in `homebrew-cask`; the fallback
  cask under `homebrew/Casks/fuse-t.rb` is a pinned scaffolding copy.
- **Snap + FUSE.** Strict confinement blocks mounts; classic
  confinement requires Snap Store review.
- **BSD packaging.** Native runtime/mount gates and rc.d assets exist for all
  four BSDs; downstream ports/pkgsrc publication and native install/upgrade
  qualification remain outstanding.
- **Solaris-family packaging.** Native API/CLI jobs, SMF assets, and retained
  deterministic candidates exist. Kernel mounting is explicitly unsupported;
  IPS publication and native install/upgrade qualification remain outstanding.
- **NAS hardware.** Archive validation cannot replace install/upgrade/reboot
  and live transfer tests on Synology, QNAP, and ASUSTOR hardware.

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
- Docker image runs as the distroless non-root user (`65532:65532`) with
  `pcloudd serve` as PID 1.
- Windows deliberately uses a per-user daemon. Named-pipe SID checks, DPAPI,
  and WinFSP mounts must share the interactive user's identity; the MSI does
  not register an SCM service account.

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
