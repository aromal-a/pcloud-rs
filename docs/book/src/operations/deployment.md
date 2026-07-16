# Deployment

## 1. Purpose

This chapter is the topology-and-fleet reference for rolling out
`pcloud-daemon` and `pcloud-cli`. It is the **1000-seat checklist**;
scope it down for smaller rollouts but do not skip the supply-chain or
telemetry sections. Each topology (single-host, multi-host,
container, Kubernetes, reverse-proxied web UI) documents:

- network assumptions,
- secret storage,
- upgrade strategy,
- backup strategy,
- incident response.

> **Honesty callout.** The Rust path still has open parity gaps
> (`bd-1du.4` mounted-drive, `bd-1du.10` final parity gate; see
> [`STATUS.md`](../../../../STATUS.md)). Do not claim "production ready"
> unless the retained matrix rows apply to your deployment.

## 2. Prereqs

- **Fleet inventory** in CSV:
  `hostname,os,arch,init,fuse,legacy_version` — one row per seat.
- A **pinned release** with sha256, signature (cosign or GPG), and
  release-note URL. Never deploy `latest`.
- A **config repository** with signed commits that holds
  `config.json` templates per profile (`staging`, `production`).
- A **fleet tool** (Ansible, Chef, Puppet, Salt, Intune, Jamf,
  Workspace ONE) able to push files, restart services, and verify
  sha256 before activation.
