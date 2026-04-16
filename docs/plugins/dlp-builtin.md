# Built-in DLP Pre-Upload Scanner Plugin

Crate: `pcloud-plugin-dlp` (wave H10)

## 1. Purpose

`pcloud-plugin-dlp` ships the first in-tree plugin that runs
**synchronously on the pre-upload path** to detect obvious secret
material before files leave the host. Unlike the three other
first-party plugins (which are observational or scheduler-driven),
DLP sits on the hot path: the daemon does not begin an upload until
the plugin has returned a verdict or hit the configured timeout.

It is intentionally small, opinionated, and single-user. See
[`pcloud-plugin-dlp-enterprise`](../enterprise/dlp.md) for custom
rulesets, tenant policy, audit stream forwarding, and whole-file
scanning.

## 2. Why this plugin exists — the incident class it prevents

Every week, someone on every team drags one of these files into their
pCloud sync folder:

- `.env` with `AWS_ACCESS_KEY_ID=AKIA…` and the matching secret on the
  next line.
- `id_rsa`, `id_ed25519`, or a `.pem` file dumped into `~/Documents`
  for "temporary safekeeping".
- a JWT pasted into a `scratchpad.txt` while debugging an auth flow.
- a `tokens.json` export from a browser session.
- a Git bundle containing repo history that includes long-since-rotated
  keys the scrubber forgot.

None of these are attacks. They are mistakes. Catching them at the
moment of upload — before the bytes cross the network — removes the
window in which a public link could ever be issued for the offending
file. The scanner is tuned so that **audit-only mode is the default**,
because a DLP that says "no" to a user's upload without a warm-up
period is a DLP that gets turned off.

## 3. Capabilities

| Capability        | Required | Purpose                                       |
|-------------------|:--------:|-----------------------------------------------|
| `ObserveStatus`   | yes      | Only capability the plugin needs.             |
| `SyncControl`     | no       | Scanner does not pause/resume sync.           |
| `CryptoControl`   | no       | Scanner never touches key material.           |
| `NetworkEgress`   | no       | Scanner never initiates network traffic.      |

No extra `PCLOUD_PLUGIN_ALLOW_*` env flag is needed; the master
`PCLOUD_PLUGINS_ENABLED=1` is enough.

**Runtime-gated enforcement.** `ObserveStatus` is enforced inside
`PluginRegistry::dispatch` on every pre-upload scan call. If the
operator revokes the capability (by removing it from the plugin
manifest set in config, or by disabling the runtime), the DLP
scanner's `PreUploadScan` handler is **never** invoked — the registry
emits `plugin.capability.denied{plugin=dlp, op=pre_upload_scan,
missing=ObserveStatus}` and the upload proceeds on the unscanned path
(treat revocation as an audited "no DLP coverage" decision, not as a
quiet pass-through). A panicking scan handler is caught by the
registry and the DLP plugin is de-registered for the rest of the
daemon's lifetime; restart required.

## 4. Configuration reference

`[plugins.dlp]` in `pcloud.conf`:

| Key             | Type            | Default | Validation                                    | Purpose                               |
|-----------------|-----------------|--------:|-----------------------------------------------|---------------------------------------|
| `enabled`       | bool            | `true`  | —                                             | Master switch.                        |
| `strict_mode`   | bool            | `false` | —                                             | `false` = audit-only, `true` = enforce. |
| `timeout_ms`    | u32             | `5000`  | `> 0`; host truncates > 30 000                | Hard upper bound per scan.            |
| `[plugins.dlp.rules]` | table<bool> | `{}`    | Keys must be one of the 6 rule IDs            | Per-rule on/off overrides.            |

Rule IDs recognised under `[plugins.dlp.rules]`:

- `AWS_ACCESS_KEY`
- `AWS_SECRET_KEY`
- `PRIVATE_KEY_PEM`
- `JWT`
- `GENERIC_PASSWORD_LITERAL`
- `HIGH_ENTROPY`

Unlisted rule IDs default to `true` (enabled). An unknown key is
rejected by the config parser.

Example `pcloud.conf`:

```toml
[plugins.dlp]
enabled      = true
strict_mode  = false
timeout_ms   = 5000

[plugins.dlp.rules]
AWS_ACCESS_KEY = true
HIGH_ENTROPY   = false     # noisy on photo sync roots
```

## 5. Lifecycle + event flow

```
                 upload initiated                 upload bytes leave host
 ┌──────────────────────┐                                   │
 │ pcloud-fs / transfer │                                   │
 └──────────┬───────────┘                                   │
            │ PreUploadScan { path, size, content_hash,     │
            │   first_bytes ≤ 4 KiB, mime_guess }           │
            ▼                                               │
     ┌──────────────┐                                       │
     │ DLP scanner  │─── timeout_ms budget ─────┐           │
     └──────┬───────┘                           │           │
            │ DlpScanResult { verdict, audit }  │           │
            ▼                                   ▼           │
     ┌──────────────┐              ┌───────────────────┐    │
     │ audit sink   │◀── HMAC ──── │ host fallback:    │    │
     │  (append)    │              │   allow + audit   │    │
     └──────┬───────┘              └─────────┬─────────┘    │
            │                                │              │
            └──────── combined verdict ──────┴──────────────▶
                             │
                             ▼
                   Allow  → upload proceeds
                   Deny   → upload rejected (strict only)
                   Other  → reserved; not emitted today
```

