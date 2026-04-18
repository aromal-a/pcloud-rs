# 30-Day RC Soak Runbook

This runbook governs the mandatory 30-day soak for any `pcloud-rs` Release
Candidate (RC) build prior to General Availability (GA). The soak exercises
the RC against real pCloud traffic on a dedicated test tenant, with daily
workload automation and weekly chaos injection. It is the final gate for GA
sign-off and complements `bd-1du.10` (final parity proof).

The soak is binary: every numbered pass criterion must hold for the full
window. A single hard failure triggers rollback and re-queues the RC.

---

## 1. Preparation (Day -3 to Day 0)

### 1.1 Dedicated pCloud test tenant

- Provision a dedicated pCloud account (paid tier, ≥ 500 GiB quota).
- Enable TFA with a recovery code stored in the ops password manager.
- Generate a long-lived API token for headless automation and store it in
  the soak-operator vault (not the RC binary, not the repo).
- Pre-seed the account with a known-good corpus: `corpus-baseline.tar` must
  unpack to 100,000 files across 5,000 directories, total ≈ 20 GiB, mixed
  sizes from 0 bytes to 500 MiB.
- Create a dedicated crypto folder `/soak-crypto` with a rotating passphrase
  recorded per run.

### 1.2 Host fleet

Allocate at minimum:

| Role    | OS                    | CPU    | RAM    | Disk  |
| ------- | --------------------- | ------ | ------ | ----- |
| soak-l1 | Linux (Debian 12)     | 4 vCPU | 8 GiB  | 200 G |
| soak-m1 | macOS 14 (Apple sil.) | 4 vCPU | 16 GiB | 200 G |
| soak-w1 | Windows 11 23H2       | 4 vCPU | 8 GiB  | 200 G |
| soak-b1 | FreeBSD 14 (optional) | 4 vCPU | 8 GiB  | 200 G |

Each host must have:

- NTP locked to `pool.ntp.org` (clock-jump chaos will deliberately break
  this; the baseline must be clean).
- A dedicated `pcloud-soak` UNIX/Windows account with no other workloads.
- Outbound HTTPS to `*.pcloud.com` only; no other egress.
- Per-host Prometheus scraper pointed at the daemon `/metrics` and `/slo`
  endpoints.
- `pcloud-chaos` preinstalled and gated behind `PCLOUD_CHAOS=1`.

### 1.3 RC installation

Install the RC **via the native package** for each OS — never via
`cargo install` during soak. This proves the packaging path, not just the
binary:

- Linux: `apt install ./pcloud-rs_<ver>_amd64.deb`
- macOS: `installer -pkg pcloud-rs-<ver>.pkg -target /`
- Windows: `msiexec /i pcloud-rs-<ver>.msi /qn`
- FreeBSD: `pkg add pcloud-rs-<ver>.pkg`

Record package SHA-256, signature verification output, and `pcloudc
--version` into the run log.

### 1.4 Baseline bring-up

Before Day 1, complete one full manual smoke:

1. `pcloudc login` with TFA.
2. `pcloudc sync add` a 1 MiB tree, confirm upload.
3. `pcloudc mount /mnt/pcloud`, list root, unmount cleanly.
4. `pcloudc audit verify` must return `ok`.

If any step fails, **do not start the soak**. File an RC blocker.

---

## 2. Daily Automation (Day 1 to Day 30)

A single cron entry on each host drives the daily workload. It must run as
the `pcloud-soak` user and log to `/var/log/pcloud-soak/<date>/`.

### 2.1 Cron schedule

```
# /etc/cron.d/pcloud-soak
0  2 * * * pcloud-soak /opt/pcloud-soak/bin/daily.sh   >> /var/log/pcloud-soak/daily.log 2>&1
0  * * * * pcloud-soak /opt/pcloud-soak/bin/scrape.sh  >> /var/log/pcloud-soak/scrape.log 2>&1
30 1 * * 0 pcloud-soak /opt/pcloud-soak/bin/chaos.sh   >> /var/log/pcloud-soak/chaos.log 2>&1
```

### 2.2 `daily.sh` workload

Each daily run performs, in order:

1. **100k-file sync.** Rsync-expand `corpus-baseline.tar` into the sync
   root under a date-stamped subdir, wait for the daemon queue to drain.
   Abort if drain exceeds 6 hours.
2. **10 GiB single-file upload.** `dd if=/dev/urandom of=big.bin bs=1M
   count=10240`, `pcloudc upload big.bin /soak/big-<date>.bin`. Verify
   server-side SHA-256 via `pcloudc checksumfile`.
3. **8 concurrent publink creates.** Spawn 8 `pcloudc publink create`
   processes against distinct files, assert all 8 URLs resolve with
   `curl -I` in < 5 s each.
4. **One mount–unmount cycle.** `pcloudc mount /mnt/pcloud`, `ls -laR
   /mnt/pcloud/soak | wc -l`, `pcloudc unmount /mnt/pcloud`. Verify
   unmount is clean (no stale mount entry).
5. **One crypto-folder setup + teardown.** Create ephemeral
   `/soak-crypto/<date>`, upload a 1 MiB fixture, read it back,
   `pcloudc crypto-folder remove`.

Each step exits non-zero on failure; the wrapper records a per-step
scorecard line (see §6).

### 2.3 Hourly `scrape.sh`

```bash
ts=$(date -u +%Y%m%dT%H%M%SZ)
curl -s http://127.0.0.1:9898/metrics > /var/log/pcloud-soak/metrics/${ts}.prom
curl -s http://127.0.0.1:9898/slo     > /var/log/pcloud-soak/slo/${ts}.json
```

