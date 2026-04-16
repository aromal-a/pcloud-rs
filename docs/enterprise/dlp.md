# DLP — Enterprise Data-Loss Prevention Posture

> **Status:** **Design-only.** This page describes the
> **enterprise** DLP posture: central policy distribution,
> third-party connectors (Nightfall, Symantec DLP, Microsoft
> Purview, Google Cloud DLP, ICAP), and signed enterprise plugins.
> It is **not yet implemented**.
> The single-user, in-tree `pcloud-plugin-dlp` plugin is
> **separate** and intentionally minimal — it is a path-hash-only
> built-in intended for solo users, not an enterprise control.
> See §13 for the boundary between the two.
> Wired interfaces referenced here
> (`PluginOperation::PreUploadScan`, `PluginCapability::
> UploadInspect`, audit event shape) are design placeholders and
> should not be treated as contracts until the enterprise wave
> lands.

## 1. Purpose

Enterprise customers are contractually and often legally required
to prevent certain content from ever leaving the endpoint.
Typical controls include:

- pattern matches (PCI, SSN, Stripe secret keys, custom regex
  packs),
- MIME-type denylists (raw disk images, archive bombs, source
  archives from protected repos),
- entropy heuristics (catch high-entropy blobs that look like
  private keys, wallet seeds, unlabelled credentials),
- integration with existing DLP platforms: Nightfall, Symantec
  DLP, Microsoft Purview, Google Cloud DLP, ICAP scanners
  on-prem.

Today, `pcloud-rs` uploads whatever the user or sync engine feeds
it. There is no pre-upload inspection and no hook for
enterprise DLP, aside from the single-user `pcloud-plugin-dlp`
built-in (§13). This page specifies a thin, plugin-native DLP
hook that runs **before** `upload_save` commits the block to
pCloud and that is auditable, fail-safe, and privacy-aware.

The beginner-friendly story: *enterprise policy says "don't let
PCI leave"; the daemon asks every configured scanner before
every upload; if any says no, the upload never happens, and the
audit chain records the reason.*

## 2. Threat model

| Threat | Mitigation |
| --- | --- |
| Regulated content (PCI, PHI, PII) exfiltrated via pCloud upload | Pre-upload scanner chain; any `Deny` short-circuits before `upload_save` |
| Scanner timeout/outage becomes an exfiltration channel | `strict` mode treats timeout as Deny; config validation refuses `strict` DLP with `strict` residency if no scanner is enabled |
| Plugin sees more data than it needs | Default exposure = hash + size + mime + first N bytes only; full-stream access requires signed manifest + capability + config opt-in + TLS |
| Third-party bridge exfiltrates content | TLS-only transport enforced; bearer creds stored as `SecretString` from keyring, never logged; mandatory audit record per call |
| Audit log tampered to hide a Deny | Chain-hashed in `pcloud-observability`; tail hash carried into next DR snapshot |
| Matched-content leakage via audit record | Audit records carry symbolic rule ids only; matched substrings never written |
| Central policy push by attacker (fleet RCE vector) | Policy bundles are signed; distribution is out-of-band (MDM / config-management); daemon refuses unsigned policy bundles |
| Crypto folder bypass (content encrypted before scan) | Scanners run **pre-encryption** for crypto folders; ciphertext inspection is a non-goal |
| Scanner plugin itself compromised | `PluginRegistry` signature check; `UploadStreamAccess` gated behind explicit operator opt-in; scanner runs under daemon UID, not root |

Explicit **non-threats:** post-upload scanning of already-
committed pCloud content; replacing endpoint DLP agents. This
hook is a targeted egress control, not a full endpoint DLP
platform.

## 3. Scope

In scope for the enterprise wave (design-only):

- `PreUploadScan` hook before `upload_save`,
- in-tree built-in scanners (regex, entropy) under
  `pcloud-daemon/src/dlp/`,
- third-party bridge protocol (HTTP POST, optional streaming),
- central policy distribution via signed policy bundles,
- audit record per attempt,
- CLI surface for operator testing + status,
- fail-safe modes (`strict` / `balanced` / `audit_only`).

Out of scope:

- post-upload scanning,
- crypto-folder ciphertext inspection (scanning happens
  **pre-encryption** by design),
- replacing endpoint DLP agents,
- anything the single-user built-in `pcloud-plugin-dlp` already
  does (path-hash allow/deny, see §13).

## 4. Design

### 4.1 Plugin operation

