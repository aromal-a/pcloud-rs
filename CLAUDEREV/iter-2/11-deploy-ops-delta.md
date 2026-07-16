# Dimension 11 — Deployment & Operations — Iter-2 Delta

**Re-audit date:** 2026-04-29
**Mode:** read-only.
**Iter-1 totals:** 0 CRITICAL · 4 HIGH · 7 MEDIUM · 6 LOW.

---

## Re-verification of iter-1 HIGH findings

### DEPLOY-H-11.3 (`IPAddressDeny=any` blocks API egress) — CONFIRMED

`packaging/systemd/pcloudd.service:119-120` ships:

```
IPAddressDeny=any
IPAddressAllow=localhost
```

Followed by an inline comment (`pcloudd.service:121-122`) acknowledging
operators MUST broaden via drop-in. Three `override*.conf.example`
files ship as `*.example` (`override.conf.example`,
`override-fuse.conf.example`, `override-user.conf.example`); none is
auto-installed by `packaging/debian/postinst`. No allow-list drop-in
is present in the unit drop-in directory by default. Finding stands
unchanged.

### DEPLOY-H-11.1 (Windows MSI installs no-op service) — PARTIALLY OUTDATED

Iter-1 evidence cited `CLAUDE.md` claiming
`pcloud_daemon::serve_with_shutdown` returns `Unsupported` on Windows.
Direct read of `crates/pcloud-daemon/src/serve.rs:533-604` shows the
function is **fully wired** on all platforms: it bootstraps the runtime
shell, spawns the sync loop, spawns the health server, calls
`IpcServer::bind` (which on Windows dispatches to
`crates/pcloud-ipc/src/platform/windows.rs:286 WindowsIpc::bind_listener`
that creates a per-user-SID named pipe at `\\.\pipe\pcloud-rs-<sid>`),
and enters the cooperative serve loop with the SCM-shared `AtomicBool`.
`crates/pcloud-daemon-win/src/main.rs:218` invokes the same function
verbatim. The named-pipe `accept` path on Windows
(`platform/windows.rs:340+`) implements `CreateNamedPipeW` per-accept
with `CancelIoEx` cancellation, peer SID extraction, and overlapped IO.

Iter-1 HIGH-11.1 should be downgraded — the named-pipe accept loop
**is** wired today. CLAUDE.md is stale on this point. The remaining
caveat (no live WinFSP mount, no integration tests run on Windows) is
distinct from "service does no work". This is a delta finding worth
flagging.

---

## Inventory: `packaging/`

Total directories: 23. Classification:

| Subdir | Kind | Files |
|---|---|---|
| `apparmor/` | MAC profile (Linux) | `usr.local.bin.pcloudd` |
| `appimage/` | Linux portable bundle | `AppRun`, `build-appimage.sh`, `pcloud-rs.desktop`, `README.md` |
| `bsd/` | docs only | `README.md` |
| `chocolatey/` | Windows package manager | `pcloud-rs.nuspec`, `tools/chocolatey{install,uninstall}.ps1` |
| `debian/` | nfpm/cargo-deb spec | `nfpm.yaml`, `cargo-deb.toml`, `control`, `pcloud-rs.logrotate`, `postinst`, `postrm`, `README.md` |
| `docker/` | OCI container | `Dockerfile`, `docker-compose.yml`, `entrypoint.sh`, `README.md` |
| `flatpak/` | Linux Flatpak | `com.pcloud.pcloud-rs.{yaml,desktop,metainfo.xml}`, `README.md` |
| `freebsd/` | rc.d (top-level) | `pcloudd.rc` |
| `homebrew/` | macOS brew | `pcloud-rs.rb`, `Casks/fuse-t.rb` |
| `init/common/` | shared env + wrapper | `pcloudd.env.example`, `pcloudd-wrapper.sh` |
| `init/dinit/` | dinit unit | `pcloudd` |
| `init/freebsd/` | rc.d (init dir) | `pcloudd` |
| `init/netbsd/` | rc.d | `pcloudd` |
| `init/openbsd/` | rc.d | `pcloudd` |
| `init/openrc/` | OpenRC | `pcloudd` |
| `init/runit/` | runit | `pcloudd` |
| `init/s6/` | s6 | `pcloudd` |
| `init/sysvinit/` | SysV init | `pcloudd` |
| `macos/` | LaunchDaemon + signing/install scripts | `com.pcloud.pcloudd.plist`, `com.pcloud.pcloud-rs.plist`, `entitlements.plist`, `build-pkg.sh`, `build-dmg.sh`, `first-run.sh`, `install.sh`, `uninstall.sh`, `setup-keychain.sh`, `launchd-status.sh`, `README.md` |
| `man/` | manpages | `pcloudc.1`, `pcloudd.1`, `pcloud.conf.5`, `README.md` |
| `netbsd/` | rc.d (top-level) | `pcloudd` |
| `openbsd/` | rc.d (top-level) | `pcloudd` |
| `scoop/` | Windows scoop | `pcloud-rs.json`, `README.md` |
| `scripts/` | reproducibility | `verify-reproducibility.sh` |
| `selinux/` | MAC policy | `pcloud-rs.fc`, `pcloud-rs.te` |
| `signing/` | code signing | `notarize-macos.sh`, `sign-macos.sh`, `sign-windows.ps1`, `README.md` |
| `snap/` | Snap | `snapcraft.yaml`, `README.md` |
| `systemd/` | systemd | `pcloudd.service`, `pcloudd.socket`, `override.conf.example`, `override-fuse.conf.example`, `override-user.conf.example`, `README.md` |
| `windows/wix/` | WiX MSI | `pcloud-rs.wxs`, `License.rtf`, `README.md` |
| `winget/` | Windows winget | `pcloud-rs.yaml`, `README.md` |