- `first_bytes` is capped by the host at 4 KiB.
- `path` is hashed (`SHA-256`, hex) before anything is logged.
- `content_hash` is supplied by the host so the scanner never re-hashes
  the file.
- Timeout is enforced by the host; the plugin itself is synchronous
  inside the budget.

## 6. Rule taxonomy

The scanner ships with **six rules**. Each is a lexical match against
`first_bytes` (plus, in one case, entropy).

| Rule ID                     | Detection                                                              | False-positive class                           |
|-----------------------------|------------------------------------------------------------------------|------------------------------------------------|
| `AWS_ACCESS_KEY`            | Regex `AKIA[0-9A-Z]{16}`                                               | Very rare; AWS's own prefix is unique.         |
| `AWS_SECRET_KEY`            | "secret" keyword within 40 chars of a 40+ char base64-ish token        | Config files using the word "secret" as label. |
| `PRIVATE_KEY_PEM`           | Regex `-----BEGIN .* PRIVATE KEY-----`                                 | None that are not also real PEM material.      |
| `JWT`                       | Regex `eyJ[…].eyJ[…].[…]`                                              | Base64 structures that happen to split 3-way.  |
| `GENERIC_PASSWORD_LITERAL`  | Case-insensitive `password[=:]\s*\S{8,}`                               | Tutorials, docs, shell examples.               |
| `HIGH_ENTROPY`              | Shannon entropy of `first_bytes` > 7.5 bits/byte, no known magic       | Encrypted archives, novel compressed formats.  |

### HIGH_ENTROPY magic-number suppression

`HIGH_ENTROPY` is suppressed when `first_bytes` starts with one of 15
recognised compressed / media magic numbers. Otherwise, every
`.jpg`, `.mp4`, or `.zip` in a photo sync root would trip the rule.

The suppression list (exact prefixes checked by
`is_known_binary_magic`):

1. `gzip` (`0x1F 0x8B`)
2. `zip`  (`PK\x03\x04`, `PK\x05\x06`, `PK\x07\x08`)
3. `jpeg` (`0xFF 0xD8 0xFF`)
4. `png`  (`\x89PNG\r\n\x1A\n`)
5. `bzip2` (`BZh`)
6. `xz`   (`\xFD 7zXZ`)
7. `7z`   (`7z\xBC\xAF\x27\x1C`)
8. `ogg`  (`OggS`)
9. `flac` (`fLaC`)
10. `pdf`  (`%PDF-`)
11. `RIFF` (`RIFF` + `WAVE` / `AVI ` / `WEBP`)
12. `GIF8` (`GIF87a` / `GIF89a`)
13. `mp4/ftyp` (offset-4 `ftyp`)
14. `mkv/webm` (`\x1A\x45\xDF\xA3`)
15. `zstd` (`\x28\xB5\x2F\xFD`)

Encrypted archives (e.g. `7z` with `-mhe=on`) typically *pass* magic
suppression and therefore still trip `HIGH_ENTROPY` — which is usually
the correct outcome: encrypted blobs leaving the host should at least
be audited.

## 7. Performance and path-hash audit

### Scan prefix size (throughput knob)

The plugin always sees *at most* the first 4 KiB of a file. The host
truncates `first_bytes` to that bound regardless of file size. Scan
cost is therefore O(4 KiB) per file, independent of file size — a scan
completes in single-digit milliseconds on any modern hardware.

### Path-hash-only audit

```rust
pub struct DlpAuditEvent {
    pub path_hash: String,      // SHA-256(path), hex — never the raw path
    pub rule_ids: Vec<String>,
    pub verdict: UploadScanVerdict,
}
```

The plugin **never** writes:

- the raw file path,
- any byte of the file contents,
- `first_bytes` or any slice thereof,
- the regex match text.

Only the SHA-256 path hash, the matched rule IDs, and the verdict ever
reach the audit stream. This keeps the DLP audit log safe to forward
to central logging without re-leaking the very data the scanner is
trying to protect.

## 8. Outputs

- **Audit log entry** per scan (`DlpAuditEvent`), HMAC-chained by the
  host's audit engine.
- **Daemon log line** at `info` for hits, `debug` for clean scans.
- **No desktop notification** (DLP is silent; hits go to the audit log).
- **IPC response** in strict mode: the upload RPC fails with a
  `Denied` verdict; `pcloudc pending` shows the denial.

## 9. Test recipes

### Unit tests (built-in)

```bash
cargo test -p pcloud-plugin-dlp
```

Covered: AWS access-key detection, PEM private-key header, high-entropy
random buffer trip, known compressed magic suppression, strict-mode
deny, audit-only allow, per-rule disable, path-hash redaction
invariant.

### Manual verification with a scratch file

