# Installation

> **Current availability (verified 2026-07-16).** This project has no public
> GitHub release and no published binary package channel yet. Build from a
> reviewed source commit, or build one of the in-tree package recipes locally.
> Do not substitute the placeholder package-manager commands in older copies of
> this guide: they cannot install this project. After a source build, run:
>
> ```bash
> pcloudc --version     # prints version + git hash + build profile
> pcloudc doctor        # self-check probes, exit 0 when healthy
> pcloudc doctor --strict   # promote WARN to FAIL (CI / hardened hosts)
> ```
>
> When the applicable probes succeed, jump to [First login](first-login.md). Workflow definitions
> and packaging recipes are development assets, not proof that an installable
> release exists or that a target has been qualified. NAS outputs remain Tier-2
> candidates until their hardware matrices pass.

## What you'll learn

- Which install path is usable now, which packaging assets can be built locally,
  and which public channels do not yet exist.
- How pCloud is architected at a glance, so you can tell *why* the
  installer creates a `0700` config dir, a user-scoped daemon, and a
  mode-`0600` socket.
- The source-build commands and the intended package layout for future `.deb`,
  `.rpm`, macOS, Windows, community-channel, and BSD releases.
- How to verify the install end-to-end and how to read each probe in
  `pcloudc doctor --strict`.
- The top five install failures and the one-line fix for each.

## Conceptual background

`pcloud-rs` is a **three-piece client** for the pCloud service:

1. **`pcloudc`** — a thin CLI. It parses your command, opens the local
   IPC socket, and blocks until the daemon replies. It never talks to
   the network. It never stores secrets in its own address space any
   longer than one IPC round-trip.
2. **`pcloudd`** — the long-lived service. It owns
   network I/O, the SQLite store, the optional auth-token vault, and
   the sync/mount engines. The in-tree native Linux package recipes install a
   systemd system unit using `DynamicUser=yes`; source/user installs can run it
   as your own UID with the user-unit compatibility drop-in. The IPC socket is
   derived from the active `PCLOUD_ROOT` / XDG runtime layout and is
   mode `0600` inside a `0700` parent directory.
3. **A lifecycle integration** — systemd on Linux, a per-user LaunchAgent on
   macOS, per-user `pcloudc start` on Windows, or rc.d assets on supported
   BSDs. The daemon identity must match the owner of IPC and secret storage.

Why you see those permissions even before first login:

- The daemon authenticates pCloud API calls with a **bearer auth
  token** obtained from your password (plus 2FA) or a service token.
  The token is the crown jewel; guarding its on-disk home by
  `0600`/`0700` is the first line of defence.
- The CLI connects over a **local Unix-domain socket** (or a named
  pipe on Windows). The daemon verifies the peer UID on every accept;
  cross-user connections are refused even when file-system modes would
  allow them.
- **Sync** (file-level replication between a local folder and a
  pCloud folder) and **mount** (virtual filesystem exposing the
  remote tree) are **independent** features sharing the same daemon.
  You can use one, both, or neither.
- The **crypto folder** surface is end-to-end encrypted client-side;
  the server never sees plaintext. Crypto and non-crypto content can
  live side by side in the same account.
- **Public links** share content outside the account via one-off URLs
  with optional password / expiry / upload quota.

> **Expert sidebar (FAANG-ops angle).** Treat `pcloudd` like any
> other per-user sidecar: scoped to one UID, no setuid bits, no root
> capabilities, systemd `ProtectSystem=strict`, `PrivateTmp=yes`. All
> state under `$XDG_STATE_HOME/pcloud-rs`. For fleet rollouts, the
> packaging channel you pick determines your patch-cadence story:
> `.deb` / `.rpm` through an internal apt/dnf mirror gives you the
> cleanest SBOM pipeline; Nix gives you bit-for-bit reproducibility;
> AppImage / standalone `cargo install` does **not** — reserve those
> for dev boxes.

## Intended native-package layout

No public channel currently installs these files. The in-tree native-package
recipes are required to converge on this layout before the first release. If a
locally built package differs, treat that as a packaging defect.

| Artefact | Path (Linux) | Mode |
|---|---|---|
| CLI binary | `/usr/bin/pcloudc` | `0755` |
| Daemon binary | `/usr/bin/pcloudd` | `0755` |
| Systemd unit | `/lib/systemd/system/pcloudd.service` | `0644` |
| Env/config seed | `/etc/pcloud-rs/pcloudd.env.example` | `0644` |
| Runtime/state root | operator-set `PCLOUD_ROOT`, commonly `/var/lib/pcloud-rs` for system services | `0700` |
| Man pages | `pcloudc.1`, `pcloudd.1`, `pcloud.conf.5` | `0644` |
<!-- man-page filenames verified against `packaging/man/` 2026-04-30 (CLAUDEREV iter-1 HIGH-3 fix). -->