**Note:** `packaging/freebsd/pcloudd.rc` and `packaging/init/freebsd/pcloudd`
both exist (likewise netbsd/openbsd at top-level and under `init/`); the
duplication is undocumented. Same goes for `packaging/bsd/README.md` which
points to no concrete artefact.

---

## Container / OCI / K8s

- **Dockerfile present** at `packaging/docker/Dockerfile` (multi-stage,
  `rust:alpine` builder → `gcr.io/distroless/static-debian12:nonroot`
  runtime, uid 65532, static-musl link, `HEALTHCHECK` invoking
  `pcloudc ping` over IPC, no shell in runtime stage).
- **docker-compose** present.
- **No Helm chart.** No `Chart.yaml` anywhere in the tree.
- **No Kustomize.** No `kustomization.yaml`.
- **No Kubernetes manifests** (no `Deployment`, `StatefulSet`, `Service`,
  `ConfigMap` YAMLs).
- **No BuildKit/OCI image build pipeline in CI.** The Dockerfile is
  hand-rolled-only; no `.github/workflows/*.yml` step builds and
  publishes the image.

---

## Configuration secret loading

- Documented mechanism (in `packaging/systemd/pcloudd.service:149-156`):
  **systemd-creds** (`LoadCredentialEncrypted=`) — explicit guidance
  not to pass secrets via `Environment=`.
- KMS providers wired in code: `crates/pcloud-config/src/crypto_kms.rs`
  with `aws-kms`, `vault-kms`, `pkcs11-kms` cargo features; selected
  via `[crypto.kms]` in profile JSON.
- Auth vault: file-mode `0600` under `0700` parent
  (`crates/pcloud-daemon/src/auth_vault.rs`); macOS Keychain backend
  available; Windows DPAPI backend implied per `auth.backend = auto`.
- No raw password persistence (per CLAUDE.md security rules and verified
  in iter-1).

---

## Health endpoints (re-verify)

`crates/pcloud-daemon/src/health_server.rs:233,237` confirms:

- `GET /livez` — 200 OK while HTTP thread alive.
- `GET /readyz` — 200 OK while daemon `Running`.
- **No `/healthz` alias.** Some K8s tooling expects this name; only
  `/livez` and `/readyz` ship.
- `/metrics` and `/health` (different surface) live in
  `metrics_server.rs`.

---

## Telemetry export config