A new variant in `PluginOperation`:

```rust
PluginOperation::PreUploadScan {
    path: PathBuf,
    size: u64,
    content_hash: [u8; 32],       // BLAKE3 of full file content
    mime_guess: Option<String>,
    first_bytes: Bytes,           // up to N bytes, configurable
}
```

Capability: `PluginCapability::UploadInspect` (new). The
required-capability map in `PluginCapability::required_for` gets
a matching arm. Plugins without `UploadInspect` cannot observe
any upload payload — least-privilege by construction.

### 4.2 Plugin response

```rust
PluginOperationResponse::UploadScanVerdict {
    verdict: UploadVerdict,
    matched_rules: Vec<String>,    // symbolic rule ids, no payload echo
    redactions: Option<Vec<RedactionSpan>>,
    scanner_id: String,
    latency_ms: u32,
}

pub enum UploadVerdict {
    Allow,
    Quarantine    { reason: String },
    RedactAndAllow,                   // host applies `redactions` before upload
    Deny          { reason: String },
}
```

`Quarantine` moves the file to a daemon-owned holding area
under `$STATE/quarantine/` with 0600 perms and a JSON sidecar.
User is notified via the existing runtime event channel.
`Deny` fails the upload with `ResponseStatus::PolicyViolation`.

### 4.3 Call site

Inside `transfer_backend.rs`, after the upload buffer is
committed locally and hashed but **before** `upload_save`:

1. Host builds `PreUploadScan` op.
2. Host dispatches to **every** registered scanner in parallel
   (Tokio `JoinSet`), gated by `timeout_ms`.
3. Verdicts are merged:
   `Deny > Quarantine > RedactAndAllow > Allow`.
4. Any `Deny` short-circuits.
5. Host emits a single `PluginAuditEvent::UploadScan` per
   attempt (§7).
6. On `Allow` (or post-redaction), host proceeds to
   `upload_save`.

The host never re-entrantly calls a plugin with raw bytes beyond
`first_bytes` unless the plugin holds `UploadStreamAccess`
(§8).

### 4.4 Built-in scanners

Built-ins live under `pcloud-daemon/src/dlp/` — not new crates.

- **Regex**: pattern packs
  (`rules = ["pci", "ssn", "aws-keys"]`), compiled once via
  `regex::RegexSet`. Matches symbolic rule ids only.
- **Entropy**: sliding-window Shannon entropy on `first_bytes`
  (default 64 KiB, window 256 B, stride 128 B). Threshold
  default `7.5 bits/byte`. Top-decile windows above threshold
  trigger `Quarantine` (strict) or `Allow + flag` (audit-only).

### 4.5 Third-party bridge

- HTTP POST to `endpoint` with JSON body:
  `{ hash, size, mime, first_bytes_b64 }`.
- When `UploadStreamAccess` is granted, body is
  `Transfer-Encoding: chunked` streaming the full content.
- TLS-only (production transport policy in `pcloud-config`
  forbids plaintext bridges).
- Bearer credential stored as `SecretString` sourced from the OS
  keyring via `pcloud-secret`. Never logged. Never echoed.

### 4.6 Central policy distribution

The enterprise mode consumes **signed policy bundles**
distributed out-of-band (MDM, Ansible, SCCM, Intune, Jamf).
Bundle format:

- TOML document with `[upload.dlp.*]` sections,
- accompanying detached signature (GPG or Sigstore),
- daemon refuses unsigned bundles even if `--allow-unsigned` is
  passed (distinct from DR snapshots).

Policy rotation is pull-based: daemon watches a configured
directory, atomically swaps the active bundle when a newer
valid-signed bundle appears, and emits
`PluginAuditEvent::PolicyRotated` with the old/new bundle
digests. A roll-back bundle with a lower `policy_version` is
refused unless `allow_downgrade = true` is explicitly set in the
retained policy.

### 4.7 Fail-safe matrix

Each scanner has a per-call deadline (`timeout_ms`, default
`5000`).

| Mode | Timeout | Error | Unavailable |
| --- | --- | --- | --- |
| `strict` | Deny | Deny | Deny |
| `audit_only` | Allow + flag | Allow + flag | Allow + flag |
| `balanced` | Quarantine | Quarantine | Allow + flag |

`strict` is the only mode permitted when
`data_residency.strict = true` (see `data-residency.md`) to avoid
exfiltration through the scanner-failure path. Config validation
refuses the unsafe combination at startup.

