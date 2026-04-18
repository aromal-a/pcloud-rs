> **Pre-alpha scaffold — not live / not production-verified.** This document
> describes design and unit-tested code that has not been validated against a
> real production deployment. Do not treat it as a shippable capability.
> See `CLAUDE.md` and `docs/enterprise/README.md` for the honesty rules.

# Data-Residency Pinning

> **Status:** **Landed (H11 + runtime integration pass)** — enforcement
> helpers, config schema, region resolver cache, audit event, and
> `PolicyViolation` error variant are implemented and unit-tested.
> The daemon runtime now consults the evaluator at three high-value
> call sites (`sync_root_add`, `create_public_link` /
> `create_file_public_link` / `create_folder_public_link`, and
> `create_upload_link`). Strict-mode violations are refused on the
> wire with `PolicyViolation { kind: "data_residency" }` and the
> outcome — allow, warn, or refuse — is persisted in the audit chain
> under the `residency.warn` / `residency.violation` categories.
> **Remaining integration work:** `set_api_server` dispatch-level
> wiring is still tracked; see §11.

## 1. Purpose

pCloud operates multiple data centers (EU in Luxembourg, US in
Texas) and already exposes a per-account region hint and a
per-folder region attribute. Enterprises in regulated sectors
(finance, health, public sector) need to **prove** that data
never crossed a jurisdictional boundary — not just trust a
server-side setting. The controls that ride on this proof are
concrete:

- GDPR Art. 44 on transfers outside the EEA,
- Swiss FADP,
- UK IDTA,
- HIPAA BAAs naming US-only processing,
- sectoral regimes (SEC 17a-4, FINMA RS 18/3, ASIC CPS 234).

This feature makes the Rust client **refuse** to upload into, or
sync into, a DC that is not in the operator-pinned allow-list,
and logs every residency decision through the signed audit chain
so auditors can enumerate the outcome of every attempt.

In one sentence: *the operator configures allowed regions; the
client refuses everything else and tells the audit chain why.*

## 2. Threat model

Residency enforcement protects against specific failure modes:

| Threat | Mitigation |
| --- | --- |
| Operator routes the session to a DC outside the allow-list via `set_api_server` | Evaluator rejects server hints outside `allowed_regions` (§5.3) |
| pCloud silently migrates a folder to another DC | Background `residency_sweeper` re-validates every ~40 min; `ResidencyMigration` audit event fires; queued uploads cancelled; sync root transitions to `Quarantined` (§5.4) |
| Test account accidentally uploads into non-compliant region | Pre-upload evaluator aborts before any byte hits the wire (§5.2) |
| Scanner-failure fallthrough becomes an exfiltration channel | `data_residency.strict = true` forces `upload.dlp.mode = strict`; config validation refuses the unsafe combination |
| Snapshot replication to non-compliant bucket | `disaster-recovery.md` cross-reference: destination region is checked against `allowed_regions` at snapshot-create time |
| Cache-poisoning via a lying server response | Cache invalidates on mismatch; `ResidencyMigration` logged; stale-region uploads cancelled |
| Audit-log tampering to hide violations | Records are chained by rolling hash in `pcloud-observability`; tail hash embedded in next snapshot manifest (§4 of `disaster-recovery.md`) |

Explicit **non-threats:** this feature does **not** claim
cryptographic attestation of residency. It trusts pCloud's
region attribute (same trust model as the legacy C client). A
malicious pCloud operator returning a forged region string would
defeat the client-side check; enterprises needing stronger
guarantees must combine this with crypto folders and/or enterprise
KMS (`kms.md`).

## 3. Scope

Landed (H11):

- `[data_residency]` config section,
- region resolver with 1-hour TTL in-memory cache,
- `PolicyViolation { kind: String }` wire error,
- `PluginAuditEvent::ResidencyViolation` audit record,
- three enforcement **helpers** (not yet adopted at call-sites).

Out of scope for H11:

