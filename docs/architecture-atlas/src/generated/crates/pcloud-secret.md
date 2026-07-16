# `pcloud-secret`

**Maturity:** Internal stable

**Version:** `0.1.0`

**Directory:** `crates/pcloud-secret`

**Manifest:** [`crates/pcloud-secret/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/Cargo.toml)

Zeroize-on-drop secret wrappers (SecretString, SecretBytes) with redacted Debug.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_secret` | lib | [`crates/pcloud-secret/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/lib.rs) |
| `roundtrip` | example | [`crates/pcloud-secret/examples/roundtrip.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/examples/roundtrip.rs) |
| `proptest_zeroize_invariants` | test | [`crates/pcloud-secret/tests/proptest_zeroize_invariants.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/proptest_zeroize_invariants.rs) |
| `redaction_and_zeroize` | test | [`crates/pcloud-secret/tests/redaction_and_zeroize.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/redaction_and_zeroize.rs) |
| `serialize_is_forbidden` | test | [`crates/pcloud-secret/tests/serialize_is_forbidden.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/serialize_is_forbidden.rs) |
| `secret_ct_eq` | bench | [`crates/pcloud-secret/benches/secret_ct_eq.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/benches/secret_ct_eq.rs) |

## Direct dependencies

`criterion`, `proptest`, `subtle`, `zeroize`

## Cargo features

No declared package features.

## File inventory (11)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-secret/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-secret/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/README.md) | documentation | pcloud-secret |
| [`crates/pcloud-secret/benches/secret_ct_eq.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/benches/secret_ct_eq.rs) | benchmark | Constant-time equality micro-benchmarks for `SecretString`. |
| [`crates/pcloud-secret/examples/roundtrip.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/examples/roundtrip.rs) | example | Demonstrates a `SecretString` round-trip: construct, expose, audit-visible |
| [`crates/pcloud-secret/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/lib.rs) | library root | pcloud-secret |
| [`crates/pcloud-secret/src/redact.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/redact.rs) | Rust module | Format a `key=&lt;redacted&gt;` token for inclusion in log lines. |
| [`crates/pcloud-secret/src/secret_bytes.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_bytes.rs) | Rust module | `SecretBytes` — an audit-hardened wrapper around a heap-allocated binary |
| [`crates/pcloud-secret/src/secret_string.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_string.rs) | Rust module | `SecretString` — an audit-hardened wrapper around a heap-allocated UTF-8 |
| [`crates/pcloud-secret/tests/proptest_zeroize_invariants.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/proptest_zeroize_invariants.rs) | test | Property tests for `SecretBytes` / `SecretString` zeroize invariants. |
| [`crates/pcloud-secret/tests/redaction_and_zeroize.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/redaction_and_zeroize.rs) | test | Integration tests for pcloud-secret wrappers. |
| [`crates/pcloud-secret/tests/serialize_is_forbidden.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/serialize_is_forbidden.rs) | test | Enforces that `SecretString` / `SecretBytes` do NOT implement any |

## Rust declaration index (54 total; 15 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `secret_ct_eq` | `private` | fn | [`crates/pcloud-secret/benches/secret_ct_eq.rs:25`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/benches/secret_ct_eq.rs#L25) | Read the source/rustdoc for the exact contract. |
| `main` | `private` | fn | [`crates/pcloud-secret/examples/roundtrip.rs:18`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/examples/roundtrip.rs#L18) | Read the source/rustdoc for the exact contract. |
| `redact` | `pub` | mod | [`crates/pcloud-secret/src/lib.rs:69`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/lib.rs#L69) | Log-line redaction helpers (audit-friendly `key=&lt;redacted&gt;` tokens). |
| `secret_bytes` | `pub` | mod | [`crates/pcloud-secret/src/lib.rs:73`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/lib.rs#L73) | Zeroize-on-drop, redacted-`Debug` wrapper around a binary secret. See the crate-level docs for the full list… |
| `secret_string` | `pub` | mod | [`crates/pcloud-secret/src/lib.rs:77`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/lib.rs#L77) | Zeroize-on-drop, redacted-`Debug` wrapper around a UTF-8 secret. See the crate-level docs for the full list o… |
| `CRATE_NAME` | `pub` | const | [`crates/pcloud-secret/src/lib.rs:84`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/lib.rs#L84) | Crate identifier used in audit/telemetry records. ``` assert_eq!(pcloud_secret::CRATE_NAME, "pcloud-secret");… |
| `SecretMaterial` | `pub` | trait | [`crates/pcloud-secret/src/lib.rs:97`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/lib.rs#L97) | Introspection surface common to every secret wrapper. The only information a non-owner is ever allowed to lea… |
| `expose_len` | `private` | fn | [`crates/pcloud-secret/src/lib.rs:99`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/lib.rs#L99) | Return the byte length of the secret without exposing its content. |
| `ExposeSecret` | `pub` | trait | [`crates/pcloud-secret/src/lib.rs:133`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/lib.rs#L133) | Explicit, audit-visible borrow of the underlying secret. This is the **only** legitimate way to reach the pla… |
| `expose_secret` | `private` | fn | [`crates/pcloud-secret/src/lib.rs:137`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/lib.rs#L137) | Borrow the underlying secret in plaintext. Every call site is intentionally grep-able for audit review; see t… |
| `redact_field` | `pub` | fn | [`crates/pcloud-secret/src/redact.rs:31`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/redact.rs#L31) | Format a `key=&lt;redacted&gt;` token for inclusion in log lines. Use this when a structured log event must record… |
| `SecretBytes` | `pub` | struct | [`crates/pcloud-secret/src/secret_bytes.rs:23`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_bytes.rs#L23) | Zeroize-on-drop, redacted-`Debug` wrapper around a binary secret. |
| `new` | `pub` | fn | [`crates/pcloud-secret/src/secret_bytes.rs:35`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_bytes.rs#L35) | Wrap a binary secret (key material, MAC tag, derived bytes). The buffer is scrubbed when the `SecretBytes` is… |
| `is_empty` | `pub` | fn | [`crates/pcloud-secret/src/secret_bytes.rs:46`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_bytes.rs#L46) | Returns `true` when the underlying buffer has zero length. ``` use pcloud_secret::secret_bytes::SecretBytes;… |
| `clone_secret` | `pub` | fn | [`crates/pcloud-secret/src/secret_bytes.rs:59`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_bytes.rs#L59) | Audit-visible duplication. See \[`crate::secret_string::SecretString::clone_secret`\]. ``` use pcloud_secret::{… |
| `expose_len` | `private` | fn | [`crates/pcloud-secret/src/secret_bytes.rs:65`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_bytes.rs#L65) | Read the source/rustdoc for the exact contract. |
| `expose_secret` | `private` | fn | [`crates/pcloud-secret/src/secret_bytes.rs:71`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_bytes.rs#L71) | Read the source/rustdoc for the exact contract. |
| `fmt` | `private` | fn | [`crates/pcloud-secret/src/secret_bytes.rs:77`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_bytes.rs#L77) | Read the source/rustdoc for the exact contract. |
| `eq` | `private` | fn | [`crates/pcloud-secret/src/secret_bytes.rs:91`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_bytes.rs#L91) | Constant-time equality. Protects MAC-tag and derived-key comparisons from byte-at-a-time timing oracles. ```… |
| `zeroize` | `private` | fn | [`crates/pcloud-secret/src/secret_bytes.rs:99`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_bytes.rs#L99) | Read the source/rustdoc for the exact contract. |
| `SecretString` | `pub` | struct | [`crates/pcloud-secret/src/secret_string.rs:36`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_string.rs#L36) | Secret-bearing UTF-8 wrapper. See module docs for hardening guarantees. Deliberately does not derive `Clone`;… |
| `new` | `pub` | fn | [`crates/pcloud-secret/src/secret_string.rs:48`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_string.rs#L48) | Wrap a UTF-8 secret (password, auth token, 2FA code, ...). The value is zeroized when the `SecretString` is d… |
| `is_empty` | `pub` | fn | [`crates/pcloud-secret/src/secret_string.rs:61`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_string.rs#L61) | Returns `true` when the underlying string has zero length. Safe to log — reveals only emptiness, not content.… |
| `clone_secret` | `pub` | fn | [`crates/pcloud-secret/src/secret_string.rs:78`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_string.rs#L78) | Audit-visible duplication of the secret. Replaces the removed `#\[derive(Clone)\]`. Each invocation doubles the… |
| `expose_len` | `private` | fn | [`crates/pcloud-secret/src/secret_string.rs:84`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_string.rs#L84) | Read the source/rustdoc for the exact contract. |
| `expose_secret` | `private` | fn | [`crates/pcloud-secret/src/secret_string.rs:90`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_string.rs#L90) | Read the source/rustdoc for the exact contract. |
| `fmt` | `private` | fn | [`crates/pcloud-secret/src/secret_string.rs:96`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_string.rs#L96) | Read the source/rustdoc for the exact contract. |
| `eq` | `private` | fn | [`crates/pcloud-secret/src/secret_string.rs:110`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_string.rs#L110) | Constant-time equality. Protects auth-token and password comparisons from byte-at-a-time timing oracles. ```… |
| `zeroize` | `private` | fn | [`crates/pcloud-secret/src/secret_string.rs:121`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/src/secret_string.rs#L121) | Read the source/rustdoc for the exact contract. |
| `prop_secret_bytes_new_exposes_input` | `private` | fn | [`crates/pcloud-secret/tests/proptest_zeroize_invariants.rs:21`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/proptest_zeroize_invariants.rs#L21) | Round-trip invariants: new-then-expose matches the input, and `Debug` never leaks the plaintext content. |
| `prop_secret_string_new_exposes_input` | `private` | fn | [`crates/pcloud-secret/tests/proptest_zeroize_invariants.rs:36`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/proptest_zeroize_invariants.rs#L36) | Read the source/rustdoc for the exact contract. |
| `prop_secret_bytes_ct_eq_matches_bytes_eq` | `private` | fn | [`crates/pcloud-secret/tests/proptest_zeroize_invariants.rs:49`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/proptest_zeroize_invariants.rs#L49) | Constant-time equality must equal structural equality. |
| `prop_secret_string_ct_eq_matches_string_eq` | `private` | fn | [`crates/pcloud-secret/tests/proptest_zeroize_invariants.rs:59`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/proptest_zeroize_invariants.rs#L59) | Read the source/rustdoc for the exact contract. |
| `prop_secret_bytes_zeroize_empties_exposed` | `private` | fn | [`crates/pcloud-secret/tests/proptest_zeroize_invariants.rs:73`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/proptest_zeroize_invariants.rs#L73) | Explicit `zeroize()` empties the exposed buffer even before drop. Note: we can't observe post-drop memory in… |
| `prop_secret_string_zeroize_empties_exposed` | `private` | fn | [`crates/pcloud-secret/tests/proptest_zeroize_invariants.rs:82`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/proptest_zeroize_invariants.rs#L82) | Read the source/rustdoc for the exact contract. |
| `prop_secret_bytes_clone_secret_is_equal` | `private` | fn | [`crates/pcloud-secret/tests/proptest_zeroize_invariants.rs:91`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/proptest_zeroize_invariants.rs#L91) | `clone_secret` produces an equal but independently-owned secret. |
| `prop_secret_string_clone_secret_is_equal` | `private` | fn | [`crates/pcloud-secret/tests/proptest_zeroize_invariants.rs:99`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/proptest_zeroize_invariants.rs#L99) | Read the source/rustdoc for the exact contract. |
| `secret_string_debug_is_redacted` | `private` | fn | [`crates/pcloud-secret/tests/redaction_and_zeroize.rs:22`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/redaction_and_zeroize.rs#L22) | Read the source/rustdoc for the exact contract. |
| `secret_bytes_debug_is_redacted` | `private` | fn | [`crates/pcloud-secret/tests/redaction_and_zeroize.rs:30`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/redaction_and_zeroize.rs#L30) | Read the source/rustdoc for the exact contract. |
| `secret_string_expose_returns_original` | `private` | fn | [`crates/pcloud-secret/tests/redaction_and_zeroize.rs:38`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/redaction_and_zeroize.rs#L38) | Read the source/rustdoc for the exact contract. |
| `secret_bytes_expose_returns_original` | `private` | fn | [`crates/pcloud-secret/tests/redaction_and_zeroize.rs:46`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/redaction_and_zeroize.rs#L46) | Read the source/rustdoc for the exact contract. |
| `secret_string_drop_zeroizes_backing_storage` | `private` | fn | [`crates/pcloud-secret/tests/redaction_and_zeroize.rs:55`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/redaction_and_zeroize.rs#L55) | Read the source/rustdoc for the exact contract. |
| `secret_string_partial_eq_is_constant_time_compatible` | `private` | fn | [`crates/pcloud-secret/tests/redaction_and_zeroize.rs:87`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/redaction_and_zeroize.rs#L87) | Read the source/rustdoc for the exact contract. |
| `secret_bytes_partial_eq_is_constant_time_compatible` | `private` | fn | [`crates/pcloud-secret/tests/redaction_and_zeroize.rs:98`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/redaction_and_zeroize.rs#L98) | Read the source/rustdoc for the exact contract. |
| `secret_string_clone_secret_duplicates_content` | `private` | fn | [`crates/pcloud-secret/tests/redaction_and_zeroize.rs:107`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/redaction_and_zeroize.rs#L107) | Read the source/rustdoc for the exact contract. |
| `secret_bytes_clone_secret_duplicates_content` | `private` | fn | [`crates/pcloud-secret/tests/redaction_and_zeroize.rs:115`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/redaction_and_zeroize.rs#L115) | Read the source/rustdoc for the exact contract. |
| `redact_field_does_not_leak_value` | `private` | fn | [`crates/pcloud-secret/tests/redaction_and_zeroize.rs:123`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/redaction_and_zeroize.rs#L123) | Read the source/rustdoc for the exact contract. |
| `prop_secret_string_debug_never_leaks` | `private` | fn | [`crates/pcloud-secret/tests/redaction_and_zeroize.rs:131`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/redaction_and_zeroize.rs#L131) | Read the source/rustdoc for the exact contract. |
| `prop_secret_bytes_debug_never_leaks` | `private` | fn | [`crates/pcloud-secret/tests/redaction_and_zeroize.rs:142`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/redaction_and_zeroize.rs#L142) | Read the source/rustdoc for the exact contract. |
| `prop_secret_string_clone_equal_and_redacted` | `private` | fn | [`crates/pcloud-secret/tests/redaction_and_zeroize.rs:151`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/redaction_and_zeroize.rs#L151) | Read the source/rustdoc for the exact contract. |
| `Serializable` | `private` | trait | [`crates/pcloud-secret/tests/serialize_is_forbidden.rs:36`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/serialize_is_forbidden.rs#L36) | Read the source/rustdoc for the exact contract. |
| `NotSerializable` | `private` | trait | [`crates/pcloud-secret/tests/serialize_is_forbidden.rs:38`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/serialize_is_forbidden.rs#L38) | Read the source/rustdoc for the exact contract. |
| `assert_not_serializable` | `private` | fn | [`crates/pcloud-secret/tests/serialize_is_forbidden.rs:42`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/serialize_is_forbidden.rs#L42) | Read the source/rustdoc for the exact contract. |
| `secret_types_are_not_trivially_serializable` | `private` | fn | [`crates/pcloud-secret/tests/serialize_is_forbidden.rs:45`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-secret/tests/serialize_is_forbidden.rs#L45) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Core workspace code may depend on this contract. External applications should prefer `pcloud-sdk` unless they intentionally own the lower-level runtime.
