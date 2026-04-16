# pcloud-plugin-dlp

Wave H10. First-party, single-user pcloud-rs plugin that scans the
first 4 KiB of every upload for obvious secret material before the
bytes leave the host.

Authoritative user docs:
[`docs/plugins/dlp-builtin.md`](../../docs/plugins/dlp-builtin.md).

## Purpose

Catch everyday mistakes — an AWS key in a `.env` file being dragged
into a sync folder, a `.pem` under `~/Documents`, a JWT pasted into
a scratchpad. Not a full enterprise DLP suite; custom rulesets,
per-tenant policy, and audit-stream forwarding belong to
`pcloud-plugin-dlp-enterprise`.

## Plugin-API ops introduced

- `PluginOperation::PreUploadScan { path, size, content_hash, first_bytes, mime_guess }`

Response type:

- `PluginOperationResponse::UploadScanVerdict(verdict)` where
  `verdict: pcloud_plugin_api::UploadScanVerdict` is one of
  `Allow`, `Deny`, `Quarantine`, or `RedactAndAllow`.

This op runs **synchronously on the upload hot path**. The daemon
will not begin the upload until the plugin has returned a verdict or
hit the configured timeout.

## Capabilities

| Capability        | Required |
|-------------------|:--------:|
| `ObserveStatus`   | yes      |
| `SyncControl`     | no       |
| `CryptoControl`   | no       |
| `NetworkEgress`   | no       |

No extra `PCLOUD_PLUGIN_ALLOW_*` flags needed.

## Configuration knobs

`[plugins.dlp]`:

| Key           | Type | Default | Purpose                                |
|---------------|------|---------|----------------------------------------|
| `enabled`     | bool | `true`  | Master switch.                         |
| `strict_mode` | bool | `false` | `false` = audit-only, `true` = enforce.|
| `timeout_ms`  | u32  | `5000`  | Hard upper bound per scan.             |

`[plugins.dlp.rules]` — per-rule toggles; unlisted rules are on.

## Built-in rules (6)

| Rule ID                     | Detection                                                      |
|-----------------------------|----------------------------------------------------------------|
| `AWS_ACCESS_KEY`            | `AKIA[0-9A-Z]{16}`                                             |
| `AWS_SECRET_KEY`            | "secret" keyword within 40 chars of a 40+ char base64 token    |
| `PRIVATE_KEY_PEM`           | `-----BEGIN .* PRIVATE KEY-----`                               |
| `JWT`                       | `eyJ[...].eyJ[...].[...]`                                      |
| `GENERIC_PASSWORD_LITERAL`  | `password[=:]\s*\S{8,}` (case-insensitive)                     |
| `HIGH_ENTROPY`              | Shannon entropy of first 4 KiB > 7.5 bits/byte                 |

`HIGH_ENTROPY` is suppressed when `first_bytes` starts with one of
15 known compressed/media magic numbers (`gzip`, `zip`, `jpeg`,
`png`, `bzip2`, `xz`, `7z`, `ogg`, `flac`, `pdf`, `RIFF`, `GIF8`,
`mp4/ftyp`, and two extras). This keeps false-positive rates on
photo/video sync roots near zero.

## Modes

- **Audit-only (default):** always returns `Allow`, emits a
  `DlpAuditEvent` for every hit. Run here for 1–2 weeks, tune, then
  consider switching.
- **Strict:** any matched rule returns `Deny`.

## Audit event — path-hash-only

```rust
pub struct DlpAuditEvent {
    pub path_hash: String,      // SHA-256(path), hex — never raw path
    pub rule_ids: Vec<String>,
    pub verdict: UploadScanVerdict,
}
```

The plugin **never** logs:

- the raw path,
- any byte of the file contents,
- `first_bytes` or any slice thereof.

Only the SHA-256 path hash, the matched rule IDs, and the verdict
reach the audit stream. This keeps the DLP audit log safe to
forward to centralised logging without re-leaking the very data the
scanner is trying to protect.

## Security posture

- `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`.
- No network I/O.
- No disk I/O outside the pre-upload window the host hands in.
- No raw paths or content in any log, audit event, or error
  message — only path hashes.
- Hard per-file timeout prevents a runaway regex from wedging the
  upload pipeline; host falls back to the configured `on_timeout`
  policy (default: allow + audit).
- Audit events are integrity-protected by the host's HMAC-chained
  audit log.

## Single-user scope

Global rule set per install. No per-sync-root policy, no tenant
boundaries, no signed central policy push. Those are enterprise
features.

## Honest limitations

- **First 4 KiB only.** A secret that appears after the first 4 KiB
  of a large file is not caught. Whole-file scanning is an
  enterprise feature.
- **Regex-based, not semantic.** The rules match lexical patterns.
  They will miss encoded/encrypted secrets and may false-positive
  on high-entropy payloads whose magic number is not on the
  suppression list. Start in audit-only.
- **No quarantine directory.** The built-in scanner only emits
  `Allow` / `Deny`. Moving bad files into a quarantine directory is
  an enterprise feature.
- **No per-sync-root rules.** Rule set is global.
- **Strict-mode surprises.** Validate rule configs in audit-only
  first; otherwise a misconfigured rule blocks uploads.

## Lifecycle (dev summary)

The crate exposes `DlpScanner::scan(&PluginOperation) -> DlpScanResult`.
The host dispatches `PluginOperation::PreUploadScan` inside the
configured `timeout_ms` budget; the scanner returns a `DlpScanResult`
with the verdict and the audit event (`path_hash`, `rule_ids`,
`verdict`). On timeout the host falls back to the configured
`on_timeout` policy (default: allow + audit).

## Tests

```bash
cargo test -p pcloud-plugin-dlp
```

9 tests covering each rule, magic-number suppression, strict vs
audit-only, timeout fallback, and path-hash redaction invariants.

### Manual smoke test

```bash
export PCLOUD_PLUGINS_ENABLED=1

cat > /tmp/fake.env <<'EOF'
AWS_ACCESS_KEY_ID=AKIA1234567890ABCDEF
EOF

# Drop into a sync root, trigger upload, then inspect audit.
pcloudc sync resume <root>
pcloudc --field rule_ids --json pending
```

The `rule_ids` column should contain `AWS_ACCESS_KEY`. No raw path is
printed anywhere; only the SHA-256 hex path-hash is ever logged.