See [`packaging/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/README.md)
for the per-channel truth table. Linux raw/package jobs, strict signed macOS
and Windows jobs, and candidate-only NAS jobs are defined, but the repository
has no release to download today. Docker publishing and SLSA provenance remain
unimplemented.

## Step-by-step: current install paths

### Build from source (current cross-platform path)

POSIX source install:

```bash
# Rust 1.89+ — matches workspace Cargo.toml `rust-version`
rustc --version
git clone https://github.com/ezechiel203/pcloud-rs
cd pcloud-rs/
cargo build --workspace --release --locked          # 5–15 min on a laptop
sudo install -m 0755 target/release/pcloudc        /usr/local/bin/
sudo install -m 0755 target/release/pcloudd        /usr/local/bin/
```

Windows source build (keep both executables in the same directory on `PATH`):

```powershell
rustc --version
git clone https://github.com/ezechiel203/pcloud-rs
Set-Location pcloud-rs
cargo build --workspace --release --locked
New-Item -ItemType Directory -Force "$HOME\bin\pcloud-rs" | Out-Null
Copy-Item target\release\pcloudc.exe,target\release\pcloudd.exe "$HOME\bin\pcloud-rs\"
# Add $HOME\bin\pcloud-rs to the user PATH before opening a new shell.
```

`pcloudc start` searches for `pcloudd` beside the CLI and then on `PATH`; do not
install the two binaries into unrelated directories.

What each step does:

- `cargo build --workspace --release --locked` builds every crate in
  the workspace with optimisations on, respecting the committed
  `Cargo.lock`. `--locked` is non-negotiable for reproducibility —
  drop it and you lose the supply-chain chain of custody.
- `install -m 0755 …` installs the binary with a sane mode. Do **not**
  `cp` the artefact; `cp` preserves the build-tree mode and can land
  a `0775` group-writable binary.

Expected output (trimmed):

```
Compiling pcloud-proto v0.1.0
...
Finished release [optimized] target(s) in 6m 42s
```

Common failures:

- **`error: failed to download`** — proxy / offline environment. Add
  `--frozen` and point `CARGO_HOME` at a pre-seeded vendor directory.
- **`linking with cc failed`** — missing system libs (`libssl-dev`,
  `libsqlite3-dev`, `pkg-config`, `fuse3`). On Debian:
  `sudo apt install build-essential pkg-config libssl-dev libsqlite3-dev libfuse3-dev`.

> **Expert tip.** `cargo install --path crates/pcloud-cli --locked`
> lands **only** the CLI in `~/.cargo/bin`. Use it only when you have built and
> installed the matching daemon yourself. CLI and daemon must agree on the IPC
> protocol version.

### Linux native-package recipes (`.deb` / `.rpm`)

The tag workflows define x86-64 `.deb` and `.rpm` outputs, but no tag has
published them. There is no project APT, DNF/YUM, or zypper repository. Build a
package locally only for development and inspect it with the validators in
`packaging/scripts/` before installing it on a disposable host.

### Arch Linux (AUR)

Neither `pcloud-rs-bin` nor `pcloud-rs-git` exists in the AUR. Build from source;
do not run `paru -S` or `yay -S` for those names unless the packaging matrix is
updated with a verified publication record.

### Nix / NixOS

```bash
# from this repo (flakes)
nix build .#pcloudc
nix run .#pcloudc -- --version
nix run .#pcloudd -- --help
```

The flake exposes `packages.<system>.{pcloud-rs,pcloud-rs-repro,pcloudc,pcloudd}`
and `apps.<system>.{pcloudc,pcloudd}`. `default` runs `pcloudc`. Checks are
`fmt`, `clippy`, `test`, and package builds. No `nixosModules.pcloud-rs`
output exists yet.

### Docker / OCI

```bash
docker build -f packaging/docker/Dockerfile -t pcloud-rs:dev .
docker run --rm -it \
  -v pcloud-rs-state:/var/lib/pcloud-rs \
  --entrypoint /usr/local/bin/pcloudc \
  pcloud-rs:dev --version
```

No `.github/workflows/docker.yml` exists today, so no GHCR image or cosign
OCI signature is published by this repository. Treat Docker as a local
build/scanning recipe until a publish workflow lands.

The image does **not** bundle FUSE. Container mount needs
`--cap-add SYS_ADMIN --device /dev/fuse` plus a matching host kernel —
we do not recommend it outside controlled CI. `pcloudc sync add`
works fine inside the container.

### Community channels: unavailable

AppImage, Flatpak (`com.pcloud.pcloud-rs`), Snap, Homebrew, winget,
Chocolatey, and Scoop have in-tree scaffolding only. None is a supported public
install channel. In particular, there is no `ezechiel203/homebrew-pcloud-rs`
tap or `ezechiel203/scoop-bucket`, and the registry package names shown in old
documentation are not published.

### macOS `.pkg`: release pipeline only

The strict workflow is designed to emit a signed, notarized, stapled package
only after native fuse-t tests and Gatekeeper assessment. No such package has
been published. Build from source on macOS for development; do not use a
`releases/latest` URL until an actual qualified release exists.

### Windows MSI/Burn: release pipeline only

The strict workflow is designed to build Authenticode-signed binaries, MSI,
and a signed WinFSP Burn bootstrapper. No installer has been published. Build
from source on Windows for development; winget, Chocolatey, and Scoop cannot
install this project today.

### FreeBSD / NetBSD / OpenBSD / DragonFly BSD

```bash
# Build from source; downstream binary packages are not published here.
git clone https://github.com/ezechiel203/pcloud-rs
cd pcloud-rs/
cargo build --workspace --release --locked
sudo install -m 0755 target/release/pcloudc         /usr/local/bin/
sudo install -m 0755 target/release/pcloudd         /usr/local/bin/
```

Each explicitly supported BSD has a strict native workspace and live FUSE job.
Those jobs must pass for the release commit; in-tree rc.d assets do not imply a
published downstream package.

## Verification

Every locally built installation uses the same two probes:

```bash
pcloudc --version
pcloudc doctor
```

### `pcloudc --version`

```
pcloudc 0.1.0 (commit abc1234, release)
```

Field-selector extraction for scripting:

```bash
pcloudc --version | awk '{ print $2 }'            # → 0.1.0
pcloudc --version | grep -oP 'commit \K[0-9a-f]+' # → abc1234
```

### `pcloudc doctor`

Representative categories from a healthy run:

```
[OK]   daemon reachable
[OK]   config and vault permissions
[OK]   network reachable: binapi.pcloud.com:443
[OK]   managed directories are owner-only
[OK]   upload journal clean
summary: 8 ok, 0 warn, 0 fail
```

The exact count varies with configured mount roots and optional vault state.

What each probe means:

| Probe | Healthy | Failing | Fix |
|---|---|---|---|
| `config:` | `(mode 0600, ok)` | `(mode 0644, expected 0600)` | `chmod 0600 ~/.config/pcloud-rs/config.toml && chmod 0700 ~/.config/pcloud-rs` |
| `runtime:` | `(mode 0700, ok)` | `owned by root` | `sudo rm -rf ~/.local/state/pcloud-rs && pcloudc start` as your user |
| `socket:` | `(0600, peer-UID ok)` | `0666` or `peer UID mismatch` | Stop daemon, remove runtime dir, restart |
| `tls:` | `mandatory` | `(downgrade allowed)` | Never ship to prod; re-install from the official channel |
| `fuse:` | `provider = fuse-t\|fuse3\|fusefs 3.x+` | `provider not found` | Install the FUSE package for your OS, or ignore if you do not use mount |

### `pcloudc doctor --strict`

CI/hardened-host mode. Any WARN (unknown TLS root, unsigned daemon
binary, weakly permissive parent directory) is promoted to a FAIL and
the exit status becomes non-zero. Use this in your image-baking
pipeline, not in interactive troubleshooting.

```bash
pcloudc doctor --strict
echo "doctor exit: $?"                 # 0 only if everything clean
pcloudc doctor --json | jq -r '.checks[] | select(.status!="ok") | .name'
```

> **Expert tip.** `pcloudc doctor --json` can feed existing observability, but
> the project is still version `0.1.0` and has not published a stable JSON
> schema. Pin consumers to a reviewed commit until the SDK/CLI SemVer contract
> is released.

## Troubleshooting — top five

1. **`pcloudc: command not found`** — `PATH` not refreshed. Log out
   and back in, or `hash -r` / `rehash`. On NixOS, add the package to
   `environment.systemPackages`, not just `nix-shell`. macOS `.pkg`:
   make sure `/usr/local/bin` is on `PATH`.
2. **`config: mode is 0644, expected 0600`** — another tool rewrote
   the file. Fix with the `chmod` one-liner above. The daemon
   **refuses to start** with loose modes; this is deliberate.
3. **`socket: runtime dir ~/.local/state/pcloud-rs is owned by root`**
   — you ran `pcloudc` once with `sudo`. Remove the dir:
   `sudo rm -rf ~/.local/state/pcloud-rs`, then run `pcloudc doctor`
   as your own user.
4. **`fuse: provider not found`** — only matters if you intend to
   mount. Install FUSE3 (Linux/BSD), fuse-t (macOS), or the verified WinFSP
   runtime (Windows).
5. **Windows `pcloudc start` times out** — inspect the per-user
   `daemon.log` under the pcloud-rs data directory. Do not create an SCM
   service: its SID and DPAPI scope would not match the interactive user.

## Next steps

- [First login](first-login.md) — start the daemon, authenticate,
  handle 2FA and the scripted login flow.
- [First sync](first-sync.md) — register a sync root, watch progress,
  understand conflict policy.
- [Configuration reference](../reference/config.md) — every key of
  `config.toml`.
- [Packaging matrix](../operations/packaging-matrix.md) — per-channel
  ownership, signing posture, SBOM coverage.
- [Exit codes](../reference/exit-codes.md) — the full table referenced
  by troubleshooting sections across the book.
