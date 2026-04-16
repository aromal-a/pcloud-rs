# Installation

> **TL;DR** — pick the install path for your OS, then run the same two
> verification commands everywhere:
>
> ```bash
> pcloudc --version     # prints build triple + git hash
> pcloudc doctor        # self-check probes, exit 0 when healthy
> pcloudc doctor --strict   # promote WARN to FAIL (CI / hardened hosts)
> ```
>
> When both succeed, jump to [First login](first-login.md). `pcloud-rs` is
> **pre-alpha**: Linux is the flagship target; macOS, Windows and *BSD
> packaging recipes are real, but mount surfaces on non-Linux are still
> scaffolded (see `bd-1du.4`).

## What you'll learn

- Which package channel ships which artefacts (CLI, daemon, service
  unit, man pages, FUSE provider).
- How pCloud is architected at a glance, so you can tell *why* the
  installer creates a `0700` config dir, a user-scoped daemon, and a
  mode-`0600` socket.
- The exact install commands for cargo-install, `.deb`, `.rpm`,
  Homebrew, Nix/flake, AppImage, Flatpak, Snap, Docker, Windows
  (winget / Chocolatey / Scoop / MSI), and the *BSDs.
- How to verify the install end-to-end and how to read each probe in
  `pcloudc doctor --strict`.
- The top five install failures and the one-line fix for each.

## Conceptual background

`pcloud-rs` is a **three-piece client** for the pCloud service:

1. **`pcloudc`** — a thin CLI. It parses your command, opens the local
   IPC socket, and blocks until the daemon replies. It never talks to
   the network. It never stores secrets in its own address space any
   longer than one IPC round-trip.
2. **`pcloud-daemon`** — the long-lived user-scoped service. It owns
   network I/O, the SQLite store, the optional auth-token vault, and
   the sync/mount engines. It runs as **your user**, not as root, and
   exposes its IPC socket at `~/.local/state/pcloud-rs/ipc.sock` with
   mode `0600` inside a `0700` parent directory.
3. **A native service unit** — a systemd user unit on Linux, a launchd
   agent on macOS, a Windows Service on Windows, or an `rc.d` script
   on *BSD. The unit lets your OS supervise the daemon the same way it
   supervises every other service it knows about.

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

> **Expert sidebar (FAANG-ops angle).** Treat `pcloud-daemon` like any
> other per-user sidecar: scoped to one UID, no setuid bits, no root
> capabilities, systemd `ProtectSystem=strict`, `PrivateTmp=yes`. All
> state under `$XDG_STATE_HOME/pcloud-rs`. For fleet rollouts, the
> packaging channel you pick determines your patch-cadence story:
> `.deb` / `.rpm` through an internal apt/dnf mirror gives you the
> cleanest SBOM pipeline; Nix gives you bit-for-bit reproducibility;
> AppImage / standalone `cargo install` does **not** — reserve those
> for dev boxes.

## What every install channel installs

Every channel lands the same three artefacts with the same on-disk
shape. If your package manager refuses to ship these modes, file a
bug — it is the hardening baseline, not a preference.

| Artefact | Path (Linux) | Mode |
|---|---|---|
| CLI binary | `/usr/bin/pcloudc` | `0755` |
| Daemon binary | `/usr/libexec/pcloud-rs/pcloud-daemon` | `0755` |
| Systemd user unit | `/usr/lib/systemd/user/pcloud-daemon.service` | `0644` |
| Config template | `~/.config/pcloud-rs/config.toml` (created on first run) | `0600` in `0700` |
| Runtime state | `~/.local/state/pcloud-rs/` | `0700` |
| Man pages | `pcloudc.1`, `pcloud-daemon.1`, `pcloud.conf.5` | `0644` |