- SIEM + alerting set up to ingest the daemon’s structured JSON logs
  (see [runbook.md](./runbook.md#log-analysis-guide)).

## 3. Conceptual background

### What the daemon provides, and why topology matters

`pcloud-daemon` is a **long-lived local process** that owns an on-disk
store (SQLite), a UID-bound auth vault, an append-only audit log, and
optionally a FUSE/WinFSP/fuse-t mount. It talks to the pCloud API over
TLS and exposes a **local, owner-only, Unix-domain IPC socket** to
`pcloud-cli` / `pcloud-web`. The socket is never network-reachable by
design; all multi-user access must go through a reverse proxy that
terminates authentication and forwards to `127.0.0.1:<port>`.

### Threat model boundaries (short form)

- **Host UID** is the security boundary. A process running as the
  daemon’s UID is inside the trust envelope.
- **Vault** is confidential; its mode (`0600`) and parent directory
  mode (`0700`) are **enforced at open time**. Do not loosen them.
- **IPC socket** inherits the same rule: `0600`, owner-only peer.
- **TLS is mandatory** for the `production` config profile. The config
  loader refuses downgrades by design — **do not patch around it**.

### Config-management split

- **Fleet-managed**: API base URL, log level, log format, telemetry
  opt-in flag, vault persistence toggle, mount policy, allowed
  regions.
- **User-mutable**: sync root list, mount point, UI preferences.

## 4. Step-by-step procedure

### 4.1 Phase 0 — Pre-flight (two weeks before rollout)

1. **Inventory the fleet** (CSV above).
2. **Pick the release** and record its sha256 and release-notes URL.
3. **Stage a canary pool** (1–2% of seats) across every
   OS/arch/init/FUSE combination. At least one representative per
   FUSE runtime (FUSE3, fuse-t, WinFSP, *BSD fusefs).
4. **Dry-run the upgrade** on a staging host using
   [Upgrade](./upgrade.md), including `pcloudc migrate-from-c` if any
   legacy C clients are in scope.
5. **Freeze the config schema** — document the fleet-managed vs
   user-mutable split in your internal runbook.

### 4.2 Phase 1 — Supply-chain gate

Every artefact leaving the staging repo MUST pass:

1. **Source pin** — build from a signed Git tag; record commit SHA.
2. **Reproducible build** — clean container; record builder identity,
   source URI, source SHA, and build command. Current public workflows do
   not emit SLSA provenance.
3. **sha256 manifest** — publish checksum files and sign the artefacts the
   workflow actually signs. Current raw binary releases use cosign blob
   signatures; package `SHA256SUMS` is not signed yet.
4. **Dependency audit** — `cargo audit` and `cargo deny check`; fail
   on any unresolved RUSTSEC advisory.
5. **License audit** — `cargo deny check licenses` against the
   allowlist.
6. **Secret scan** — scan build output and config templates for
   embedded credentials.
7. **Binary transparency** — publish to a transparency log (Sigstore
   Rekor or equivalent).

### 4.3 Phase 2 — Canary wave (day 0, 1–2%)

```bash
# Example: push via Ansible
ansible-playbook -i inventory/canary.ini roll-pcloud-rs.yml \
  -e pcloud-rs_version=X.Y.Z \
  -e pcloud-rs_sha256=<pinned-hash>
```

Expected per-host output (parse with JSON selectors):

```bash
pcloudc version --json | jq '.daemon'      # "X.Y.Z"
pcloudc doctor --json   | jq '.checks[] | select(.level=="error")'
# (empty)
```

Hold 48 hours with zero P0 incidents before advancing. If a P0 fires,
freeze the wave, execute the cohort rollback per
[runbook Playbook 3](./runbook.md#playbook-3-rollback), and file a
bead before re-attempting.

### 4.4 Phase 3 — Broad waves (Wave A 20% / Wave B 78%)

Run in two sub-waves as per [Upgrade §4.1](./upgrade.md). Each wave
runs per-host:

```bash
pcloudc doctor --json > /var/log/pcloud-rs/post-upgrade-$(date +%s).json
pcloudc --json status \
  | jq '{auth: .auth.state, sync: .sync.root_count, mount: .mount.state}'
```

Any host where `doctor` reports a failing check is pulled out of the
wave and triaged individually.

### 4.5 Phase 4 — Close-out

```bash
pcloudc version --json | jq '.daemon'     # matches the pinned version
```

Reconcile against the Phase 0 inventory. Missing hosts are
investigated, not ignored.

## 5. Verification per topology

### 5.1 Single-host install (workstation / server)

- **Network assumptions**: outbound TLS to `*.pcloud.com`; local IPC
  socket under `$XDG_RUNTIME_DIR/pcloud-rs/daemon.sock`.
- **Secret storage**: UID-bound vault; opt-in token persistence.
- **Upgrade**: [Upgrade §4](./upgrade.md).
- **Backup**: nightly `pcloudc backup snapshot-create` to an offsite
  destination (see [Backup snapshots](./backup-snapshots.md)).
- **Incident response**: [Runbook playbooks](./runbook.md).

Verify:

```bash
pcloudc doctor            # full health bundle
pcloudc status            # auth=, sync=, crypto=, engine summary
curl --unix-socket $XDG_RUNTIME_DIR/pcloud-rs/daemon.sock \
  http://localhost/health
```

### 5.2 Multi-host fleet (mTLS agent integration)

- **Network assumptions**: each host runs its own daemon; the fleet
  tool’s agent (Chef client, Puppet agent, Salt minion) carries
  mTLS to its control plane. The daemon is **not** remote-callable —
  the agent drives `pcloudc` locally.
- **Secret storage**: the agent never sees vault material; credentials
  are entered interactively on first login or provisioned via
  `pcloudc login` driven from a console jump host.
- **Upgrade**: 2-wave rolling per [Upgrade §4.1](./upgrade.md).
- **Backup**: fleet-wide GFS schedule; see
  [Backup snapshots](./backup-snapshots.md).
- **Incident response**: aggregate SIEM; `ipc.peer.rejected` on any
  non-shared host is a SEV-2 until proven otherwise.

### 5.3 Container deployment (Docker / systemd-nspawn)

- **Network assumptions**: container hosts exactly one daemon; the
  IPC socket lives in a tmpfs mount scoped to the container. TLS
  egress requires the container image to ship an up-to-date CA
  bundle.
- **Secret storage**: mount the vault path from a tmpfs **or** an
  encrypted volume with host-side key management. Never bake vault
  material into the image.
- **systemd-nspawn caveats**: FUSE inside a container requires
  `--capability=CAP_SYS_ADMIN` and `--bind=/dev/fuse`; on hosts with
  an opinionated seccomp profile the `mount` syscall may still be
  blocked — coordinate with the platform team.
- **Upgrade**: rebuild the image, push via your registry, roll
  containers one at a time; the in-image `entrypoint` must honour
  `SIGTERM` to drain gracefully.

Docker recipe skeleton (signing posture applies):

```Dockerfile
# See packaging/docker/ for the canonical Dockerfile.
FROM debian:stable-slim
RUN useradd -m -u 10001 pcloud-rs
COPY --chown=pcloud-rs:pcloud-rs pcloud-daemon /usr/local/bin/
USER pcloud-rs
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/pcloud-daemon"]
```

### 5.4 Kubernetes ergonomics

Honest status: **no Helm chart ships today**. Operators roll their
own `StatefulSet` with:

- one replica per user (the daemon is single-writer against its
  store),
- a `PersistentVolumeClaim` for `~/.config/pcloud-rs/` and
  `~/.local/share/pcloud-rs/`,
- an `initContainer` that runs `cosign verify-blob` against the
  daemon binary,
- a `preStop` hook calling `pcloudc backup snapshot-create` before
  `SIGTERM`,
- a `readinessProbe` hitting `/health` via an `emptyDir` tmpfs where
  the IPC socket lives.

Exposing the web UI to multiple operators requires a reverse-proxy
sidecar — see 5.5.

### 5.5 Reverse-proxy for the web UI

- **Network assumptions**: `pcloud-web` binds **loopback only** and
  panics on non-loopback bind (ADR 0004). Multi-operator access must
  go through a same-host proxy.
- **Secret storage**: proxy terminates TLS; upstream is plain HTTP on
  `127.0.0.1`.
- **Upgrade**: update proxy + daemon together inside a maintenance
  window.
- **Backup**: the web UI is stateless; back up the daemon, not the
  proxy.
- **Incident response**: check the proxy access log and the daemon
  `ipc.peer.rejected` counter.

See the [web UI reverse-proxy recipe](./web-ui.md#reverse-proxy-recipes)
for nginx, Caddy, and OIDC-broker examples.

### 5.6 Observability — Prometheus scraping & OTel exporter

- **Prometheus** — the daemon exposes `/slo` and a feature-gated
  `/metrics` endpoint on the same local IPC socket. Operate a
  **local on-host exporter** that reads the socket and re-publishes
  over `127.0.0.1:<prom-port>`; scrape that. **Do not** weaken
  socket permissions to allow remote scraping.
- **OpenTelemetry** — the daemon has hooks for an OTel exporter in
  `pcloud-observability`. Configure via:

  ```toml
  [otel]
  enabled       = true
  endpoint      = "https://otel-collector.internal:4317"
  protocol      = "grpc"
  client_cert   = "/etc/pcloud-rs/keys/otel.crt"
  client_key    = "/etc/pcloud-rs/keys/otel.key"
  trust_bundle  = "/etc/pcloud-rs/keys/otel-ca.pem"
  ```

  The exporter shares the daemon’s TLS policy (no plaintext in
  `production`).

## 6. Rollback (fleet-level)

Every rollout has an implicit rollback target: the previous release.
Pre-requirements:

- keep the previous package available in your artefact repo for at
  least two upgrade cycles,
- keep the previous `SHA256SUMS.prev.txt` and signature,
- keep the previous `config.json` template tagged in the config repo.

Procedure:

1. Freeze the active wave (stop rolling).
2. Execute [Runbook Playbook 3](./runbook.md#playbook-3-rollback) on
   affected seats.
3. Re-verify `pcloudc version --json` matches the previous pinned
   version on every rolled-back seat.
4. File a bead; hold the next wave attempt until the bead is
   resolved.

## 7. Tradeoffs / tuning

| Knob                             | Default               | Tradeoff                                                        |
|----------------------------------|-----------------------|-----------------------------------------------------------------|
| Wave hold duration               | 48 h / 48 h / 72 h    | Shorter accelerates delivery, misses diurnal regressions.       |
| Canary verbose logging window    | 24 h                  | Verbose logs pressure SIEM ingest budgets.                      |
| Telemetry retention              | 90 d                  | Longer aids long-tail triage but grows privacy surface.         |
| Vault token persistence          | opt-in                | Enabling removes re-login friction; increases blast radius.     |
| Prometheus scrape interval       | 15 s                  | Lower interval increases cardinality cost in the TSDB.          |
| OTel sampling rate               | 1.0 (all)             | Sample < 1.0 to cap exporter bandwidth on large fleets.         |

## 8. Common failure modes

1. **EDR scan storms on FUSE mounts.**
   - Symptom: CPU pegged, high EIO rate.
   - Cause: real-time scanner sweeping every `readdir` on the mount.
   - Fix: document FUSE mount path exclusion in the EDR policy; AV /
     EDR allowlist the daemon binary **by sha256**, not by path.

2. **`auth.login.failed` spike post-rollout.**
   - Symptom: alerting fires across many seats simultaneously.
   - Cause: TFA enrollment drift, incorrect API endpoint in the new
     config template.
   - Fix: roll back config only (not binary) if TLS policy is intact;
     otherwise execute Playbook 3.

3. **Prometheus scrape returns 404.**
   - Symptom: `/metrics` returns 404 from the daemon.
   - Cause: daemon was built without the `metrics` feature.
   - Fix: rebuild with `--features metrics`; verify via
     `pcloudc doctor --json | jq '.build.features'`.

4. **systemd-nspawn mount fails with `EPERM`.**
   - Symptom: FUSE mount refuses to start inside the container.
   - Cause: seccomp blocking `mount`; missing `CAP_SYS_ADMIN`.
   - Fix: add `Capability=CAP_SYS_ADMIN` and `Bind=/dev/fuse` to the
     nspawn unit; accept the increased privilege cost or move to a
     VM.

5. **OTel exporter silently drops spans.**
   - Symptom: traces appear only sporadically in the collector.
   - Cause: TLS handshake failing at the collector (cert not trusted),
     spans dropped by exporter queue.
   - Fix: `pcloudc doctor --json | jq '.checks[] | select(.id=="otel")'`;
     restore the trust bundle; verify the collector endpoint from the
     host (`curl --cacert`).

## 9. Security / compliance notes

- **Never ship pre-populated vaults.** Each user authenticates
  interactively on first run. The vault is UID-bound and refuses
  cross-UID restores by design.
- **Headless accounts** (service accounts): provision a dedicated
  pCloud user and drive TFA from a secure interactive channel
  (console jump host). Do not embed tokens in images.
- **EDR / AV allowlist by sha256**, not by path — path-based
  allowlists break under every upgrade.
- **Windows WinFSP** — the kernel driver must be trusted by the
  endpoint-protection allowlist; coordinate with the driver
  allowlist owner before rollout.
- **Production transport** — TLS is mandatory. The loader rejects
  downgrade. Any plaintext endpoint override is rejected at config
  load time.
- **Telemetry opt-in** — see §9.1.
- **Audit retention** — retain the audit chain for the length of your
  compliance regime; the tail hash is covered by snapshot backups.

### 9.1 Telemetry opt-in (detailed)

The daemon collects **no telemetry by default**. When opted in:

- no user file paths, filenames, or sync-root contents are
  transmitted,
- no secrets, tokens, or vault contents are transmitted,
- no remote pCloud user identifiers beyond what is required for
  aggregate crash triage.

Enabling fleet-wide:

1. Document the data classes in your privacy notice.
2. Set in the fleet-managed profile:

   ```toml
   [telemetry]
   enabled  = true
   endpoint = "https://telemetry.internal.example.com"
   ```

3. Provide an on-host opt-out (user-exercisable, no admin rights).
4. Retain collected telemetry ≤ 90 d by default.
5. Surface telemetry state in `pcloudc --json status`.

GDPR / UK GDPR / CPRA jurisdictions: treat the in-CLI toggle as the
consent mechanism; record the consent event server-side. **Do not**
collect from users who have not consented.

## 10. Capacity planning

- **Per-seat steady state**: ~50 MiB RSS idle, ~200 MiB with an
  active large mount. Budget 1 GiB RSS headroom for > 100k synced
  files.
- **Page cache** (`~/.cache/pcloud-rs/`): disposable; size for the
  user’s largest expected working set, not full quota.
- **SQLite store**: ~200 bytes per synced file.
- **Audit log**: append-only; rotate / snapshot during backup, not in
  flight.

## 11. Cross-references

- [Upgrade](./upgrade.md) — per-host 2-wave procedure.
- [Runbook](./runbook.md) — live playbooks.
- [Backup snapshots](./backup-snapshots.md) — DR and GFS retention.
- [Web UI](./web-ui.md) — reverse-proxy recipes.
- [Packaging matrix](./packaging-matrix.md) — install paths and
  service-manager entries.
- [CLI reference](../reference/cli.md).
- [Config reference](../reference/config.md).
- [Prometheus reference](../reference/metrics.md).
