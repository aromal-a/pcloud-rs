# `pcloud-plugin-api`

**Maturity:** Experimental / bounded

**Version:** `0.1.0`

**Directory:** `crates/pcloud-plugin-api`

**Manifest:** [`crates/pcloud-plugin-api/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/Cargo.toml)

Plugin manifest, signature verification (ed25519), and registry API for pcloud-rs.

## Feature-family profile

**Why it exists.** Define a capability-limited extension contract before loading third-party code.

**What it is good for.** Signed manifests, registry/lifecycle metadata, operation/response messages, capabilities, and audit events.

**Why it is good at that job.** Ed25519 verification, explicit capability grants, size/version checks, and secret-free messages make extension authority reviewable.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_plugin_api` | lib | [`crates/pcloud-plugin-api/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs) |

## Direct dependencies

`ed25519-dalek`, `pcloud-config`, `serde`, `serde_json`, `sha2`, `thiserror`

## Cargo features

No declared package features.

## File inventory (3)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-plugin-api/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-plugin-api/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/README.md) | documentation | pcloud-plugin-api |
| [`crates/pcloud-plugin-api/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs) | library root | pcloud-plugin-api |

## Rust declaration index (91 total; 27 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `CRATE_NAME` | `pub` | const | [`crates/pcloud-plugin-api/src/lib.rs:134`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L134) | Canonical crate identifier, used in structured logs and telemetry. |
| `PluginCapability` | `pub` | enum | [`crates/pcloud-plugin-api/src/lib.rs:155`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L155) | Capabilities a plugin can request. Each capability gates a disjoint set of \[`PluginOperation`\] variants. Capa… |
| `required_for` | `pub` | fn | [`crates/pcloud-plugin-api/src/lib.rs:170`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L170) | The capability required to execute a given operation kind. |
| `PluginManifest` | `pub` | struct | [`crates/pcloud-plugin-api/src/lib.rs:196`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L196) | Declarative plugin descriptor. The canonical byte form for signing is produced by \[`PluginManifest::canonical… |
| `canonical_bytes` | `pub` | fn | [`crates/pcloud-plugin-api/src/lib.rs:216`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L216) | Canonical serialization used as the ed25519 message. Format: `sha256("pcloud-plugin-manifest-v1" \|\| serde_jso… |
| `PluginSignature` | `pub` | struct | [`crates/pcloud-plugin-api/src/lib.rs:231`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L231) | Optional ed25519 signature over \[`PluginManifest::canonical_bytes`\]. Serde representation uses lowercase hex… |
| `serialize` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:239`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L239) | Read the source/rustdoc for the exact contract. |
| `deserialize` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:249`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L249) | Read the source/rustdoc for the exact contract. |
| `Wire` | `private` | struct | [`crates/pcloud-plugin-api/src/lib.rs:251`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L251) | Read the source/rustdoc for the exact contract. |
| `hex_encode` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:267`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L267) | Read the source/rustdoc for the exact contract. |
| `LUT` | `private` | const | [`crates/pcloud-plugin-api/src/lib.rs:268`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L268) | Read the source/rustdoc for the exact contract. |
| `hex_decode_fixed` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:277`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L277) | Read the source/rustdoc for the exact contract. |
| `hex_nibble` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:289`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L289) | Read the source/rustdoc for the exact contract. |
| `PluginOperation` | `pub` | enum | [`crates/pcloud-plugin-api/src/lib.rs:305`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L305) | Typed operations a plugin can request from the host. The host validates the capability requirement for each o… |
| `UploadScanVerdict` | `pub` | enum | [`crates/pcloud-plugin-api/src/lib.rs:395`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L395) | Verdict returned by a DLP / content scanning plugin in response to a \[`PluginOperation::PreUploadScan`\]. Non-… |
| `FileIntegrityOutcome` | `pub` | enum | [`crates/pcloud-plugin-api/src/lib.rs:411`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L411) | Outcome of a single file-integrity check reported by the host's checksum scanner. Non-secret coarse-grained s… |
| `FileIntegrityResult` | `pub` | struct | [`crates/pcloud-plugin-api/src/lib.rs:424`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L424) | A single file-integrity event. Streamed to plugins that hold the \[`PluginCapability::ObserveStatus`\] capabili… |
| `PluginOperationResponse` | `pub` | enum | [`crates/pcloud-plugin-api/src/lib.rs:439`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L439) | Typed responses. The host never hands SecretString / SecretBytes / AuthVault references back to a plugin. |
| `PublinkSummary` | `pub` | struct | [`crates/pcloud-plugin-api/src/lib.rs:483`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L483) | Redacted, non-secret summary of a single public link the host exposes to observer plugins. Only fields explic… |
| `Plugin` | `pub` | trait | [`crates/pcloud-plugin-api/src/lib.rs:533`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L533) | The host-facing plugin contract. Implementations MUST be `Send`. # Interaction model Plugins are **pull-drive… |
| `manifest` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:536`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L536) | Return the plugin's declarative manifest. Called by the registry at registration time and must be stable for… |
| `signature` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:541`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L541) | Optional signature for the manifest. Returning `None` means the plugin is unsigned. The registry rejects unsi… |
| `on_load` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:548`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L548) | Called once after the registry has validated the manifest, verified the signature (if required), and resolved… |
| `next_operation` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:554`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L554) | Optional — plugins return the next operation they would like the host to execute, or `None` when idle. The de… |
| `on_response` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:560`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L560) | Delivered to the plugin after `PluginRegistry::invoke` produced a response. Default is a no-op. |
| `PluginContext` | `pub` | struct | [`crates/pcloud-plugin-api/src/lib.rs:572`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L572) | Redacted view of the host runtime handed to the plugin at load time. Contains strictly non-secret data. No `S… |
| `PluginAuditEvent` | `pub` | enum | [`crates/pcloud-plugin-api/src/lib.rs:591`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L591) | Audit event kinds the registry emits. Hosts are expected to forward these into their tamper-evident audit log. |
| `PluginAuditSink` | `pub` | trait | [`crates/pcloud-plugin-api/src/lib.rs:661`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L661) | Host-provided audit logger. The default \[`NullAuditSink`\] drops events — the daemon should wire this into its… |
| `record` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:665`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L665) | Record a single plugin-related audit event. Implementations must not panic; durable persistence failures shou… |
| `NullAuditSink` | `pub` | struct | [`crates/pcloud-plugin-api/src/lib.rs:670`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L670) | No-op sink. Production hosts MUST replace this with a real sink. |
| `record` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:673`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L673) | Read the source/rustdoc for the exact contract. |
| `RegisteredPlugin` | `pub` | struct | [`crates/pcloud-plugin-api/src/lib.rs:682`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L682) | Snapshot of a successfully-registered plugin kept inside the registry. |
| `PluginError` | `pub` | enum | [`crates/pcloud-plugin-api/src/lib.rs:720`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L720) | Error returned by plugin registration and invocation. # `#\[non_exhaustive\]` rationale This enum is intentiona… |
| `PluginRegistry` | `pub` | struct | [`crates/pcloud-plugin-api/src/lib.rs:777`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L777) | In-memory registry of loaded plugins. The registry is the single entry point for both registration (which gat… |
| `new` | `pub` | fn | [`crates/pcloud-plugin-api/src/lib.rs:784`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L784) | Construct an empty registry. |
| `register` | `pub` | fn | [`crates/pcloud-plugin-api/src/lib.rs:798`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L798) | Register a plugin. Enforces, in order: 1. Policy enables plugins at all. 2. Manifest fields are well-formed.… |
| `loaded_plugins` | `pub` | fn | [`crates/pcloud-plugin-api/src/lib.rs:877`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L877) | Return the slice of currently-registered plugins in registration order. |
| `get` | `pub` | fn | [`crates/pcloud-plugin-api/src/lib.rs:883`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L883) | Look up a registered plugin by id. |
| `authorize` | `pub` | fn | [`crates/pcloud-plugin-api/src/lib.rs:890`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L890) | Enforce capability for a proposed operation. Returns the required capability on success. Records an audit ent… |
| `dispatch` | `pub` | fn | [`crates/pcloud-plugin-api/src/lib.rs:957`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L957) | Capability-gated, panic-guarded dispatch. This is the **single enforcement point** every host dispatcher MUST… |
| `deregister` | `pub` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1003`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1003) | Explicitly de-register a plugin. Returns `true` when a plugin with `plugin_id` was present and removed. Emits… |
| `deregister_internal` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1012`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1012) | Read the source/rustdoc for the exact contract. |
| `validate_manifest` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1032`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1032) | Read the source/rustdoc for the exact contract. |
| `granted_capabilities` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1048`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1048) | Read the source/rustdoc for the exact contract. |
| `verify_signature` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1069`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1069) | Returns `(signed, trusted_key_fingerprint)`. |
| `is_trusted_key` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1102`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1102) | Read the source/rustdoc for the exact contract. |
| `operation_label` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1108`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1108) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-plugin-api/src/lib.rs:1129`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1129) | Read the source/rustdoc for the exact contract. |
| `CapturingAudit` | `private` | struct | [`crates/pcloud-plugin-api/src/lib.rs:1143`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1143) | Read the source/rustdoc for the exact contract. |
| `record` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1148`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1148) | Read the source/rustdoc for the exact contract. |
| `ObservePlugin` | `private` | struct | [`crates/pcloud-plugin-api/src/lib.rs:1192`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1192) | Read the source/rustdoc for the exact contract. |
| `manifest` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1197`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1197) | Read the source/rustdoc for the exact contract. |
| `signature` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1205`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1205) | Read the source/rustdoc for the exact contract. |
| `on_load` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1208`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1208) | Read the source/rustdoc for the exact contract. |
| `SyncPlugin` | `private` | struct | [`crates/pcloud-plugin-api/src/lib.rs:1213`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1213) | Read the source/rustdoc for the exact contract. |
| `manifest` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1215`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1215) | Read the source/rustdoc for the exact contract. |
| `on_load` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1226`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1226) | Read the source/rustdoc for the exact contract. |
| `dev_policy` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1231`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1231) | Read the source/rustdoc for the exact contract. |
| `sign_manifest` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1237`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1237) | Read the source/rustdoc for the exact contract. |
| `plugins_disabled_rejects_load` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1249`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1249) | Read the source/rustdoc for the exact contract. |
| `invalid_manifest_is_rejected` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1263`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1263) | Read the source/rustdoc for the exact contract. |
| `BadPlugin` | `private` | struct | [`crates/pcloud-plugin-api/src/lib.rs:1264`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1264) | Read the source/rustdoc for the exact contract. |
| `manifest` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1266`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1266) | Read the source/rustdoc for the exact contract. |
| `on_load` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1274`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1274) | Read the source/rustdoc for the exact contract. |
| `capability_denied_when_policy_refuses` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1290`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1290) | Read the source/rustdoc for the exact contract. |
| `authorize_blocks_ungranted_operation` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1305`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1305) | Read the source/rustdoc for the exact contract. |
| `authorize_allows_granted_operation` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1334`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1334) | Read the source/rustdoc for the exact contract. |
| `unsigned_plugin_rejected_in_prod_mode` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1357`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1357) | Read the source/rustdoc for the exact contract. |
| `signed_plugin_accepted_in_prod_mode` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1373`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1373) | Read the source/rustdoc for the exact contract. |
| `signed_plugin_with_untrusted_key_rejected` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1403`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1403) | Read the source/rustdoc for the exact contract. |
| `tampered_signature_rejected` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1424`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1424) | Read the source/rustdoc for the exact contract. |
| `dev_mode_unsigned_load_warns_in_audit` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1445`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1445) | Read the source/rustdoc for the exact contract. |
| `plugin_context_contains_no_secret_types` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1457`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1457) | Read the source/rustdoc for the exact contract. |
| `observe_publink_list_requires_observe_status` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1476`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1476) | Read the source/rustdoc for the exact contract. |
| `timer_tick_requires_observe_status` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1484`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1484) | Read the source/rustdoc for the exact contract. |
| `authorize_allows_observe_publink_list_with_observe_status` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1492`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1492) | Read the source/rustdoc for the exact contract. |
| `unknown_plugin_authorize_errors` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1509`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1509) | Read the source/rustdoc for the exact contract. |
| `DlpShapedPlugin` | `private` | struct | [`crates/pcloud-plugin-api/src/lib.rs:1525`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1525) | A DLP-shaped plugin whose manifest requests an *empty* capability set — i.e. `ObserveStatus` has been revoked… |
| `manifest` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1527`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1527) | Read the source/rustdoc for the exact contract. |
| `on_load` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1536`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1536) | Read the source/rustdoc for the exact contract. |
| `dispatch_blocks_dlp_scan_without_observe_status` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1542`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1542) | Read the source/rustdoc for the exact contract. |
| `AutohealShapedPlugin` | `private` | struct | [`crates/pcloud-plugin-api/src/lib.rs:1586`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1586) | An autoheal-shaped plugin that only requested `ObserveStatus`: it can observe integrity events, but MUST NOT… |
| `manifest` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1588`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1588) | Read the source/rustdoc for the exact contract. |
| `on_load` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1596`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1596) | Read the source/rustdoc for the exact contract. |
| `dispatch_blocks_autoheal_quarantine_without_sync_control` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1602`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1602) | Read the source/rustdoc for the exact contract. |
| `dispatch_runs_handler_when_capability_granted` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1633`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1633) | Read the source/rustdoc for the exact contract. |
| `dispatch_unknown_plugin_short_circuits` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1658`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1658) | Read the source/rustdoc for the exact contract. |
| `dispatch_catches_handler_panic_and_deregisters` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1675`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1675) | Read the source/rustdoc for the exact contract. |
| `explicit_deregister_removes_plugin_and_audits` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1730`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1730) | Read the source/rustdoc for the exact contract. |
| `dispatch_denies_network_probe_without_network_egress` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1757`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1757) | Read the source/rustdoc for the exact contract. |
| `dispatch_denies_crypto_query_without_crypto_capability` | `private` | fn | [`crates/pcloud-plugin-api/src/lib.rs:1779`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-api/src/lib.rs#L1779) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.
