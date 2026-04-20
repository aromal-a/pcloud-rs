# Future plugin ideas

> Status: brainstorm — none of these are scheduled. This document
> captures proposals generated during a design conversation so the
> team can pick up any item without re-deriving the rationale. When
> an item is picked up, open a bead under the `plugin` label and
> move the item into the main plugin catalogue table in
> [README.md](./README.md) once a first-pass crate lands.

The existing first-party plugin set (see [README.md](./README.md))
covers publink expiry, autoheal, backup schedule, and DLP. The ideas
below extend the catalogue along the project's existing
enterprise/security/ops lean.

Every idea lists: **what it does**, **why it's useful**, **which
existing capabilities it would need**, and **minimum viable scope**
— i.e. the smallest thing that is still worth shipping.

---

## Security / compliance

### `crypto-autosort`

**What:** Pattern-based auto-move of files matching configurable
globs (e.g. `*.key`, `*.pem`, `*.env`, `tax/**`, `wallet/**`) into
the Crypto Folder after upload.

**Why:** Complements `pcloud-plugin-dlp`, which *detects* obvious
secrets and can block upload. This one *remediates* the common case
where a user uploads sensitive material to an unencrypted path.
Closes the "I uploaded my SSH key by accident" failure mode.

**Capabilities needed:** `ObserveStatus` (to see uploads complete)
+ a new `FileMove` capability (not yet present in
`pcloud-plugin-api`; would need to land first).

**Min viable:** Hard-coded rule set (`*.key`, `*.pem`, `.env*`),
operator-configurable destination folder, emit a desktop
notification when a move fires so the user knows their secret is
now in Crypto. No rollback. No wildcard escape (`tax/**` only,
never `**/*.key` which would catch unrelated binaries).

---

### `share-auditor`

**What:** Scheduled sweep of all active public links + shares.
Flags stale entries (`created_at + N days`, no access in M days),
over-permissioned shares (e.g. write when only read was requested),
and pre-compliance-date entries (shares created before a configured
policy date).

**Why:** Share/public-link drift is the #1 reason enterprise audits
fail. You already have the public-link and shares backends wired
(ncx.66 / audit-06 closed); this plugin turns that infrastructure
into a scheduled audit report.

**Capabilities needed:** `ObserveStatus` (to read the share list)
+ `SyncControl` (to revoke). Emits to Prometheus via
`pcloud-observability` — reuses the existing dashboard infra.

**Min viable:** Daily sweep, flag-only (no auto-revoke), writes a
JSON report under `~/.local/state/pcloud-rs/share-audits/`. Surface
one new Prometheus gauge: `pcloud_share_audit_flagged_count`.

**Recommended pick if only one plugin ships** — highest enterprise
signal-to-effort, leverages existing backends, plugs into existing
dashboards.

---

### `access-geofence`

**What:** Alert when a download or share access originates from an
IP/geo outside an operator-allowlist.

**Why:** Catches stolen-credential post-exploitation patterns. You
already have auth audit logs (audit_verifier_service.rs); this
plugin just consumes them and emits alerts.

**Capabilities needed:** `ObserveStatus` on auth/access events.
Requires an IP geolocation source (either bundled MaxMind GeoLite2
DB, or an external HTTP call — operator chooses).

**Min viable:** Allowlist of country codes (`US,CA,DE`), emit a
desktop notification on deny. Log to a fixed file. No active
enforcement (that would be a separate policy-engine feature).

---

### `compliance-report`

**What:** Generate a monthly PDF/HTML report covering: all active
public links + share ACLs, crypto state (locked/unlocked,
fingerprint), retention-policy violations, failed auth attempts
aggregated by source, and mount-integrity sweeps.

**Why:** SOC2/GDPR/HIPAA auditors want this exact report shape.
Right now a customer would need to cobble it together by hand.

**Capabilities needed:** `ObserveStatus` on virtually everything.
Requires a report template engine (probably `tera` or `askama`).