## 5. Interfaces

- `PluginOperation::PreUploadScan` — host → scanner.
- `PluginOperationResponse::UploadScanVerdict` — scanner →
  host.
- `PluginAuditEvent::UploadScan` — host → audit chain.
- `PluginAuditEvent::PolicyRotated` — host → audit chain on
  bundle rotation.
- CLI:
  ```
  pcloudc dlp status           # lists scanners + data class each sees
  pcloudc dlp test ./file      # dry-run, prints verdict + audit entry
  pcloudc dlp reload-policy    # re-read the signed bundle
  ```

## 6. Configuration

```toml
[upload.dlp]
enabled = true
mode = "strict"        # strict | balanced | audit_only
timeout_ms = 5000
first_bytes = 65536
scanners = ["builtin.regex", "builtin.entropy", "nightfall"]

[upload.dlp.builtin.regex]
rule_packs = ["pci", "ssn", "aws-keys"]
custom_rules_path = "/etc/pcloud-rs/dlp/custom.toml"

[upload.dlp.builtin.entropy]
threshold_bits = 7.5
window_bytes = 256

[upload.dlp.nightfall]
endpoint = "https://api.nightfall.ai/v3/scan"
credential_ref = "keyring:nightfall_api_key"
stream_full_content = false

[upload.dlp.policy]
bundle_dir = "/etc/pcloud-rs/dlp/policies"
signing_keyring = "/etc/pcloud-rs/dlp/keyring.gpg"
allow_downgrade = false
```

Validation (fail-closed at daemon start):

- `mode = strict` requires at least one enabled scanner.
- `stream_full_content = true` requires all of:
  `UploadStreamAccess` in manifest + signed manifest
  verification + TLS-only transport.
- `timeout_ms` clamped to `[100, 30_000]`.
- `policy.bundle_dir` must be owner-only (0700); unsigned
  bundles refused.

## 7. Audit

Every upload attempt appends one record to the signed audit
chain:

```
{
  "ts": "...",
  "path": "/Work/q4.xlsx",
  "size": 184320,
  "content_hash": "blake3:...",
  "verdict": "Deny",
  "scanners": [
    {"id":"builtin.regex","matched":["pci.visa"],"latency_ms":12},
    {"id":"nightfall","matched":["CC_NUMBER"],"latency_ms":411}
  ],
  "mode": "strict",
  "policy_version": "dlp-2026-04-15"
}
```

- Chain link covers the whole record; post-hoc edits are
  detectable.
- Quarantine events additionally record the quarantine path and
  sidecar digest.
- Matched **content** is **never** written — only symbolic rule
  ids.

## 8. Onboarding

**Minimal enterprise rollout checklist:**

1. Decide policy: `audit_only` first, `balanced` after
   calibration, `strict` only when false-positive rate is
   acceptable.
2. Generate and publish a signing key for policy bundles; import
   the public key into the daemon's `signing_keyring`.
3. Publish a v1 policy bundle through MDM (Intune, Jamf,
   Ansible, SCCM) into `bundle_dir`.
4. Per-tenant scanner enablement: start with
   `builtin.regex` + `builtin.entropy`; add third-party
   connectors only after bridge creds are vaulted in the OS
   keyring.
5. Turn on `audit_only`; run for one sprint; review
   `PluginAuditEvent::UploadScan` records in SIEM.
6. Promote to `balanced`; review quarantine queue.
7. Promote to `strict` only once residency (`data-residency.md`)
   is also `strict`.
8. Wire `PluginAuditEvent::UploadScan` + `PolicyRotated` into
   SIEM (Splunk, Elastic, Sentinel) via
   `PluginCapability::ObserveStatus`.

## 9. Verification

Planned (not yet landed):

- Unit tests for each built-in scanner.
- Integration tests against `pcloud-mockserver` covering every
  verdict, timeout behaviour per mode, and redaction handling.
- Tamper tests on signed policy bundles.
- Negative test: `strict` DLP + `strict` residency with no
  scanner enabled must refuse-to-start.
- Reference Nightfall bridge plugin as a signed plugin example.

Until this wave lands, DLP is specified, not demonstrated.

## 10. Failure modes