```bash
# 1. make sure DLP audits upload attempts
export PCLOUD_PLUGINS_ENABLED=1

cat > /tmp/fake.env <<'EOF'
AWS_ACCESS_KEY_ID=AKIA1234567890ABCDEF
AWS_SECRET_ACCESS_KEY=abcdef1234567890abcdef1234567890abcdef12
EOF

# 2. drop it into a sync root, trigger an upload cycle
pcloudc sync resume <sync_root_id>

# 3. verify DLP audit record via the field selector
pcloudc --field rule_ids --json pending

# 4. inspect the audit log (path_hash only, no path)
pcloudc --json audit verify | jq '.[] | select(.kind == "dlp")'
```

The `rule_ids` field should include `AWS_ACCESS_KEY` (and possibly
`AWS_SECRET_KEY`). No raw path is printed anywhere.

### Strict-mode regression

```bash
# audit-only first
pcloudc config set plugins.dlp.strict_mode false
pcloudc sync resume <root>   # should succeed, audit logged

# enable strict
pcloudc config set plugins.dlp.strict_mode true
pcloudc sync resume <root>   # should fail with Deny
```

## 10. Failure modes

| Symptom                                       | Cause                                        | Remedy                                            |
|-----------------------------------------------|----------------------------------------------|---------------------------------------------------|
| Upload blocked on a file that is clearly fine | `HIGH_ENTROPY` trip, no magic match          | Disable `HIGH_ENTROPY` in `[plugins.dlp.rules]`.  |
| Scanner timed out, upload auditied as allowed | Extremely slow host or huge regex config     | Raise `timeout_ms` cautiously, file an issue.     |
| Audit log missing entries                     | Plugin disabled or audit sink unreachable    | `pcloudc audit verify`; check `[plugins.dlp].enabled`. |
| False positives on `.git/objects/*`           | Packfiles are high-entropy and not on magic list | Add `.git` to sync ignore list, or disable rule. |
| Strict mode blocking legitimate work          | Rule hit on a tutorial or doc                | Tune `[plugins.dlp.rules]`; roll back to audit-only. |

## 11. Limitations (honest)

- **First 4 KiB only.** A secret that only appears after the first 4
  KiB of a large file will not be caught. Whole-file scans are an
  enterprise feature.
- **Regex-based, not semantic.** The rules match lexical patterns.
  They will miss sophisticated encoded secrets and can false-positive
  on high-entropy compressed payloads whose magic number is not on the
  suppression list. Start in audit-only.
- **No quarantine directory.** The built-in scanner only emits `Allow`
  or `Deny`. Moving bad files into a quarantine directory is an
  enterprise feature.
- **No per-sync-root policy.** The rule set is global. If you need
  different rules per sync root, the enterprise plugin supports that.
- **Strict-mode surprises.** A broken rule config in strict mode
  blocks uploads. Validate in audit-only first.
- **No auto-remediation.** The plugin never deletes, overwrites, or
  moves the offending file. That is intentional.
- **Single-user scope.** No central policy push, no tenant boundaries.

## 12. Tuning: home deploy vs. FAANG enterprise

| Knob / concern          | Home / single-user                     | Enterprise / fleet                                        |
|-------------------------|----------------------------------------|-----------------------------------------------------------|
| `strict_mode`           | `false` (audit-only) for weeks          | `true` after baseline, with exception workflow in place   |
| `timeout_ms`            | Default `5000`                          | `2000`–`3000` behind a load balancer; watch p99 scan time |
| `HIGH_ENTROPY`          | `false` on photo/video sync roots       | `true` everywhere; rely on magic-number suppression       |
| Audit forwarding        | Local log only                          | Use `pcloud-enterprise-dlp` for centralised SIEM feeds    |
| Rule customisation      | Stock 6 rules                           | Use enterprise plugin for custom detectors                |
| Per-tenant policy       | n/a                                     | Enterprise plugin                                         |
| Operator alerting       | None (desktop user sees their own log)  | Wire audit stream to PagerDuty / Opsgenie                 |

## 13. Security posture

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`.
- No network I/O. No disk I/O outside the pre-upload window handed in
  by the host.
- No raw paths or file content in any log, audit event, or error
  message — only SHA-256 path hashes.
- Audit events are integrity-protected by the host's HMAC-chained
  audit log.
- Hard per-file timeout prevents a runaway regex from wedging the
  upload pipeline; host falls back to the configured `on_timeout`
  policy (default: allow + audit).

## 14. CLI interactions

The plugin does not add subcommands. User-facing touch points:

- `pcloudc pending` — shows uploads queued and, in strict mode,
  uploads blocked by DLP (the audit record carries the `rule_ids`).
- `pcloudc audit verify` / `pcloudc audit-verify` — verifies the
  integrity of the local audit log that carries DLP verdicts.
- `pcloudc doctor` — surfaces recent DLP activity in its diagnostics
  bundle.
- `pcloudc --json pending` returns DLP audit entries in a stable JSON
  envelope. Use the field selector (e.g.
  `pcloudc --field rule_ids pending`) to extract specific values
  without a JSON post-processor.