**Min viable:** Markdown output (not PDF — let the operator pipe
to `pandoc` if they want PDF). First-of-month cron trigger.
Published to a configurable destination folder (could be inside
pCloud itself, meta).

---

## Data hygiene

### `dedup-scan`

**What:** Periodic content-hash scan to identify duplicates across
the sync root. Reports duplicate groups with total reclaimable
bytes.

**Why:** Most users have thousands of duplicated files accumulated
over years (same screenshot uploaded twice, photo library imports
re-run). Storage savings are usually 5-15%.

**Capabilities needed:** `ObserveStatus` (to enumerate), plus
access to the existing file-hash metadata from the diff poller.
The pCloud API exposes per-file hash (`hash` u64) so we don't need
to re-read file content — just query the diff.

**Min viable:** Report-only mode. Outputs JSON listing duplicate
groups. No automatic dedup (server-side-copy + delete-original is
tracked under `bd-1du.10` row 93 and needs IPC wiring to land
first). User can manually act on the report.

---

### `retention-policy`

**What:** Age-based rules: "archive files in `/Incoming` older than
90 days to `/Archive`", "delete files in `/Recordings` older than
1 year", "delete files with tag `tmp` after 7 days".

**Why:** Standard enterprise data-lifecycle hygiene. The diff
poller + selective sync infra already has everything needed; this
plugin just applies policy.

**Capabilities needed:** `ObserveStatus` + `SyncControl` +
filesystem read (for tag/metadata extraction if we want
tag-based rules). Move/delete must route through the existing
transfer backend error classifier (ncx.48).

**Min viable:** Age-based only (no tags). Rules in a TOML file.
Dry-run mode by default; operator flips a flag to enable actual
moves/deletes. Emit a Prometheus counter per action.

---

## Operational

### `quota-runway`

**What:** Tracks storage usage over time, fits a trend line,
projects when the account will hit the plan quota. Alerts at
configurable thresholds (80%, 90%, 95%, projected-7-days-to-full).

**Why:** Nobody likes finding out they're out of storage when an
upload silently fails mid-backup.

**Capabilities needed:** `ObserveStatus` on `userinfo` (gives quota
+ used-bytes). Persists time-series to SQLite or the existing
settings store.

**Min viable:** Daily poll, fixed thresholds (80/90/95%), desktop
notification on breach. Linear regression on the last 30 days for
the runway projection — no fancy time-series math.

---

### `webhook-notifier`

**What:** Emit signed HTTP webhooks on events: upload complete,
share created, public link created/accessed, crypto unlock, mount
start/stop.

**Why:** Unlocks arbitrary third-party integrations (Slack notify
on upload, Discord bot on share, Zapier triggers) without forcing
each integration into the plugin API itself. One plugin, infinite
endpoints.

**Capabilities needed:** `ObserveStatus` on all emitted event
types. HTTP client (already transitively present via `reqwest`).
Needs a signing-key/secret store — reuse `pcloud-secret` wrappers.

**Min viable:** Single webhook URL, one shared HMAC secret for
signing, fixed event set (upload, share, public-link). Operator
configures URL + secret via settings. Retry with exponential backoff
using `pcloud-resilience::TokenBucket`.

---

## Developer-facing

### `cli-hooks`

**What:** Run a user-specified shell command on file events —
`inotifywait`-style but server-driven via the diff stream so it
works for remote-only changes too.

**Why:** Closest pCloud has to a programmable trigger. Useful for
indexing (run `updatedb`), CI (push to git repo), local backups,
anything.

**Capabilities needed:** `ObserveStatus` on diff events + subprocess
spawn. Needs careful sandboxing — the shell command should inherit
a restricted env, with file path passed as $1.

