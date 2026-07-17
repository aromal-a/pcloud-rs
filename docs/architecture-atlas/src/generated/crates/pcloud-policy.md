# `pcloud-policy`

**Maturity:** Experimental / bounded

**Version:** `0.8.1-beta`

**Directory:** `crates/pcloud-policy`

**Manifest:** [`crates/pcloud-policy/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/Cargo.toml)

Policy enforcement layer (OPA/Rego) for the pcloud-rs Rust daemon.

## Feature-family profile

**Why it exists.** Apply organization rules before sensitive operations rather than relying only on server-side administration.

**What it is good for.** Default-deny Rego evaluation, policy bundle loading/hot reload, contextual allow/deny decisions, and null single-user policy.

**Why it is good at that job.** Fail-closed evaluation, owner-only policy files, deterministic inputs, and a null default preserve security and single-user simplicity.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_policy` | lib | [`crates/pcloud-policy/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs) |

## Direct dependencies

`regorus`, `serde`, `serde_json`, `tempfile`, `thiserror`

## Cargo features

No declared package features.

## File inventory (6)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-policy/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/Cargo.toml) | Cargo manifest | The default feature set also enables Regorus's bytecode VM (`rvm`), which |
| [`crates/pcloud-policy/examples/policies/allow-all.rego`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/examples/policies/allow-all.rego) | example | Development-only: allow every request. Do NOT ship in production. |
| [`crates/pcloud-policy/examples/policies/crypto-setup-managed-device.rego`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/examples/policies/crypto-setup-managed-device.rego) | example | Default deny so unrelated commands are not implicitly allowed by this file. |
| [`crates/pcloud-policy/examples/policies/default-deny.rego`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/examples/policies/default-deny.rego) | example | Safe baseline: every request that isn't explicitly allowed by another |
| [`crates/pcloud-policy/examples/policies/publink-expiry-7d.rego`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/examples/policies/publink-expiry-7d.rego) | example | Allow by default; deny only the specific case we care about. |
| [`crates/pcloud-policy/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs) | library root | pcloud-policy |

## Rust declaration index (31 total; 8 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `PolicyInput` | `pub` | struct | [`crates/pcloud-policy/src/lib.rs:131`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L131) | Input shaped for every daemon request before it reaches the handler. The daemon dispatch layer converts each… |
| `PolicyDecision` | `pub` | enum | [`crates/pcloud-policy/src/lib.rs:146`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L146) | Result of a policy evaluation. |
| `PolicyError` | `pub` | enum | [`crates/pcloud-policy/src/lib.rs:160`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L160) | Errors returned by a \[`PolicyEngine`\]. |
| `PolicyEngine` | `pub` | trait | [`crates/pcloud-policy/src/lib.rs:197`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L197) | Trait implemented by every policy backend. # Contract Implementors MUST: 1. **Fail closed on evaluation error… |
| `evaluate` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:210`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L210) | Evaluate `input` against the currently loaded policy. # Errors Returns \[`PolicyError::Evaluation`\] if the bac… |
| `reload` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:231`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L231) | Reload policy from its configured source (typically on `SIGHUP`). If reload fails the previously loaded polic… |
| `NullPolicyEngine` | `pub` | struct | [`crates/pcloud-policy/src/lib.rs:241`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L241) | A safe, audit-only engine that allows every request. This is the default used in development builds so contri… |
| `new` | `pub` | fn | [`crates/pcloud-policy/src/lib.rs:245`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L245) | Construct a new `NullPolicyEngine`. |
| `evaluate` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:251`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L251) | Read the source/rustdoc for the exact contract. |
| `reload` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:258`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L258) | Read the source/rustdoc for the exact contract. |
| `RegoPolicyEngine` | `pub` | struct | [`crates/pcloud-policy/src/lib.rs:278`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L278) | Rego-backed policy engine — evaluates `.rego` policies via the `regorus` pure-Rust interpreter. The engine: 1… |
| `fmt` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:284`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L284) | Read the source/rustdoc for the exact contract. |
| `DECISION_QUERY` | `private` | const | [`crates/pcloud-policy/src/lib.rs:292`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L292) | Rego query evaluated for every \[`PolicyInput`\]. |
| `new` | `pub` | fn | [`crates/pcloud-policy/src/lib.rs:301`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L301) | Construct a new Rego engine bound to `policy_dir`. All `*.rego` files inside `policy_dir` are loaded. Each fi… |
| `build_engine` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:312`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L312) | Load every `*.rego` file in `dir` into a freshly constructed `regorus::Engine`, validating file permissions a… |
| `check_permissions` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:342`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L342) | Refuse any policy file that is group-writable or world-writable. On non-Unix platforms this check is a no-op… |
| `decision_from_value` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:369`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L369) | Extract a \[`PolicyDecision`\] from a `regorus` query result. The Rego contract is a single rule `decision` ret… |
| `evaluate` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:411`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L411) | Read the source/rustdoc for the exact contract. |
| `reload` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:455`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L455) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-policy/src/lib.rs:469`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L469) | Read the source/rustdoc for the exact contract. |
| `sample_input` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:473`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L473) | Read the source/rustdoc for the exact contract. |
| `null_engine_allows_everything` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:484`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L484) | Read the source/rustdoc for the exact contract. |
| `null_engine_reload_is_noop` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:493`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L493) | Read the source/rustdoc for the exact contract. |
| `write_policy` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:501`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L501) | Write `contents` as `name` inside `dir`, mode 0o600 on Unix. |
| `DEFAULT_DENY_REGO` | `private` | const | [`crates/pcloud-policy/src/lib.rs:515`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L515) | Read the source/rustdoc for the exact contract. |
| `PUBLINK_EXPIRY_REGO` | `private` | const | [`crates/pcloud-policy/src/lib.rs:520`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L520) | Read the source/rustdoc for the exact contract. |
| `evaluates_default_deny_policy` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:533`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L533) | Read the source/rustdoc for the exact contract. |
| `evaluates_publink_expiry_rule` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:545`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L545) | Read the source/rustdoc for the exact contract. |
| `reload_swaps_engine_atomically` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:571`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L571) | Read the source/rustdoc for the exact contract. |
| `reload_failure_preserves_previous_engine` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:597`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L597) | Read the source/rustdoc for the exact contract. |
| `refuses_world_writable_policy_file` | `private` | fn | [`crates/pcloud-policy/src/lib.rs:620`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-policy/src/lib.rs#L620) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.
