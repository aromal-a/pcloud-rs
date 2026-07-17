# `pcloud-plugin-dlp`

**Maturity:** Experimental / bounded

**Version:** `0.8.1-beta`

**Directory:** `crates/pcloud-plugin-dlp`

**Manifest:** [`crates/pcloud-plugin-dlp/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/Cargo.toml)

Built-in pre-upload DLP scanner plugin (regex + Shannon entropy) for pcloud-rs.

## Feature-family profile

**Why it exists.** Inspect outbound content before upload so obvious secrets can be blocked or reviewed.

**What it is good for.** Built-in regex and Shannon-entropy scanning with findings and policy decisions.

**Why it is good at that job.** Bounded scanning, explicit rules, and pre-upload placement provide useful local detection without granting network access.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_plugin_dlp` | lib | [`crates/pcloud-plugin-dlp/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs) |

## Direct dependencies

`pcloud-plugin-api`, `pcloud-secret`, `regex`, `serde`, `sha2`, `thiserror`

## Cargo features

No declared package features.

## File inventory (3)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-plugin-dlp/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-plugin-dlp/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/README.md) | documentation | pcloud-plugin-dlp |
| [`crates/pcloud-plugin-dlp/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs) | library root | Built-in pre-upload DLP (data-loss-prevention) scanner plugin. |

## Rust declaration index (36 total; 18 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `DlpError` | `pub` | enum | [`crates/pcloud-plugin-dlp/src/lib.rs:44`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L44) | Errors returned by \[`DlpScanner`\]. |
| `DlpConfig` | `pub` | struct | [`crates/pcloud-plugin-dlp/src/lib.rs:59`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L59) | Configuration block for the DLP plugin. Typically deserialised from the `\[plugins.dlp\]` section of the daemon… |
| `default_true` | `private` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:76`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L76) | Read the source/rustdoc for the exact contract. |
| `default_timeout_ms` | `private` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:80`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L80) | Read the source/rustdoc for the exact contract. |
| `default` | `private` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:85`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L85) | Read the source/rustdoc for the exact contract. |
| `rule_enabled` | `pub` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:99`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L99) | Whether a specific rule is enabled. Unknown rule IDs default to enabled. |
| `DlpAuditEvent` | `pub` | struct | [`crates/pcloud-plugin-dlp/src/lib.rs:109`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L109) | Non-secret audit event emitted by the DLP plugin. Contains a path *hash* rather than the raw path, never the… |
| `DlpScanResult` | `pub` | struct | [`crates/pcloud-plugin-dlp/src/lib.rs:120`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L120) | Combined result of a single scan. |
| `rule_ids` | `pub` | mod | [`crates/pcloud-plugin-dlp/src/lib.rs:129`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L129) | Known built-in rule identifiers. Kept stable for config + audit. |
| `AWS_ACCESS_KEY` | `pub` | const | [`crates/pcloud-plugin-dlp/src/lib.rs:131`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L131) | Read the source/rustdoc for the exact contract. |
| `AWS_SECRET_KEY` | `pub` | const | [`crates/pcloud-plugin-dlp/src/lib.rs:132`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L132) | Read the source/rustdoc for the exact contract. |
| `PRIVATE_KEY_PEM` | `pub` | const | [`crates/pcloud-plugin-dlp/src/lib.rs:133`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L133) | Read the source/rustdoc for the exact contract. |
| `JWT` | `pub` | const | [`crates/pcloud-plugin-dlp/src/lib.rs:134`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L134) | Read the source/rustdoc for the exact contract. |
| `GENERIC_PASSWORD_LITERAL` | `pub` | const | [`crates/pcloud-plugin-dlp/src/lib.rs:135`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L135) | Read the source/rustdoc for the exact contract. |
| `HIGH_ENTROPY` | `pub` | const | [`crates/pcloud-plugin-dlp/src/lib.rs:136`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L136) | Read the source/rustdoc for the exact contract. |
| `CompiledRule` | `private` | struct | [`crates/pcloud-plugin-dlp/src/lib.rs:139`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L139) | Read the source/rustdoc for the exact contract. |
| `DlpScanner` | `pub` | struct | [`crates/pcloud-plugin-dlp/src/lib.rs:145`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L145) | DLP scanner implementing the B5 pre-upload hook. |
| `new` | `pub` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:153`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L153) | Build a new scanner from its \[`DlpConfig`\]. |
| `required_capability` | `pub` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:185`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L185) | The capability this plugin needs. Always \[`PluginCapability::ObserveStatus`\]. |
| `scan` | `pub` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:190`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L190) | Perform a scan of a \[`PluginOperation::PreUploadScan`\] payload. |
| `compile` | `private` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:258`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L258) | Read the source/rustdoc for the exact contract. |
| `hash_path` | `private` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:262`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L262) | Read the source/rustdoc for the exact contract. |
| `shannon_entropy` | `pub` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:275`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L275) | Compute Shannon entropy in bits/byte over `buf`. |
| `is_known_binary_magic` | `pub` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:299`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L299) | True if `buf` starts with a known compressed or binary-container magic signature for which high entropy is ex… |
| `MAGICS` | `private` | const | [`crates/pcloud-plugin-dlp/src/lib.rs:300`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L300) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-plugin-dlp/src/lib.rs:321`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L321) | Read the source/rustdoc for the exact contract. |
| `op` | `private` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:324`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L324) | Read the source/rustdoc for the exact contract. |
| `detects_aws_access_key_in_first_bytes` | `private` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:335`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L335) | Read the source/rustdoc for the exact contract. |
| `detects_private_key_pem_header` | `private` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:348`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L348) | Read the source/rustdoc for the exact contract. |
| `high_entropy_random_buffer_triggers_rule` | `private` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:361`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L361) | Read the source/rustdoc for the exact contract. |
| `known_compressed_magic_skips_entropy_rule` | `private` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:384`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L384) | Read the source/rustdoc for the exact contract. |
| `strict_mode_returns_deny_on_match_else_allow` | `private` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:403`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L403) | Read the source/rustdoc for the exact contract. |
| `audit_only_mode_returns_allow_but_emits_event` | `private` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:421`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L421) | Read the source/rustdoc for the exact contract. |
| `rejects_non_preupload_operation` | `private` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:435`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L435) | Read the source/rustdoc for the exact contract. |
| `disabled_plugin_always_allows_no_rules` | `private` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:442`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L442) | Read the source/rustdoc for the exact contract. |
| `per_rule_disable_suppresses_match` | `private` | fn | [`crates/pcloud-plugin-dlp/src/lib.rs:457`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-dlp/src/lib.rs#L457) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.