**Min viable:** One command, one glob pattern. No fan-out. Execute
with a fixed timeout (30s). Log stdout/stderr to a fixed file. Do
not ship until the DLP pattern matcher lands (reuse its
path-pattern logic so it's consistent with other plugins).

---

### `scripted-transforms`

**What:** Pipe new uploads through a user script. Example use: OCR
PDFs via `tesseract`, resize uploaded images via `imagemagick`,
virus-scan via `clamscan`, strip EXIF via `exiftool`.

**Why:** Removes the need to write a whole plugin crate for
one-off transformations.

**Capabilities needed:** `ObserveStatus` on upload completion +
subprocess spawn + the `FileMove` capability (to replace the
original with the transformed version). Same sandboxing concerns
as `cli-hooks`.

**Min viable:** One transform per glob pattern (e.g. `*.pdf` → OCR
script). In-place replacement only; no forked-file output. Fixed
timeout. Fail-open on script error (original stays). Mutually
exclusive with `cli-hooks` until the capability model distinguishes
observer from mutator.

---

## Integration plays

### `git-lfs-bridge`

**What:** Expose the pCloud account as a Git LFS remote. `git lfs
push` / `git lfs fetch` routed through pcloud-rs.

**Why:** A real problem for teams who want large-file storage but
don't want yet another S3 bill. pCloud's 10+ TB tiers are
competitive per-GB.

**Capabilities needed:** HTTP server (not present — would need
pulling in `axum` or similar). The LFS protocol is a simple
batch-oid-over-HTTPS shape. Persistent per-repo OID→pCloud-fileid
mapping in a SQLite store.

**Min viable:** Single-repo mode (no multi-tenant), batch upload +
download only (no locking or auth delegation). Documented as
"experimental" — LFS has protocol quirks that only show up at
scale.

---

### `restic-repo`

**What:** Expose a pCloud folder as a `restic` backup repository.
`restic` sees it as a regular filesystem path via the existing
FUSE mount; this plugin just wires the mount lifecycle to
`restic`'s expected layout.

**Why:** restic is the gold standard for encrypted backups, and
running it against pCloud today requires manual mount + path
setup. Makes it one config stanza instead of several steps.

**Capabilities needed:** `SyncControl` to drive the mount. A
shipped `restic` wrapper script or systemd unit template.

**Min viable:** Drop in the systemd unit template + a `pcloudd
mount --restic /my/backups` one-liner CLI. No active coordination
with restic itself — just make the path reliably available.

---

### `photo-sync`

**What:** Watch a local camera / screenshots folder and auto-upload
new media with EXIF-based dedup (don't re-upload if a file with the
same perceptual hash already exists in the target folder).

**Why:** iCloud Photos and Google Photos are the default because
they're turnkey; giving pcloud-rs users a turnkey photo flow
closes the gap.

**Capabilities needed:** `SyncControl` + filesystem watch (already
have `fs_watcher.rs`) + an EXIF library. Dedup would use the pHash
algorithm, not a content-hash, so visually identical-but-re-encoded
photos dedup correctly.

**Min viable:** One watch directory → one destination folder. EXIF
date-based subfolder layout (`YYYY/MM/`). No pHash initially —
start with content-hash, upgrade to pHash in v2.

---

## Picking an order

If the team wants to ship one plugin per wave, my recommended order
(highest value-per-effort first):

1. **`share-auditor`** — biggest enterprise win, purely additive
2. **`quota-runway`** — simplest to implement, high user value
3. **`webhook-notifier`** — unlocks third-party integration flywheel
4. **`retention-policy`** — follows naturally after share-auditor
5. **`crypto-autosort`** — needs the new `FileMove` capability first
6. **`access-geofence`** — needs geolocation dep decision
7. Integration plays (`git-lfs-bridge`, `restic-repo`, `photo-sync`) — larger scope, ship after the core additive ones are in place

Anything below that (`compliance-report`, `dedup-scan`, `cli-hooks`,
`scripted-transforms`) should wait for a concrete customer ask — they
are good ideas but not load-bearing.
