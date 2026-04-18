> **Pre-alpha scaffold — not live / not production-verified.** This document
> describes design and unit-tested code that has not been validated against a
> real production deployment. Do not treat it as a shippable capability.
> See `CLAUDE.md` and `docs/enterprise/README.md` for the honesty rules.

# High Availability for pcloud-rs

> **Status:** Tier 1 **landed** (per-UID isolation via XDG + `SO_PEERCRED`).
> Tier 2 **landed** as of 2026-04-16 — active-passive file-lock
> handoff via `<state_dir>/daemon.lease` (`flock(LOCK_EX | LOCK_NB)`),
> config block `[ha]` (`enabled`, `mode = "refuse" | "passive"`,
> heartbeat / poll cadence), IPC probe `Method::HaStatus`, CLI
> surface `pcloudc ha status`, and integration tests
> `ha_two_daemon_contention.rs`. Source:
> `crates/pcloud-daemon/src/ha_lease.rs`,
> `crates/pcloud-config/src/ha.rs`. Tier 3 (Windows SCM),
> Tier 4 (nginx Web UI front-door), and the legacy systemd
> `failover_restart.rs` test remain **design-only** — do not cite
> this document as evidence of those shipped.

Audience: Enterprise IT, SRE, packagers.
Scope: `pcloudd` daemon, optional Web UI, Windows SCM wrapper.

## 1. Purpose

pcloud-rs is a **per-user sync client**, not a shared service.
Classical active-active HA — redundant nodes behind a load
balancer, shared state, quorum election — is the wrong model.
Applying it would be cargo-culting patterns from distributed
databases onto a single-user desktop agent.

This document defines HA in terms appropriate to the actual
workload:

1. **Tier 1 — Per-UID isolation.** Multiple `pcloudd` instances
   coexist safely on the same host, one per logical user.
   Already enforced.
2. **Tier 2 — Service fail-over (Linux).** A single user's
   daemon recovers automatically from crashes in <5s without
   losing in-flight upload or mount state.
3. **Tier 3 — Windows SCM auto-restart.** Same as Tier 2 in
   Windows vocabulary.
4. **Tier 4 — Web UI front-door.** Optional reverse-proxy recipe
   so a shared host can expose N users' Web UIs behind one TLS
   listener.

## 2. Threat model

| Threat | Tier / Mitigation |
| --- | --- |
| One user reads another user's sync state | Tier 1: XDG dirs 0700, vault 0600, UID-checked, `SO_PEERCRED` on IPC |
| One user escalates through shared IPC socket | Tier 1: IPC socket mode 0600 per UID, `SO_PEERCRED` at accept |
| Daemon crash loses in-flight uploads | Tier 2: writeback journal replay (`bd-1du.4` P1.2) |
| Daemon crash leaves stale FUSE mount | Tier 2/3: `/proc/self/mountinfo` sniff + `umount -l` + remount |
| Crash-loop amplifies a bad state | Tier 2: `StartLimitBurst=5/60s` + monitoring alert |
| Dual-boot user syncs same root from two OSes | §7: `.pcloud/.lock` refuses second mount until stale lock cleared |
| TLS termination of Web UI on raw TCP | Tier 4: Unix-socket upstream only; TLS always at nginx |
| Multiple users' Web UIs sharing one port without auth | Tier 4: `auth_request` SSO/OIDC in front; UID-scoped socket routing |
| Single misbehaving user starves the host | §5: `MemoryMax=512M`, `TasksMax=256` per user unit |
| Operator installs as system unit (one daemon, all users) | Explicit non-goal (§6); `packaging/check.sh` lint flags it |

Explicit non-threats: cross-host data loss, cross-host fail-over,
active-active replication. See §6.

## 3. Scope

In scope, shipped as enforced invariants:

- Tier 1 isolation (already end-to-end enforced).

In scope, documented and shipped as config but **not yet
test-verified**:

- Tier 2 systemd restart policy on Linux,
- Tier 3 SCM failure actions on Windows,
- `.pcloud/.lock` sync-root ownership lock,
- Tier 4 nginx recipe.

Out of scope, permanent non-goals (§6):

- one daemon serving multiple users,
- cross-host fail-over,
- active-active clustering,
- fleet management (`bd-B3`, separate page).

## 4. Design

### 4.1 Tier 1 — Per-UID isolation

Each UID on a host runs its own fully independent `pcloudd`. No
shared process, socket, store, or vault. Enforcement table:

| Resource | Path / mechanism | Enforcement |
| --- | --- | --- |
| IPC socket | `$XDG_RUNTIME_DIR/pcloudd.sock` (mode 0600) | `SO_PEERCRED` + owner-only mode |
| SQLite store | `$XDG_DATA_HOME/pcloud-rs/store.db` | Dir 0700, file 0600 |
| Auth vault | `$XDG_DATA_HOME/pcloud-rs/vault.json` | Dir 0700, file 0600, UID-checked |
| Audit log | `$XDG_STATE_HOME/pcloud-rs/audit.log` | Dir 0700, append-only per UID |
| Config | `$XDG_CONFIG_HOME/pcloud-rs/config.toml` | Dir 0700 |
| Mount point | User-owned path, checked at mount time | `user_allow_other` not required |

**Formal guarantee:** two users cannot read, write, or influence
each other's pcloudd state through any daemon-managed path. A
user cannot escalate to another user's session via the IPC
socket — `SO_PEERCRED` at accept time rejects any peer whose
effective UID differs from the daemon's own.

**Packaging invariant:** distro packages must ship
`pcloudd.service` as a **user unit**, not a system unit.
Installing as a system unit violates the isolation boundary and
is flagged by the packaging lint `packaging/check.sh`.

### 4.2 Tier 2 — Active-passive handoff + systemd restart

> **Landed 2026-04-16.** Implementation:
> `crates/pcloud-daemon/src/ha_lease.rs` +
> `crates/pcloud-config/src/ha.rs`. Tests:
> `crates/pcloud-daemon/tests/ha_two_daemon_contention.rs`
> (5 cases: primary acquires, refuse-mode blocks second daemon
> with a named-primary diagnostic, passive-mode rejects
> non-probe requests, takeover-after-release promotes a fresh
> bootstrap, and `HaRuntime::Disabled` is the default). The
> passive poll loop and end-to-end systemd `failover_restart.rs`
> integration test remain tracked under `bd-1du.10`.

#### 4.2.1 File-lock lease (`<state_dir>/daemon.lease`)

When `[ha].enabled = true` the daemon tries a non-blocking
`flock(LOCK_EX | LOCK_NB)` on `<state_dir>/daemon.lease` during
bootstrap. The file is mode `0600`; the parent directory is
already provisioned `0700`. Lease metadata (hostname, pid,
`start_ts_unix`, `instance_id`, `last_heartbeat_unix`) is
re-written on every 30s heartbeat so observers can see a rolling
liveness signal. The kernel releases the `flock` automatically
on process exit, so a crashed primary is trivially recoverable.

Configuration lives under `[ha]`:

```toml
[ha]
enabled = true
# refuse  - fail bootstrap with a named-primary diagnostic
# passive - bind IPC, reject non-probe requests with Unavailable
mode = "passive"
heartbeat_interval_secs = 30
passive_poll_interval_secs = 10
```

Defaults are backwards-compatible: `[ha].enabled = false` makes
the daemon behave identically to the pre-HA model.

#### 4.2.2 Passive-mode behaviour

A secondary daemon with `mode = "passive"` **still binds its IPC
socket** so supervisors and the CLI probe reach it, but every
non-probe request is answered with
`ResponseStatus::Unavailable` and a message that names the
primary:

```
this daemon is passive; primary is workstation-7/pid=12345
  (age=12s, instance=/var/lib/pcloud/state)
```

Requests that stay available in passive mode:

- `Method::HaStatus` (status probe)
- `Method::GetHealth`, `Method::Health` (supervisor probes)
- `Method::Shutdown` (operator-initiated graceful stop)

All other variants are rejected. Operators can inspect posture
via `pcloudc ha status`, which returns the
`HaStatusPayload` JSON `{mode, lease_owner, lease_age_s,
lease_path}` — `mode` is one of `"disabled" | "primary" |
"passive"`.

#### 4.2.3 systemd restart policy

User unit ships with:

```ini
[Service]
Restart=on-failure
RestartSec=2s
StartLimitIntervalSec=60
StartLimitBurst=5
TimeoutStopSec=10
```

Clean `SIGTERM` (user-initiated stop) does **not** trigger
restart. A crash, OOM, or non-zero exit does. The burst limit
prevents crash-loop amplification; if tripped, systemd refuses
further restarts and the §9 monitoring alert fires.

State recovery on restart:

- **Auth:** token reloaded from the vault. No user interaction.
- **Sync roots:** reloaded from `store.db`, re-validated against
  the server.
- **In-flight uploads:** writeback journal replayed. Chunks
  whose `upload_save` succeeded pre-crash are not re-uploaded;
  chunks whose `upload_write` partially completed resume from
  the last confirmed offset.
- **Mount:** stale FUSE mount detected via
  `/proc/self/mountinfo`, lazily unmounted (`umount -l`), then
  remounted. Open file handles held by user processes receive
  `EIO` on next I/O — the same failure mode as a network blip.

### 4.3 Tier 3 — Windows SCM auto-restart

MSI configures failure actions at install:

```
sc failure pcloudd reset= 300 actions= restart/60000/restart/60000/restart/60000
sc failureflag pcloudd 1
```

- `reset= 300`: failure counter resets after 300s healthy
  runtime.
- Three consecutive restart actions, 60s apart. After the third,
  SCM stops attempting and logs Event ID 7034; §9 monitoring
  covers this.
- `failureflag 1`: treat any non-zero exit as failure, matching
  Linux `Restart=on-failure` semantics.

State recovery identical to Tier 2. Windows journal lives under
`%LOCALAPPDATA%\pcloud-rs\journal\` with ACLs restricted to the
owning user SID.

### 4.4 Active-passive across hosts? No.

Two pcloudd instances on two hosts syncing the same pCloud
account to the same local path would corrupt state. The pCloud
**service** is HA; the **client** is not meant to be clustered.
Quorum, leader election, and shared-state fail-over are all
non-goals — they would require a privilege boundary and
shared-state architecture the current design does not have. See
§6.

## 5. Interfaces

### 5.1 systemd user unit

Shipped under `packaging/systemd/user/pcloudd.service`. Key
directives covered in §4.2. Operators override with a drop-in
under `~/.config/systemd/user/pcloudd.service.d/override.conf`.

### 5.2 Windows service

Installed by MSI; failure actions per §4.3. Unregistered via
`sc delete pcloudd` during uninstall.

### 5.3 Sync-root ownership lock

`<root>/.pcloud/.lock` created with `O_EXCL | O_CLOEXEC`, holding
an `flock(LOCK_EX | LOCK_NB)` for the daemon's lifetime.
Contents:

```
pid=12345
uid=1001
hostname=workstation-7
started_at=2026-04-15T10:12:33Z
```

Any second daemon attempting to register the same local root
calls `flock(LOCK_EX | LOCK_NB)` first, receives `EWOULDBLOCK`,
reads the lock file, and fails `sync-add` with a structured
error naming the holder. This is the dual-boot safety net: a
user who dual-boots into a second OS on the same disk and opens
the same sync root is **refused** until the stale lock is
manually cleared.

Stale lock recovery: if the recorded PID is dead **and** the
hostname matches, the lock is auto-reaped. Cross-hostname stale
locks require manual intervention, by design.

### 5.4 Shared-host multi-user properties

- **No cross-user state** (verified by Tier 1 isolation).
- **No cross-user audit.** Each user's audit log is confined to
  their XDG state dir. Admins who need cross-user audit must
  aggregate at the OS layer (e.g. ship auditd records, not
  pcloud-rs's own log).
- **Independent update/rollback.** A user can downgrade without
  affecting others (binary is in a system path; state is
  per-user).
- **Resource bounds.** `MemoryMax=512M`, `TasksMax=256` per user
  unit.

### 5.5 Web UI front-door (Tier 4, optional nginx recipe)

Each user's pcloudd exposes its Web UI on a UID-scoped Unix
socket in `$XDG_RUNTIME_DIR`, **never on a TCP port**. nginx
terminates TLS, applies SSO/OIDC via `auth_request`, then routes
to the right socket:

```nginx
map $http_x_pcloud_user $pcloud_socket {
    default                "/run/user/1000/pcloudd-ui.sock";
    "alice"                "/run/user/1001/pcloudd-ui.sock";
    "bob"                  "/run/user/1002/pcloudd-ui.sock";
}