| Failure | Behaviour |
| --- | --- |
| Scanner times out | Per §4.7 matrix; `strict` → Deny |
| Scanner returns 5xx | Per §4.7 matrix; `strict` → Deny |
| Plugin registry loses track of scanner | Per §4.7 matrix; `strict` → Deny, others → Allow + flag |
| Signed policy bundle signature fails | Daemon keeps the last known-good bundle and logs `policy.bundle.signature_failed` |
| Bundle tries to downgrade `policy_version` | Refused unless `allow_downgrade = true` |
| Stream-access plugin without signed manifest | Refused at startup; daemon logs and drops scanner |
| Third-party bridge credential missing from keyring | Scanner disabled; audit record notes reason |
| Matched content somehow reaches audit record | **Bug**; this must never happen. Tests must include fuzz vectors |

## 11. Honest limitations

pre-alpha reality check:

- **Nothing is shipped for the enterprise wave.** Design only.
  Every wire shape above is a placeholder and may change.
- **The in-tree built-in plugin** (`pcloud-plugin-dlp`) is
  **single-user path-hash-only** and is **not** an enterprise
  control. See §13.
- **Central policy distribution** assumes operators already run
  an MDM / config-management system. Bootstrapping the signing
  key is operator-side; pcloud-rs does not ship a key-rotation
  flow.
- **Crypto-folder ciphertext inspection** is a non-goal.
  Scanning must happen pre-encryption; this is an intentional
  architecture choice, not a gap.
- **Per-file verdict has no appeal flow** beyond quarantine.
  End-users cannot override `Deny`; only operators can.
- **Third-party scanner SLAs are not the daemon's problem.**
  `strict` mode trades availability for containment; operators
  own that decision.

## 12. Extension points

- **New built-in scanner** — add a module under
  `pcloud-daemon/src/dlp/` implementing `DlpScanner`; register
  it in the built-in scanner list.
- **New third-party connector** — ship as a signed external
  plugin declaring `UploadInspect`; config wires it by name.
- **Streaming mode** — plugins declare `UploadStreamAccess` and
  host flips to chunked body after signature + TLS check.
- **Policy schema evolution** — bundles carry
  `policy_version`; daemon supports N and N-1 simultaneously
  during rotation.

## 13. Boundary — enterprise DLP vs built-in `pcloud-plugin-dlp`

These are **two different things**. The table makes the
boundary explicit:

| Aspect | Built-in `pcloud-plugin-dlp` | Enterprise DLP (this page) |
| --- | --- | --- |
| Status | Shipped (single-user) | Design-only |
| Audience | Solo user who wants a simple deny-list | IT / security team operating a fleet |
| Input | Path + content hash only | Path, size, hash, mime, first bytes, optional full stream |
| Scanners | None — path/hash match only | Regex, entropy, third-party connectors |
| Policy | Local static config | Signed centrally-distributed bundles |
| Fail-safe modes | None (single decision) | `strict` / `balanced` / `audit_only` |
| Audit | Local log | Chain-hashed signed audit events |
| SIEM export | None | `PluginCapability::ObserveStatus` |
| Suitable for regulated industries | **No** | Yes, once landed |

Operators deploying an enterprise posture should treat the
built-in plugin as **absent** for their purposes — it does not
meet enterprise audit, policy, or connector requirements and is
not meant to.

## 14. Cross-refs

Code (design targets):

- `crates/pcloud-plugin-api/src/lib.rs` — `PluginOperation`,
  `PluginCapability`, `PluginAuditSink`.
- `crates/pcloud-daemon/src/transfer_backend.rs` —
  `upload_create` / `upload_write` / `upload_save` is the call
  site.
- `crates/pcloud-config/` — `[upload.dlp]` section.
- `crates/pcloud-observability/` — audit chain,
  `PluginAuditEvent::UploadScan`,
  `PluginAuditEvent::PolicyRotated`.
- `crates/pcloud-plugin-dlp/` — the **single-user** built-in
  plugin (see §13).
- `crates/pcloud-daemon/src/dlp/` — future home of built-in
  enterprise scanners.

Related docs:

- `docs/enterprise/data-residency.md` — cross-constraint in §4.7
  (`residency.strict ⇒ dlp.mode = strict`).
- `docs/enterprise/disaster-recovery.md` — audit-chain
  invariants that restore must preserve.
- `docs/enterprise/tracing.md` — scanner call spans under
  `pcloudd.backend.transfer` when tracing is on.
- `docs/enterprise/policy.md` — fleet policy push story that
  carries signed DLP bundles.
- `docs/enterprise/kms.md` — crypto-folder scanning invariant
  (pre-encryption).
