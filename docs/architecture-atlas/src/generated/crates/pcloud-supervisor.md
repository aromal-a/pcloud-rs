# `pcloud-supervisor`

**Maturity:** Experimental / bounded

**Version:** `0.1.0`

**Directory:** `crates/pcloud-supervisor`

**Manifest:** [`crates/pcloud-supervisor/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/Cargo.toml)

Multi-account supervisor scaffold (T2.8). Per-account state + IPC routing model.

## Feature-family profile

**Why it exists.** Model multiple isolated accounts without merging credentials, state directories, or daemon authority.

**What it is good for.** Experimental account registry, account selection, IPC routing metadata, and per-account sub-daemon spawning.

**Why it is good at that job.** Process and path isolation make account boundaries explicit; the scaffold remains separate until routing and lifecycle are production-wired.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_supervisor` | lib | [`crates/pcloud-supervisor/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs) |

## Direct dependencies

`pcloud-config`, `pcloud-daemon`, `pcloud-ipc`, `serde`, `serde_json`, `thiserror`

## Cargo features

No declared package features.

## File inventory (3)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-supervisor/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-supervisor/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs) | library root | T2.8 — multi-account supervisor scaffold. |
| [`crates/pcloud-supervisor/src/spawner.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/spawner.rs) | Rust module | T2.8.c — sub-daemon spawning helper. |

## Rust declaration index (50 total; 27 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `spawner` | `pub` | mod | [`crates/pcloud-supervisor/src/lib.rs:37`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L37) | Read the source/rustdoc for the exact contract. |
| `AccountId` | `pub` | struct | [`crates/pcloud-supervisor/src/lib.rs:43`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L43) | Stable per-account identifier. Allocated by the supervisor on first `add_account` call; never reused. |
| `new` | `pub` | fn | [`crates/pcloud-supervisor/src/lib.rs:48`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L48) | Construct an `AccountId` from a `u64`. |
| `get` | `pub` | fn | [`crates/pcloud-supervisor/src/lib.rs:54`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L54) | Underlying `u64`. |
| `AccountStatus` | `pub` | enum | [`crates/pcloud-supervisor/src/lib.rs:62`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L62) | Operational status of an account slot. |
| `AccountSlot` | `pub` | struct | [`crates/pcloud-supervisor/src/lib.rs:78`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L78) | Per-account state record. |
| `SupervisorRegistry` | `pub` | struct | [`crates/pcloud-supervisor/src/lib.rs:105`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L105) | Top-level registry of all known accounts. |
| `SupervisorError` | `pub` | enum | [`crates/pcloud-supervisor/src/lib.rs:117`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L117) | Errors returned by registry operations. |
| `new` | `pub` | fn | [`crates/pcloud-supervisor/src/lib.rs:135`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L135) | Empty registry. |
| `add_account` | `pub` | fn | [`crates/pcloud-supervisor/src/lib.rs:148`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L148) | Add a new account. Returns the freshly allocated id. The first account added is also marked as default. # Err… |
| `remove_account` | `pub` | fn | [`crates/pcloud-supervisor/src/lib.rs:181`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L181) | Remove an account. Clears the default pointer if it was pointing at the removed account. # Errors \[`Superviso… |
| `get` | `pub` | fn | [`crates/pcloud-supervisor/src/lib.rs:193`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L193) | Look up by id. |
| `by_label` | `pub` | fn | [`crates/pcloud-supervisor/src/lib.rs:200`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L200) | Look up by label (case-sensitive). Returns the first match (labels are unique by construction). |
| `set_default` | `pub` | fn | [`crates/pcloud-supervisor/src/lib.rs:210`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L210) | Set the default account. # Errors \[`SupervisorError::NotFound`\] if `id` is not in the registry. |
| `update_status` | `pub` | fn | [`crates/pcloud-supervisor/src/lib.rs:219`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L219) | Update an account's status. No-op if the id is missing. |
| `len` | `pub` | fn | [`crates/pcloud-supervisor/src/lib.rs:227`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L227) | Number of accounts in the registry. |
| `is_empty` | `pub` | fn | [`crates/pcloud-supervisor/src/lib.rs:233`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L233) | `true` when the registry has no accounts. |
| `default_id` | `pub` | fn | [`crates/pcloud-supervisor/src/lib.rs:239`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L239) | Return the default account id if set. |
| `iter` | `pub` | fn | [`crates/pcloud-supervisor/src/lib.rs:244`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L244) | Iterate accounts in id order (deterministic). |
| `AccountHint` | `pub` | enum | [`crates/pcloud-supervisor/src/lib.rs:252`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L252) | Hint a CLI invocation supplies to pick which account a request targets. |
| `route_request` | `pub` | fn | [`crates/pcloud-supervisor/src/lib.rs:272`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L272) | Resolve an `AccountHint` against a \[`SupervisorRegistry`\] and return the slot the request should route to. #… |
| `tests` | `private` | mod | [`crates/pcloud-supervisor/src/lib.rs:289`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L289) | Read the source/rustdoc for the exact contract. |
| `sock` | `private` | fn | [`crates/pcloud-supervisor/src/lib.rs:292`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L292) | Read the source/rustdoc for the exact contract. |
| `add_account_allocates_increasing_ids` | `private` | fn | [`crates/pcloud-supervisor/src/lib.rs:297`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L297) | Read the source/rustdoc for the exact contract. |
| `first_added_account_becomes_default` | `private` | fn | [`crates/pcloud-supervisor/src/lib.rs:305`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L305) | Read the source/rustdoc for the exact contract. |
| `empty_label_rejected` | `private` | fn | [`crates/pcloud-supervisor/src/lib.rs:314`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L314) | Read the source/rustdoc for the exact contract. |
| `duplicate_label_rejected` | `private` | fn | [`crates/pcloud-supervisor/src/lib.rs:327`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L327) | Read the source/rustdoc for the exact contract. |
| `remove_account_clears_default_when_pointing_at_it` | `private` | fn | [`crates/pcloud-supervisor/src/lib.rs:337`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L337) | Read the source/rustdoc for the exact contract. |
| `route_default_when_unset_errors` | `private` | fn | [`crates/pcloud-supervisor/src/lib.rs:354`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L354) | Read the source/rustdoc for the exact contract. |
| `route_by_label` | `private` | fn | [`crates/pcloud-supervisor/src/lib.rs:363`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L363) | Read the source/rustdoc for the exact contract. |
| `route_by_env_label_treated_same_as_by_label` | `private` | fn | [`crates/pcloud-supervisor/src/lib.rs:374`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L374) | Read the source/rustdoc for the exact contract. |
| `route_by_id` | `private` | fn | [`crates/pcloud-supervisor/src/lib.rs:382`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L382) | Read the source/rustdoc for the exact contract. |
| `route_unknown_label_errors` | `private` | fn | [`crates/pcloud-supervisor/src/lib.rs:390`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L390) | Read the source/rustdoc for the exact contract. |
| `update_status_round_trips` | `private` | fn | [`crates/pcloud-supervisor/src/lib.rs:398`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L398) | Read the source/rustdoc for the exact contract. |
| `iter_is_id_ordered` | `private` | fn | [`crates/pcloud-supervisor/src/lib.rs:407`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L407) | Read the source/rustdoc for the exact contract. |
| `serde_roundtrip` | `private` | fn | [`crates/pcloud-supervisor/src/lib.rs:419`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L419) | Read the source/rustdoc for the exact contract. |
| `end_to_end_two_accounts_route_independently` | `private` | fn | [`crates/pcloud-supervisor/src/lib.rs:431`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/lib.rs#L431) | Acceptance pivot: two accounts running concurrently, each CLI invocation targets one by hint. |
| `SpawnError` | `pub` | enum | [`crates/pcloud-supervisor/src/spawner.rs:52`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/spawner.rs#L52) | Errors returned by the sub-daemon spawning helper. |
| `SpawnedDaemon` | `pub` | struct | [`crates/pcloud-supervisor/src/spawner.rs:75`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/spawner.rs#L75) | Handle returned by \[`spawn_account`\]. Owns the join handle of the dedicated serve thread and a shared stop fl… |
| `is_running` | `pub` | fn | [`crates/pcloud-supervisor/src/spawner.rs:93`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/spawner.rs#L93) | `true` while the spawned thread is still live (i.e. the serve loop has not returned). Note that this races wi… |
| `stop_flag` | `pub` | fn | [`crates/pcloud-supervisor/src/spawner.rs:100`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/spawner.rs#L100) | Reference to the shared stop flag. Exposed so callers can observe (but not flip) the cooperative shutdown sta… |
| `spawn_account` | `pub` | fn | [`crates/pcloud-supervisor/src/spawner.rs:122`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/spawner.rs#L122) | Spawn a per-account daemon on a dedicated `std::thread`. The thread: 1. constructs an \[`AccountScope`\] from `… |
| `stop_account` | `pub` | fn | [`crates/pcloud-supervisor/src/spawner.rs:165`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/spawner.rs#L165) | Stop a previously spawned daemon: flip the cooperative stop flag and join the serve thread. # Errors - \[`Spaw… |
| `run_serve_loop` | `private` | fn | [`crates/pcloud-supervisor/src/spawner.rs:176`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/spawner.rs#L176) | Adapter wrapper invoked on the serve thread. Bootstraps the account-scoped runtime, binds the IPC socket, and… |
| `tests` | `private` | mod | [`crates/pcloud-supervisor/src/spawner.rs:199`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/spawner.rs#L199) | Read the source/rustdoc for the exact contract. |
| `unique_root` | `private` | fn | [`crates/pcloud-supervisor/src/spawner.rs:212`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/spawner.rs#L212) | Generate a unique short root path under `/tmp` so the derived Unix socket path stays under `SUN_LEN` (104 byt… |
| `mk_profile` | `private` | fn | [`crates/pcloud-supervisor/src/spawner.rs:225`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/spawner.rs#L225) | Read the source/rustdoc for the exact contract. |
| `wait_until` | `private` | fn | [`crates/pcloud-supervisor/src/spawner.rs:230`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/spawner.rs#L230) | Wait until `cond()` returns `true` or the deadline expires. |
| `spawn_two_accounts_get_isolated_daemons` | `private` | fn | [`crates/pcloud-supervisor/src/spawner.rs:245`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/spawner.rs#L245) | Two registered accounts spawn into independent daemons whose IPC sockets live under disjoint per-account subt… |
| `spawn_then_stop_does_not_leak_resources` | `private` | fn | [`crates/pcloud-supervisor/src/spawner.rs:300`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-supervisor/src/spawner.rs#L300) | Spawning then immediately stopping joins the thread within a reasonable bound. Validates that no thread is le… |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.