### 2.4 Log rotation and archive

- `logrotate` daily, keep 35 days on-host.
- Nightly rsync of `/var/log/pcloud-soak/` → `s3://pcloud-soak-archive/
  <host>/<run-id>/` with server-side encryption.
- Audit logs (`/var/lib/pcloud/audit/*`) are rotated separately and
  **never pruned during the soak window**.

---

## 3. Weekly Chaos Injection

On Day 7, 14, 21, and 28 at 01:30 local, `chaos.sh` selects **one** scenario
from `pcloud-chaos` at random (seeded from the run-id for reproducibility)
and injects it with `PCLOUD_CHAOS=1`:

| Scenario          | Invocation                              | Expected recovery                               |
| ----------------- | --------------------------------------- | ----------------------------------------------- |
| SIGKILL mid-flush | `pcloud-chaos sigkill --mid-flush`      | Daemon restarts; journal replays; no data loss |
| Disk-full         | `pcloud-chaos diskfull --stage 95%`     | Back-pressure; no panic; auto-resume on free   |
| Blackhole         | `pcloud-chaos netblackhole --secs 120`  | Exponential backoff; resume within 60 s of net |
| Clock-jump        | `pcloud-chaos clockjump --delta +2h`    | TLS still valid; auth still works              |
| Slowloris         | `pcloud-chaos slowloris --conns 64`     | IPC timeouts bound; no FD leak                 |

**Recovery assertion.** Within 60 s of chaos clearing, `pcloudc status`
must report `state=healthy`, `/slo` must show `error_budget_burn < 2x`, and
no new panic files may appear under `/var/lib/pcloud/panics/`.

A recovery miss counts as one chaos-recovery failure (see §5).

---

## 4. Pass Criteria

The RC passes the soak if and only if **every** criterion below holds for
the full 30-day window, across **all** hosts:

1. **Zero unattended crashes.** No SIGSEGV, no panic file, no systemd
   `Result=core-dump`. Operator-triggered restarts during chaos do not
   count.
2. **IPC p95 latency < 10 ms, 99% of the time.** Measured from hourly
   `/slo` snapshots; at most 7 hours out of ~720 may breach.
3. **Upload retry ratio < 1%.** `pcloud_transfers_retry_total /
   pcloud_transfers_attempts_total` over the window.
4. **Audit chain verifies every day.** `pcloudc audit verify` exits 0 at
   the end of every daily run; any BREAK entry is a hard fail.
5. **Zero user-visible data loss or corruption.** Nightly diff of the
   local corpus vs. server-side SHA-256 list must be empty.
6. **No RustSec advisory unhandled > 7 days.** `cargo audit` runs on the
   RC tag daily; any new `VULN` advisory must have a patched build or
   written waiver within 7 days.

---

## 5. Fail Criteria and Rollback

Any **one** of the following terminates the soak immediately:

- Any unattended crash on any host.
- Sync-state divergence: local file present, server missing, or vice
  versa, that does not self-heal within one drain cycle.
- Audit chain breakage (`pcloudc audit verify` reports `BREAK` at index
  N).
- Security advisory in a direct or transitive dependency not patched
  within 7 calendar days.
- Three or more consecutive chaos-recovery failures (regardless of
  scenario).

### Rollback procedure

1. Mark the run `FAILED` in the scorecard and in `bd-1du.10`.
2. Freeze all soak hosts: `systemctl stop pcloudd` on Linux/BSD,
   equivalent on macOS/Windows.
3. Capture forensics: full `/var/lib/pcloud/`, `/var/log/pcloud-soak/`,
   core dumps, and last 24 h of `/metrics` snapshots to the archive
   bucket under `s3://pcloud-soak-archive/<run-id>/FAILED/`.
4. Re-install the previous GA package via the native package path.
5. Open a P0 bead linked to `bd-1du.10` with the failure class, the
   forensics pointer, and a named owner.
6. A new RC must restart the full 30-day window from Day 1. Partial
   credit is **not** allowed.

---

## 6. Scorecard Template

One row per host per day. Tracked in
`docs/book/src/operations/rc-soak-scorecard-<run-id>.csv` and mirrored in
the ops dashboard. `[x]` = pass, `[ ]` = fail, `[-]` = not scheduled.

```
run-id,host,date,sync100k,upload10g,publink8,mount_cycle,crypto_cycle,chaos,audit_verify,p95_ipc_ms,retry_ratio_pct,notes
<run>,soak-l1,2026-04-15,[x],[x],[x],[x],[x],[-],[x],6.2,0.31,
<run>,soak-m1,2026-04-15,[x],[x],[x],[x],[x],[-],[x],7.8,0.42,
<run>,soak-w1,2026-04-15,[x],[x],[x],[x],[x],[-],[x],8.1,0.55,
<run>,soak-b1,2026-04-15,[x],[x],[x],[x],[x],[-],[x],6.9,0.28,
...
<run>,soak-l1,2026-04-22,[x],[x],[x],[x],[x],[x],[x],7.4,0.39,sigkill-mid-flush recovered 41s
```

Sign-off block (appended at Day 30):

```
Run-id:         <run>
RC version:     <ver>
Start:          2026-04-15
End:            2026-05-14
Hosts passed:   4 / 4
Crashes:        0
p95 breaches:   3 / 720 hours
Retry ratio:    0.41%
Audit verifies: 120 / 120
Chaos runs:     16 / 16   (4 per host × 4 weeks)
Decision:       GO / NO-GO
Approvers:      <release-lead>, <security-lead>, <sre-lead>
```

GA may ship only when the decision line reads `GO` and all three
approvers have signed.
