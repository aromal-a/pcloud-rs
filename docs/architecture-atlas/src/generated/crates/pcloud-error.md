# `pcloud-error`

**Maturity:** Internal stable

**Version:** `0.1.0`

**Directory:** `crates/pcloud-error`

**Manifest:** [`crates/pcloud-error/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/Cargo.toml)

Shared error types and result aliases for the pcloud-rs Rust workspace.

## Feature-family profile

**Why it exists.** Give the workspace one stable failure language instead of ad-hoc strings and platform errno leakage.

**What it is good for.** Mapping failures consistently across protocol, IPC, CLI, SDK, retry, policy, and operator diagnostics.

**Why it is good at that job.** Stable codes and structured variants preserve actionable context while keeping secret material out of messages.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_error` | lib | [`crates/pcloud-error/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs) |
| `code_stability` | test | [`crates/pcloud-error/tests/code_stability.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/tests/code_stability.rs) |

## Direct dependencies

`thiserror`

## Cargo features

No declared package features.

## File inventory (4)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-error/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-error/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/README.md) | documentation | pcloud-error |
| [`crates/pcloud-error/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs) | library root | pcloud-error |
| [`crates/pcloud-error/tests/code_stability.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/tests/code_stability.rs) | test | Snapshot test for stable numeric error codes. |

## Rust declaration index (40 total; 25 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `BoxedSource` | `pub` | type | [`crates/pcloud-error/src/lib.rs:129`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L129) | Boxed, type-erased source error used for cause chaining without dragging every sub-crate's concrete error typ… |
| `Error` | `pub` | enum | [`crates/pcloud-error/src/lib.rs:138`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L138) | Unified, top-level error type for the pcloud-rs Rust workspace. Each variant is a **category**. The free-form… |
| `code` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:290`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L290) | Stable numeric error code for the variant. Scripts may depend on these numbers; any change MUST update the sn… |
| `is_retryable` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:323`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L323) | Whether a caller may meaningfully retry the failed operation. The mapping here is the canonical implementatio… |
| `category` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:336`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L336) | Short, script-friendly category slug (stable). |
| `auth` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:359`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L359) | Build a new \[`Error::Auth`\] with no cause chain attached. |
| `permission` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:366`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L366) | Build a new \[`Error::Permission`\] with no cause chain attached. |
| `api` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:374`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L374) | Build a new \[`Error::Api`\], optionally carrying the original numeric `result` code returned by the pCloud API. |
| `transport` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:382`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L382) | Build a new \[`Error::Transport`\] with no cause chain attached. |
| `ipc` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:389`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L389) | Build a new \[`Error::Ipc`\] with no cause chain attached. |
| `protocol` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:396`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L396) | Build a new \[`Error::Protocol`\] with no cause chain attached. |
| `crypto` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:403`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L403) | Build a new \[`Error::Crypto`\] with no cause chain attached. |
| `storage` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:410`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L410) | Build a new \[`Error::Storage`\] with no cause chain attached. |
| `config` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:417`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L417) | Build a new \[`Error::Config`\] with no cause chain attached. |
| `local_io` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:424`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L424) | Build a new \[`Error::LocalIo`\] with no cause chain attached. |
| `not_found` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:432`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L432) | Build a new \[`Error::NotFound`\]. This variant intentionally does not carry a cause chain; see \[`Self::with_so… |
| `invalid_input` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:439`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L439) | Build a new \[`Error::InvalidInput`\]. This variant intentionally does not carry a cause chain; see \[`Self::wit… |
| `busy` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:446`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L446) | Build a new \[`Error::Busy`\]. This variant intentionally does not carry a cause chain; see \[`Self::with_source… |
| `plugin` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:452`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L452) | Build a new \[`Error::Plugin`\] with no cause chain attached. |
| `internal` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:462`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L462) | Build a new \[`Error::Internal`\] with no cause chain attached. Prefer this over panicking for broken invariant… |
| `with_source` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:473`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L473) | Attach a boxed cause to a category that supports one. For `NotFound`/`InvalidInput`/`Busy` the source is sile… |
| `from` | `private` | fn | [`crates/pcloud-error/src/lib.rs:502`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L502) | Read the source/rustdoc for the exact contract. |
| `IntoUnified` | `pub` | trait | [`crates/pcloud-error/src/lib.rs:509`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L509) | Helper used by downstream crates that want to funnel an opaque helper error into a category. Keeps the call s… |
| `into_unified` | `private` | fn | [`crates/pcloud-error/src/lib.rs:513`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L513) | Convert `self` into a unified \[`enum@Error`\] of the given \[`Category`\], preserving `self.to_string()` as the… |
| `Category` | `pub` | enum | [`crates/pcloud-error/src/lib.rs:520`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L520) | Category selector used with \[`IntoUnified::into_unified`\]. Each variant maps 1:1 onto an \[`enum@Error`\] varia… |
| `fmt` | `private` | fn | [`crates/pcloud-error/src/lib.rs:554`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L554) | Read the source/rustdoc for the exact contract. |
| `build` | `pub` | fn | [`crates/pcloud-error/src/lib.rs:579`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L579) | Construct an \[`enum@Error`\] of this category with the supplied message and no cause chain attached. For `Api`… |
| `into_unified` | `private` | fn | [`crates/pcloud-error/src/lib.rs:605`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L605) | Read the source/rustdoc for the exact contract. |
| `Result` | `pub` | type | [`crates/pcloud-error/src/lib.rs:612`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L612) | Convenience `Result` alias scoped to the unified error. |
| `tests` | `private` | mod | [`crates/pcloud-error/src/lib.rs:615`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L615) | Read the source/rustdoc for the exact contract. |
| `codes_match_categories` | `private` | fn | [`crates/pcloud-error/src/lib.rs:619`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L619) | Read the source/rustdoc for the exact contract. |
| `from_io_preserves_chain` | `private` | fn | [`crates/pcloud-error/src/lib.rs:638`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L638) | Read the source/rustdoc for the exact contract. |
| `into_unified_preserves_cause` | `private` | fn | [`crates/pcloud-error/src/lib.rs:646`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L646) | Read the source/rustdoc for the exact contract. |
| `Inner` | `private` | struct | [`crates/pcloud-error/src/lib.rs:649`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L649) | Read the source/rustdoc for the exact contract. |
| `with_source_is_noop_for_leaf_variants` | `private` | fn | [`crates/pcloud-error/src/lib.rs:657`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L657) | Read the source/rustdoc for the exact contract. |
| `retryability_matches_documented_policy` | `private` | fn | [`crates/pcloud-error/src/lib.rs:663`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L663) | Read the source/rustdoc for the exact contract. |
| `category_display_is_stable` | `private` | fn | [`crates/pcloud-error/src/lib.rs:684`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/src/lib.rs#L684) | Read the source/rustdoc for the exact contract. |
| `sample` | `private` | fn | [`crates/pcloud-error/tests/code_stability.rs:14`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/tests/code_stability.rs#L14) | Read the source/rustdoc for the exact contract. |
| `numeric_codes_snapshot` | `private` | fn | [`crates/pcloud-error/tests/code_stability.rs:19`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/tests/code_stability.rs#L19) | Read the source/rustdoc for the exact contract. |
| `roundtrip_from_std_io_is_local_io` | `private` | fn | [`crates/pcloud-error/tests/code_stability.rs:50`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-error/tests/code_stability.rs#L50) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Core workspace code may depend on this contract. External applications should prefer `pcloud-sdk` unless they intentionally own the lower-level runtime.