Confirmed iter-1 finding stands. Tracing routes to:
- structured logs via `tracing-subscriber` (writer not configured to
  rolling/file appender — daemon writes to stdout/stderr; rotation is
  the operator's responsibility via journald/syslog/logrotate).
- `pcloud-rs.logrotate` uses `systemctl kill -s HUP` — see iter-1
  LOW-11.12.
- Optional OTLP HTTP exporter under `tracing-otlp` feature (not enabled
  by default).
- No structured log file path configurable; no built-in rolling-file
  appender.

---

## Auto-update

**No auto-update mechanism shipped.** Grep across `crates/` for
`auto.update`, `self.update`, `self_update`, `UpdateCheck`,
`update.check` returns zero matches. CLAUDE.md flags
"update-check declarations in this fork are ghost surfaces and should
stay `Rejected`". Confirmed: no update path = no update-channel
attack surface = no integrity-of-binary risk from a built-in updater.
Operators rely on the OS package manager (`apt`/`dnf`/`brew`/`winget`/
`choco`/`scoop`/`snap`/`flatpak`) for update delivery. This is the
correct posture; downgrade-attack risk is on the package channel, not
on the daemon.

---

## Backup hooks

- **No `ExecStop=` pre-stop drain hook** in
  `packaging/systemd/pcloudd.service`. Stop relies on
  `KillMode=mixed` + `TimeoutStopSec` + the daemon's own SIGTERM
  handler running the cooperative-shutdown loop (which does drain
  IPC connections per the comment at line 18).
- **No vault-snapshot pre-stop or pre-upgrade hook** anywhere in
  `packaging/`. The iter-1 LOW-11.17 (no documented backup script for
  the full state set) is the parent finding — no progress.
- **macOS plist** has no equivalent of `ExitTimeOut` extension for
  drain; only the existing `ExitTimeOut=30` per iter-1 matrix.
- **Windows SCM** wrapper waits up to 5 s `wait_hint` after
  `StopPending`; relies on the cooperative-shutdown flag in the daemon
  to drain IPC. No vault-snapshot pre-stop.

---

## Delta findings (new this iteration)

### DELTA-LOW-11.18 — CLAUDE.md Windows posture is stale; misled iter-1 HIGH-11.1

**Severity:** LOW (process / documentation).
**Files:** `CLAUDE.md` (Windows posture section),
`crates/pcloud-daemon-win/src/main.rs:218`,
`crates/pcloud-daemon/src/serve.rs:533`,
`crates/pcloud-ipc/src/platform/windows.rs:286-298`.

**Evidence:** CLAUDE.md states "named-pipe IPC accept-loop wiring …
in-flight". Direct source read shows the named-pipe accept loop is
fully implemented:
`WindowsIpc::bind_listener` creates a per-user-SID pipe path,
`WindowsListener::accept` issues `CreateNamedPipeW` per accept with
`CancelIoEx`-driven shutdown, and `IpcServer::bind` →
`serve_until_shutdown_with_flag` is the same code path Unix uses. The
SCM wrapper at `crates/pcloud-daemon-win/src/main.rs:218` calls
`pcloud_daemon::serve_with_shutdown` verbatim and that function is
not gated `cfg(unix)`. Iter-1 HIGH-11.1 should be reclassified to
**MEDIUM** (live WinFSP mount and `cargo test --workspace --tests` on
Windows are still the gating items, but the service is not "no-op").

**Risk:** docs/handoff drift makes auditors over-state risk.

**Remediation:** update CLAUDE.md "Windows posture" section to reflect
that the named-pipe accept loop landed; downgrade iter-1 HIGH-11.1 to
MEDIUM with revised scope (live WinFSP / integration tests only).

### DELTA-LOW-11.19 — Duplicate BSD init artefacts at two paths

**Severity:** LOW.
**Files:** `packaging/freebsd/pcloudd.rc`,
`packaging/init/freebsd/pcloudd`, same for `netbsd/`, `openbsd/`,
plus stub `packaging/bsd/README.md`.

**Evidence:** Two parallel BSD init layouts ship — `packaging/<bsd>/`
vs `packaging/init/<bsd>/` — with no documented relationship. An
operator following the README in one path may not discover the other.

**Risk:** confusion; potential for divergent rc.d scripts to drift.

**Remediation:** consolidate to `packaging/init/<bsd>/`, redirect the
top-level `packaging/{freebsd,netbsd,openbsd}/` to symlinks or remove
them, and document the canonical layout in `packaging/README.md`.

### DELTA-LOW-11.20 — No OCI image build/publish pipeline in CI

**Severity:** LOW.
**Files:** `packaging/docker/Dockerfile`,
`.github/workflows/*.yml` (no docker job grep hit in iter-1).

**Evidence:** Dockerfile is hand-rolled-only. No CI step builds, signs
(cosign), and pushes the OCI image to `ghcr.io` or a registry on tag.

**Risk:** image hash supply-chain integrity is operator-asserted, not
release-asserted. Same family of risk as iter-1 HIGH-11.2 (`.deb`/`.rpm`
not built in CI), extended to OCI.

**Remediation:** add a `docker/release.yml` workflow gated on tags,
build with BuildKit, sign with cosign keyless OIDC, push to
`ghcr.io/ezechiel203/pcloud-rs:<tag>` and `:sha-<git>`, attach SBOM via
Syft, attest provenance via SLSA generator.

### DELTA-MED-11.21 — No Helm chart / K8s manifests despite distroless image

**Severity:** MEDIUM.
**Files:** absent.

**Evidence:** The Dockerfile produces a distroless static image with
HEALTHCHECK, named volumes (`/var/lib/pcloud-rs`, `/run/pcloud-rs`),
and a documented IPC socket pattern — i.e., it is K8s-ready in
spirit. But no `Chart.yaml`, no `Deployment` YAML, no
`kustomization.yaml`, no `livenessProbe`/`readinessProbe` snippets
referencing `/livez` / `/readyz`.

**Risk:** Operators wanting to run on K8s must compose the manifests
themselves and may misconfigure security context (the Dockerfile
demands `runAsUser: 65532`, read-only root, FUSE
`securityContext.privileged` decision, mount-propagation for FUSE).

**Remediation:** Ship a minimal Helm chart at `packaging/helm/pcloud-rs/`
with values for: storage (PVC for `/var/lib/pcloud-rs`),
`securityContext.runAsNonRoot=true`, `runAsUser=65532`,
`readOnlyRootFilesystem=true`, `livenessProbe.httpGet=/livez`,
`readinessProbe.httpGet=/readyz`, and a documented stance on the FUSE
mount-propagation question (probably "do not run mount inside the
pod; provision via DaemonSet on the host"). Add a sister Kustomize
overlay.

---

## Convergence signal

**Iter-2 found 4 new findings** (3 LOW, 1 MEDIUM) plus a re-classification
of an iter-1 HIGH down to MEDIUM scope. Not converged. A third
iteration would likely yield diminishing returns; recommend stopping
after iter-3 unless new architectural changes ship.

**Iter-2 totals (delta only):** 0 CRITICAL · 0 HIGH · 1 MEDIUM · 3 LOW
(plus 1 iter-1 HIGH downgrade recommendation).

End of delta report.