- adoption of the evaluator at the three daemon runtime
  call-sites — tracked as the known gap in §11,
- configurable cache TTL (fixed at 1h),
- per-file residency overrides (pCloud's API is folder-level),
- cryptographic residency attestation (non-goal, §11),
- choosing where pCloud stores data — that is an account-level
  contract with pCloud, not a client decision.

## 4. Design

### 4.1 Region resolution

pCloud exposes a region attribute per file/folder through
standard metadata APIs (`locationid` / `region`). A helper in
`pcloud-proto` wraps this:

```rust
pub async fn resolve_folder_region(
    client: &ApiClient,
    folder_id: u64,
) -> Result<Region, ApiError>;
```

Results are cached for **1 hour** in the daemon's in-memory
`residency_cache`, keyed by `folder_id`. The cache is invalidated
whenever the server returns a different region, and a
`ResidencyMigration` audit event is emitted (§5.4). TTL is
**fixed** at 1h in the H11 landing.

### 4.2 Evaluator

The evaluator is a pure function of `(resolved_region,
allowed_regions, strict)`:

```
Allow     if allowed_regions is empty  (backward-compatible default)
Allow     if resolved_region in allowed_regions
Deny      if resolved_region not in allowed_regions and strict
WarnAudit if resolved_region not in allowed_regions and not strict
Deny      if resolved_region is unknown and strict
```

### 4.3 Migration handling

On every upload, if the freshly resolved region differs from the
cached region, the cache is updated **and** a `ResidencyMigration
{ folder_id, from, to, detected_at }` audit record is written. If
`to ∉ allowed_regions`, all queued uploads for that folder
subtree are cancelled with `PolicyViolation` and the sync root is
transitioned to a `Quarantined` state (new state in
`pcloud-engine`). User intervention is then required: either add
the new region to the allow-list, or detach the sync root.

Migration events are rate-limited to one entry per folder per
`cache_ttl` (1h) to prevent audit-log spam during rolling
migrations.

A daemon-level background task (`residency_sweeper`) re-validates
active sync-root regions every `cache_ttl * 4` (~40 min), so
silent backend migrations are caught even in the absence of user
activity.

## 5. Interfaces

### 5.1 Sync-root add (enforcement point 1)

`sync_backend::sync_add` — after remote-folder validation,
resolve the remote region. If region ∉ `allowed_regions` refuse
with `ResponseStatus::PolicyViolation { kind: "data_residency" }`.

### 5.2 Pre-upload (enforcement point 2)

`transfer_backend::upload_create` — resolve the parent folder's
region (cache-hit path). On mismatch in strict mode, abort
before any byte hits the wire. In non-strict mode, emit a
warning audit record and allow the upload.

### 5.3 API-server selection (enforcement point 3)

`auth_backend::set_api_server` — reject server hints that would
route the session to a DC outside `allowed_regions`. This
preserves pCloud's routing semantics but keeps the daemon's
policy authoritative.

### 5.4 Wire error

```rust
pub enum ResponseStatus {
    // ...
    PolicyViolation { kind: String },
    // ...
}
```

Wire form: `{"PolicyViolation":{"kind":"data_residency"}}`.
`#[non_exhaustive]` — consumers must handle unknown `kind`
values.

### 5.5 Audit event

```rust
PluginAuditEvent::ResidencyViolation {
    action: ResidencyAction,  // SyncRootAdd | UploadCreate | SetApiServer
    region: String,           // the resolved / requested region
    allowed: Vec<String>,     // snapshot of allowed_regions at decision time
}
```

Emitted on every strict-mode denial and every non-strict
warn-downgrade. Plugins with
`PluginCapability::ObserveStatus` can subscribe (read-only) — the
pathway SIEM plugins (Splunk, Elastic) use to export residency
decisions without touching the upload pipeline.

### 5.6 CLI

```
pcloudc config set data_residency.allowed_regions EU
pcloudc config set data_residency.strict true

pcloudc residency status
pcloudc residency check /Work/Regulated
pcloudc residency migrations --since 24h
```

- `residency status` — every active sync root, its resolved
  region, and cache age.
- `residency check <path>` — live resolution bypassing cache.
- `residency migrations` — reads `ResidencyMigration` events
  from the audit chain.

All config mutations go through the existing config-write path,
which already enforces owner-only permissions on the config file.

## 6. Configuration

```toml
[data_residency]
# Empty vector = no pin, allow all regions (backward-compatible default).
allowed_regions = ["EU"]      # e.g. ["EU"], ["US"], ["EU", "US"]
strict          = true         # hard refusal vs warn-only
```

Validation (fail-closed at daemon start):

- Empty `allowed_regions` means "unrestricted" — the default for
  configs that do not declare the section. Byte-compatible with
  pre-H11 configs.
- Region strings are validated against the set returned by
  `get_api_servers` on first enforcement call. Unknown strings
  in `allowed_regions` are refused at config validation time.
- `strict = true` → denials abort with `PolicyViolation`.
- `strict = false` → denials downgrade to a warning audit record
  and allow the operation to proceed.

## 7. Onboarding

**Minimal operator walkthrough:**

1. Confirm the account's actual region via
   `pcloudc residency check /` (returns the root folder's
   region).
2. Set the allow-list to exactly that region:
   ```
   pcloudc config set data_residency.allowed_regions EU
   pcloudc config set data_residency.strict true
   ```
3. Restart the daemon.
4. Run `pcloudc residency status`. Every active sync root must
   list the expected region.
5. Subscribe your SIEM/plugin to `PluginAuditEvent::
   ResidencyViolation` via `PluginCapability::ObserveStatus`.
6. Wire `PCloudResidencyMigration` alerts into the same queue
   that handles DLP alerts — a migration into a non-allowed
   region is a P1 incident, not a log line.

## 8. Verification

Unit-tested in isolation:

- Evaluator returns `Allow` / `Deny` / `WarnAudit` for every
  branch of §4.2.
- Cache invalidation on region change.
- `ResidencyMigration` rate limit.
- Unknown region strings in `allowed_regions` refused at
  startup.

Integration tests planned in `pcloud-mockserver` (per §10):

- allowed region → upload succeeds,
- denied region → `PolicyViolation`,
- silent migration → `Quarantined` state,
- strict-mode config-validation refuse-to-start.

Integration-verified via `crates/pcloud-daemon/tests/residency.rs`
(unit-level `check_residency` coverage for every decision branch
plus dispatch-ordering assertions that the auth-token gate fires
before the residency gate). End-to-end live verification against
the production API is still pending; the in-tree mock covers the
decision surface without network.

## 9. Failure modes

| Failure | Behaviour |
| --- | --- |
| pCloud returns no region attribute | `resolve_folder_region` returns `Region::Unknown`; strict mode → Deny; non-strict → WarnAudit |
| pCloud returns region not in `allowed_regions` | Deny (strict) or WarnAudit (non-strict) |
| pCloud region differs from cached region | Cache updated; `ResidencyMigration` logged; if new region ∉ allowed, queued uploads cancelled, sync root `Quarantined` |
| Network outage during `resolve_folder_region` | Fail-closed in strict mode (Deny); fail-open with audit flag in non-strict |
| Config loaded with empty `allowed_regions` | Accepted; acts as "unrestricted"; this is the backward-compatible default |
| Config loaded with unknown region string | Daemon refuses to start |
| `strict=true` with no scanners in `[upload.dlp]` | Config validation refuses to start (see `dlp.md` §4) |

## 10. Honest limitations

pre-alpha reality check:

- **Two of three call-sites wired in the daemon runtime**
  (`sync_root_add` and the public-link / upload-link creation
  paths). `set_api_server` wiring is still outstanding; the
  helper exists but is not consulted on that dispatch path
  yet. See §11.
- **Trust in pCloud's region attribute.** No cryptographic
  attestation; a lying server defeats the check. Combining with
  crypto folders narrows the blast radius but does not replace
  attestation.
- **Fixed 1h TTL.** Not yet configurable.
- **Folder-level granularity.** Per-file residency overrides are
  not supported (pCloud API limitation).
- **In-memory cache.** Cache is reset on daemon restart; the
  first operation after restart pays a resolver round-trip.

## 11. Daemon runtime adoption

H11 landed the helpers, schema, cache, audit event, and
`PolicyViolation` variant. The runtime integration pass has since
wired the evaluator into the three highest-value enforcement
points in `pcloud-daemon/src/runtime.rs`:

- `add_sync_root` — after `validate_remote_folder`, the active
  host region is classified via
  `pcloud_backends::residency::resolve_region_from_host`
  (memoized through `residency_cache`) and passed through
  `RuntimeShell::check_residency(ACTION_SYNC_ROOT_ADD, region)`.
  A strict-mode refusal short-circuits before the sync-root
  record is persisted.
- `create_public_link` (file and folder flavors) — the same
  region resolution and `check_residency(ACTION_UPLOAD_CREATE,
  region)` call block the public-link create before the backend
  is invoked, so a disallowed region cannot publish a share URL.
- `create_upload_link` — identical enforcement, aimed at the
  write-side surface (third-party push into the tenant).

Audit records land under the categories `residency.warn` (non-
strict near-misses) and `residency.violation` (strict refusals),
carrying a stable `op=… region=… allowed=[…] refused=… warned=…`
detail string. The refusal response uses the
`PolicyViolation { kind: "data_residency" }` wire shape with a
helpful message naming the offending region and the allow-list.

Known remaining gap:

- `set_api_server` dispatch-level enforcement. The helper
  (`pcloud_backends::account_backend::enforce_set_api_server_residency`)
  exists and is unit-tested; wiring it into
  `RuntimeShell::set_api_server` is tracked as a separate bead
  so the pre-alpha claim stays honest — until that lands, an
  operator can still re-home the session between
  `allowed_regions` changes.
- `ValidatedRemoteFolder` does not yet expose a per-folder
  `api_server` hint, so `add_sync_root` currently falls back
  to the active host region. That is conservative (it blocks
  sessions pinned to disallowed DCs) but not maximally precise.
  A follow-up refactor can surface the folder-level hint.

## 12. Extension points

- **Add a new region** — register the canonical region code in
  `pcloud-proto::Region` and update `get_api_servers` mapping.
  The evaluator picks it up automatically.
- **Add a new enforcement point** — plug the evaluator into the
  chosen call-site and extend `ResidencyAction` with the new
  variant. The audit event schema is additive.
- **Swap the cache backend** — current in-memory cache could be
  replaced with a SQLite-backed cache in `pcloud-store` for
  restart-persistent decisions. Not required in H11.
- **SIEM export** — subscribe via `PluginCapability::
  ObserveStatus`; no host changes needed.

## 13. Cross-refs

Code:

- `crates/pcloud-config/` — `[data_residency]` section + validation.
- `crates/pcloud-proto/src/auth_api.rs` — `set_api_server`,
  `get_api_servers`.
- `crates/pcloud-proto/` (metadata module) — `Region` type,
  `resolve_folder_region`.
- `crates/pcloud-daemon/src/sync_backend.rs`,
  `transfer_backend.rs`, `auth_backend.rs` — enforcement points
  (integration pending).
- `crates/pcloud-error/` — `ResponseStatus::PolicyViolation`.
- `crates/pcloud-observability/` — audit chain,
  `PluginAuditEvent::ResidencyViolation`.

Related docs:

- `docs/enterprise/dlp.md` — §4 cross-constraint
  (`strict ⇒ dlp.mode = strict`).
- `docs/enterprise/disaster-recovery.md` — §8 snapshot
  destination region check.
- `docs/enterprise/kms.md` — combine residency with envelope
  encryption for stronger posture against a malicious DC.