See [`packaging/README.md`](https://github.com/pcloudcom/pcloud-rs/blob/main/packaging/README.md)
for the per-channel truth table. The **honest status (2026-04-16)** from
that file: Linux channels are wired end-to-end, Docker images are
cosign-signed; macOS Developer ID notarisation is **pending a valid
Apple Developer account**; Windows Authenticode EV signing is **a stub
awaiting an EV hardware token**; *BSD mount runtime is **scaffolded
only**.

## Step-by-step: pick your channel

### Build from source (`cargo install`, any platform with a Rust toolchain)

```bash
# Rust 1.80+ — matches Cargo.toml `rust-version`
rustc --version
git clone https://github.com/pcloudcom/pcloud-rs
cd pcloud-rs/
cargo build --workspace --release --locked          # 5–15 min on a laptop
sudo install -m 0755 target/release/pcloudc        /usr/local/bin/
sudo install -m 0755 target/release/pcloud-daemon  /usr/local/libexec/
```

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
Compiling pcloud-proto v0.9.0
...
Finished release [optimized] target(s) in 6m 42s
```

Common failures:

- **`error: failed to download`** — proxy / offline environment. Add
  `--frozen` and point `CARGO_HOME` at a pre-seeded vendor dir, or
  use a native package.
- **`linking with cc failed`** — missing system libs (`libssl-dev`,
  `libsqlite3-dev`, `pkg-config`, `fuse3`). On Debian:
  `sudo apt install build-essential pkg-config libssl-dev libsqlite3-dev libfuse3-dev`.

> **Expert tip.** `cargo install --path crates/pcloud-cli --locked`
> lands **only** the CLI in `~/.cargo/bin` — handy on a workstation
> where the daemon is already provided by the system package but you
> want a bleeding-edge CLI for testing. Never ship this combo to
> production; CLI and daemon must agree on the IPC protocol version.

### Debian / Ubuntu (`.deb`)

Supported: Debian 12+, Ubuntu 22.04+, x86_64 and aarch64.

```bash
curl -fsSL https://pkg.pcloud-rs.dev/gpg \
  | sudo tee /etc/apt/keyrings/pcloud-rs.asc > /dev/null
echo "deb [signed-by=/etc/apt/keyrings/pcloud-rs.asc] https://pkg.pcloud-rs.dev/deb stable main" \
  | sudo tee /etc/apt/sources.list.d/pcloud-rs.list
sudo apt update
sudo apt install pcloud-rs
```

The `pcloud-rs` meta-package pulls in `pcloud-rs-cli`, `pcloud-rs-daemon`,
the `fuse3` provider (recommended), and the man pages. The systemd
user unit is **enabled but not started** — the installer never starts
a service automatically. Start it yourself:

```bash
systemctl --user start pcloud-daemon
systemctl --user status pcloud-daemon    # expect "active (running)"
```

> **Expert tip.** On Debian images managed by `apt-daily`, pin the
> package to a specific version during a migration window:
> `apt-mark hold pcloud-rs` then bump it through your configuration
> manager. Avoid `apt install -y pcloud-rs` in a CI bootstrap without
> a pinned version — fleet version skew across a soak window makes
> parity bug triage painful.

### Fedora / RHEL / Rocky / Alma (`.rpm`)

Supported: Fedora 38+, RHEL 9+, Rocky 9+, AlmaLinux 9+.

```bash
sudo dnf config-manager --add-repo https://pkg.pcloud-rs.dev/rpm/pcloud-rs.repo
sudo dnf install pcloud-rs
rpm --checksig /var/cache/dnf/pcloud-rs-*.rpm     # expect: ... OK
```

RHEL 8 and derivatives need `dnf-plugins-core` first.

### Arch Linux (AUR)

```bash
paru -S pcloud-rs-bin        # release channel (what you want)
# or
yay -S pcloud-rs-bin
# contributors tracking main:
paru -S pcloud-rs-git
```

`pcloud-rs-bin` mirrors the upstream release tarball verbatim;
`pcloud-rs-git` rebuilds from `main` on every upgrade.

### openSUSE (Tumbleweed, Leap 15.5+)

```bash
sudo zypper addrepo https://pkg.pcloud-rs.dev/rpm/pcloud-rs.repo
sudo zypper refresh
sudo zypper install pcloud-rs
```

### Nix / NixOS

```bash
# one-off ad-hoc shell
nix shell nixpkgs#pcloud-rs

# permanent on NixOS — configuration.nix
environment.systemPackages = [ pkgs.pcloud-rs ];
services.pcloud-rs.enable    = true;           # enables the user unit

# straight from this repo (flakes)
nix profile install github:pcloudcom/pcloud-rs#pcloudc
```

The flake exposes `packages.<system>.{pcloudc,pcloud-daemon}` and a
`checks.<system>.integration` derivation that reproduces the CI smoke
test. This is the only channel that currently delivers fully
reproducible builds out of the box.

### Docker / OCI

```bash
docker pull ghcr.io/pcloudcom/pcloud-rs:stable
docker run --rm -it \
  -v "$HOME/.config/pcloud-rs:/config" \
  -v "$HOME/pCloud:/sync" \
  ghcr.io/pcloudcom/pcloud-rs:stable pcloudc --version
```

The image is **cosign-signed via keyless OIDC**. Verify before running
in production:

```bash
cosign verify ghcr.io/pcloudcom/pcloud-rs:stable \
  --certificate-identity-regexp 'github.com/pcloudcom/pcloud-rs' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

The image does **not** bundle FUSE. Container mount needs
`--cap-add SYS_ADMIN --device /dev/fuse` plus a matching host kernel —
we do not recommend it outside controlled CI. Classic `pcloudc sync-add`
works fine inside the container.

### Flatpak

```bash
flatpak install flathub dev.pcloud-rs.Pcloudcc
flatpak run dev.pcloud-rs.Pcloudcc --version
```

Flatpak's sandbox limits the paths the daemon can sync. The portal
prompt lets you grant a host directory on first `sync add`.

### AppImage (portable / rescue)

```bash
curl -fsSL -o pcloudc.AppImage \
  https://github.com/pcloudcom/pcloud-rs/releases/latest/download/pcloudc-x86_64.AppImage
chmod +x pcloudc.AppImage
./pcloudc.AppImage --version
```

T2 channel: convenient for one-shot use, not the recommended daily
install. Bundles `fuse3` statically; on older kernels without
`/dev/fuse` the mount surface degrades gracefully.

### Snap

```bash
sudo snap install pcloud-rs --classic
```

`--classic` is required because the daemon must see user-chosen paths
outside the snap confinement. Tracks: `stable` / `candidate`.

### macOS — Homebrew (recommended)

```bash
brew tap pcloudcom/pcloud-rs
brew install pcloud-rs fuse-t                # fuse-t optional unless you plan to mount
brew services start pcloud-rs
```

The Homebrew formula lays down a launchd agent at
`~/Library/LaunchAgents/dev.pcloud-rs.daemon.plist`. `fuse-t` is the
chosen FUSE provider on macOS; it uses NFSv4 under the hood and does
not require a kernel extension.

> **Honest status.** macOS mount is **scaffolded** today — the
> packaging is wired, the runtime surface is behind `bd-1du.4`. CLI,
> sync, transfers, shares, crypto, public links, and backup all work
> on macOS right now.

### macOS — direct `.pkg`

```bash
curl -fsSL -o pcloud-rs.pkg \
  https://github.com/pcloudcom/pcloud-rs/releases/latest/download/pcloud-rs-universal.pkg
sudo installer -pkg pcloud-rs.pkg -target /
```

The universal `.pkg` is arm64 + x86_64 fat. **Developer ID notarisation
is pending a valid Apple Developer account**; until it lands, Gatekeeper
may flag the package — verify the SHA-256 against the release page
before `sudo installer`.

### Windows — winget

```powershell
winget install pCloud.pcloud-rs
```

### Windows — Chocolatey

```powershell
choco install pcloud-rs
```

### Windows — Scoop

```powershell
scoop bucket add pcloud-rs https://github.com/pcloudcom/scoop-bucket
scoop install pcloud-rs
```

### Windows — MSI (WiX)

```powershell
# unattended
msiexec /i pcloud-rs-x64.msi /qn /norestart

# interactive — double-click the MSI
```

The MSI registers `pcloud-daemon` as a **per-user Windows Service** and
adds `pcloudc` to `PATH`. Uninstall via Control Panel or
`msiexec /x`.

> **Honest status.** Windows **Authenticode EV signing is a stub**
> awaiting an EV hardware token. SmartScreen may prompt until the EV
> key is in place. Windows **ProjFS / mounted-drive** surface is
> gated behind `bd-1du.4`. The rest of the surface — CLI, sync,
> transfers, shares, crypto, public links, backup — works today.

### FreeBSD / NetBSD / OpenBSD

```bash
# FreeBSD pkg
sudo pkg install pcloud-rs
# FreeBSD ports
cd /usr/ports/net/pcloud-rs && sudo make install clean

# NetBSD / OpenBSD — build from source
git clone https://github.com/pcloudcom/pcloud-rs
cd pcloud-rs/
cargo build --workspace --release --locked
sudo install -m 0755 target/release/pcloudc         /usr/local/bin/
sudo install -m 0755 target/release/pcloud-daemon   /usr/local/libexec/
```

Mount runtime on *BSD is **scaffolded only**. OpenBSD builds with the
default feature set honour `unveil(2)` / `pledge(2)` — a security
property the C client never had.

## Verification

Every platform, every channel, the same two probes:

```bash
pcloudc --version
pcloudc doctor
```

### `pcloudc --version`

```
pcloudc 0.9.x (commit abc1234, release)
```

Field-selector extraction for scripting:

```bash
pcloudc --version | awk '{ print $2 }'            # → 0.9.x
pcloudc --version | grep -oP 'commit \K[0-9a-f]+' # → abc1234
```

### `pcloudc doctor`

Typical healthy output:

```
pcloudc 0.9.x (commit abc1234)
config:     ~/.config/pcloud-rs/config.toml (mode 0600, ok)
runtime:    ~/.local/state/pcloud-rs        (mode 0700, ok)
socket:     not running (start with `pcloudc start`)
tls:        production policy = mandatory
fuse:       provider = fuse-t 1.x          (mount available)
```

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

> **Expert tip.** `pcloudc doctor --json` is the integration with your
> existing observability. The JSON schema is stable within a major
> version — pipe it into Prometheus via `node_exporter`'s `textfile`
> collector, or ingest it into your SIEM. Field `.version.commit` is
> the supply-chain anchor you want for incident response.

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
   mount. Install `fuse3` (Linux), `fuse-t` (macOS), or `fusefs-libs`
   (FreeBSD). Mount is still being wired on macOS / Windows / BSD.
5. **Windows service fails to start with error 5** — the per-user
   service cannot write to `%LOCALAPPDATA%\pcloud-rs`. Check the ACL
   on that directory; corporate images sometimes strip it. The MSI
   sets the correct permissions — re-run it with `/qn` to reapply.

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