server {
    listen 443 ssl http2;
    server_name pcloud.internal.example.com;

    ssl_certificate     /etc/ssl/pcloud.pem;
    ssl_certificate_key /etc/ssl/pcloud.key;

    auth_request /_authz;

    location / {
        proxy_pass http://unix:$pcloud_socket:;
        proxy_set_header X-Forwarded-User $remote_user;
    }
}
```

pcloud-rs **does not bundle** nginx, does not configure it, does
not manage its lifecycle. This is a **recipe**, not a product.
Reference topology in `docs/enterprise/lab/`.

## 6. Configuration

HA behaviour is not primarily config-driven; it is infrastructure
and packaging shape. The relevant levers are:

- packaging mode (user unit vs system unit — must be user),
- `Restart=`, `RestartSec=`, `StartLimit*` in the systemd unit,
- `sc failure` actions on Windows,
- nginx vhost config for Tier 4,
- `MemoryMax=`, `TasksMax=` per user unit on shared hosts.

Explicit non-goals, **permanently out of scope**:

- **Single daemon, multiple users.** Would require a privilege
  boundary — capability tokens, per-RPC UID tagging, mandatory
  encrypted per-user stores inside one process. None of these
  exist today. Adding them would be a rewrite, not a feature.
- **Automatic fail-over to a different host.** pcloud-rs's state
  is inherently local (mount point, FUSE kernel handle, sync
  roots tied to local paths). Moving it to another host is a
  migration, not a fail-over.
- **Active-active across hosts.** Single-writer by design.

Fleet management (central policy push, inventory, remote
kill-switch) is tracked separately under `bd-B3` and is
complementary to, not part of, this document.

## 7. Onboarding

**Minimal operator checklist:**

1. Install pcloud-rs with the **user unit** (never system). On
   Debian/Ubuntu, the packaging lint `packaging/check.sh` will
   refuse to package a system unit.
2. Enable the user unit per-user:
   `systemctl --user enable --now pcloudd.service`.
3. On shared hosts, verify `MemoryMax=512M` and `TasksMax=256`
   are set. Override via a drop-in if not.
4. Confirm `$XDG_RUNTIME_DIR` lives on a user-owned tmpfs.
5. Ship the §9 Prometheus alerts.
6. Never expose pcloudd Web UI on raw TCP — always via the §5.5
   nginx recipe.
7. Investigate every `PCloudSyncRootLockContention` alert as a
   potential dual-sync event.

## 8. Verification

Tier 1 isolation is verified today by code review and by the
existing XDG / `SO_PEERCRED` unit tests.

Tier 2 / Tier 3 pass criteria (intended; enforced by
`failover_restart.rs`, **not yet landed** — tracked under
`bd-1du.10`):

1. systemd respawns pcloudd in <5s p99 on reference hardware.
2. IPC socket accepts within 2s of process start.
3. No journaled upload is lost (verified by hashing source and
   remote object after recovery).
4. Mount point, if previously active, is remounted within 10s.
5. No secret is written to stderr, stdout, or the journal during
   recovery.

Until that test lands, Tier 2 is *designed*, not *demonstrated*.

## 9. Failure modes

| Failure | Behaviour |
| --- | --- |
| Clean `SIGTERM` (user stop) | No restart; `Restart=on-failure` excludes clean exits |
| Crash (SIGSEGV, SIGKILL, OOM) | Restart per Tier 2/3 policy |
| Crash loop (>5 in 60s) | systemd refuses further restarts; `PCloudDaemonRestartStorm` alert fires |
| Stale FUSE mount after crash | Detected via `/proc/self/mountinfo`; `umount -l` + remount |
| Dual-sync attempt on same root | Second daemon refused via `flock`; structured error names the holder |
| Dead-PID, same-host stale lock | Auto-reaped on next `sync-add` |
| Cross-hostname stale lock (dual-boot) | Manual intervention required — intentional |
| Web UI reached on raw TCP | Misconfiguration; pcloudd does not open TCP listeners |
| One user starves host memory | `MemoryMax=512M` slices the per-user unit; other users unaffected |

## 10. Honest limitations

pre-alpha reality check:

- **Design-only in parts.** Tier 1 is enforced; Tier 2/3 policy
  is shipped as packaging config but **not yet test-verified**
  in-tree. `failover_restart.rs` is tracked but unshipped.
- **Nginx recipe is a recipe**, not a bundled product. Operators
  run nginx on their own.
- **Dual-boot stale lock is intentionally refused.** The safe
  default is "require human review"; there is no heuristic that
  can distinguish a stale lock from a live lock across
  hostnames.
- **No cross-host anything.** By design. Do not try to engineer
  around it.
- **Monitoring rules are guidance**, not shipped into a canonical
  Prometheus rules directory — operators drop them in by hand.

## 11. Extension points

- **Custom restart policy** — override via systemd drop-in
  without modifying the packaged unit.
- **Aggregated audit across users** — ship auditd records at the
  OS layer; pcloud-rs does not provide cross-user audit.
- **Alternate fronting** — nginx is a recipe; Caddy or Traefik
  with a matching `map`+`proxy_pass` works too.
- **Capacity classes** — bump `MemoryMax` on dedicated hosts;
  lower `TasksMax` on locked-down hosts.

## 12. Cross-refs

Code:

- `packaging/systemd/user/pcloudd.service` — Tier 2 unit.
- `packaging/windows/msi/` — Tier 3 failure actions.
- `packaging/check.sh` — packaging lint (user-unit only).
- `crates/pcloud-daemon/src/sync_backend.rs` — `.pcloud/.lock`
  handling.
- `crates/pcloud-daemon/tests/failover_restart.rs` — pass-criteria
  test (tracked, not yet landed).

Related docs:

- `docs/enterprise/fleet.md` — central policy push (distinct
  track, `bd-B3`).
- `docs/enterprise/disaster-recovery.md` — snapshot/restore that
  Tier 2/3 assume as the last line of defence.
- `docs/enterprise/tracing.md` — restart paths preserve ambient
  thread traceparent when tracing is enabled.
- `docs/enterprise/data-residency.md` — evaluator runs on every
  post-restart sync-root re-validation.
- `CLAUDE.md` §IPC and local security — Tier 1 invariants.

## 13. Summary table

| Tier | Failure mode | Mechanism | SLO |
| --- | --- | --- | --- |
| 1 | User-to-user interference | XDG + `SO_PEERCRED` + 0600 | Invariant |
| 2 | Daemon crash (Linux) | systemd `Restart=on-failure` | <5s restart |
| 3 | Daemon crash (Windows) | SCM failure actions | <5s restart |
| 4 | TLS termination / SSO front | nginx Unix-socket upstream | Optional |
| — | Host failure | Out of scope (§6) | N/A |
| — | Shared-account dual sync | `.pcloud/.lock` file lock | Invariant |
