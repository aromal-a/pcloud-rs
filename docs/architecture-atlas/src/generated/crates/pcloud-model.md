# `pcloud-model`

**Maturity:** Internal stable

**Version:** `0.1.0`

**Directory:** `crates/pcloud-model`

**Manifest:** [`crates/pcloud-model/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/Cargo.toml)

Shared domain model types (files, folders, users, shares) for pcloud-rs.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_model` | lib | [`crates/pcloud-model/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/lib.rs) |

## Direct dependencies

`serde`, `serde_json`

## Cargo features

No declared package features.

## File inventory (12)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-model/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-model/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/README.md) | documentation | pcloud-model |
| [`crates/pcloud-model/src/auth.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/auth.rs) | Rust module | Client-visible authentication state machine. |
| [`crates/pcloud-model/src/conflict.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/conflict.rs) | Rust module | Taxonomy of sync conflicts surfaced by the engine planner. |
| [`crates/pcloud-model/src/crypto.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/crypto.rs) | Rust module | Client-visible crypto subsystem state. |
| [`crates/pcloud-model/src/health.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/health.rs) | Rust module | Coarse-grained client health classification. |
| [`crates/pcloud-model/src/ids.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/ids.rs) | Rust module | Raw `u64` value. Exposed as `pub` for ergonomic pattern |
| [`crates/pcloud-model/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/lib.rs) | library root | pcloud-model |
| [`crates/pcloud-model/src/public_links.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/public_links.rs) | Rust module | Summary row for an existing public link, as returned by |
| [`crates/pcloud-model/src/shares.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs) | Rust module | Bitwise permission flags mirroring the legacy C `PSYNC_PERM_*` |
| [`crates/pcloud-model/src/sync.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs) | Rust module | Lifecycle state of a single sync root. |
| [`crates/pcloud-model/src/transfer.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/transfer.rs) | Rust module | Lifecycle state of an individual transfer task as it moves through |

## Rust declaration index (85 total; 59 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `AuthState` | `pub` | enum | [`crates/pcloud-model/src/auth.rs:32`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/auth.rs#L32) | Client-visible authentication state machine. Mirrors the states the daemon's `auth_backend` transitions throu… |
| `ConflictKind` | `pub` | enum | [`crates/pcloud-model/src/conflict.rs:25`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/conflict.rs#L25) | Taxonomy of sync conflicts surfaced by the engine planner. Each variant captures a specific combination of lo… |
| `ConflictResolution` | `pub` | enum | [`crates/pcloud-model/src/conflict.rs:68`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/conflict.rs#L68) | Outcome of passing a \[`PlannedOperation::Conflict`\] through a resolver policy. \[`PlannedOperation::Conflict`\]… |
| `CryptoState` | `pub` | enum | [`crates/pcloud-model/src/crypto.rs:24`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/crypto.rs#L24) | Client-visible crypto subsystem state. Mirrors the `pcloud-crypto` runtime state exposed to the SDK/CLI so UI… |
| `OverallHealth` | `pub` | enum | [`crates/pcloud-model/src/health.rs:23`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/health.rs#L23) | Coarse-grained client health classification. Aggregated by the daemon from the health of individual subsystem… |
| `new` | `pub` | fn | [`crates/pcloud-model/src/ids.rs:22`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/ids.rs#L22) | Build a new id from a raw `u64`. `const` so ids can be used in constant contexts (e.g. test tables). |
| `get` | `pub` | fn | [`crates/pcloud-model/src/ids.rs:29`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/ids.rs#L29) | Extract the underlying `u64`. `const` to avoid forcing callers to match on the tuple struct. |
| `tests` | `private` | mod | [`crates/pcloud-model/src/ids.rs:132`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/ids.rs#L132) | Read the source/rustdoc for the exact contract. |
| `new_and_get_roundtrip` | `private` | fn | [`crates/pcloud-model/src/ids.rs:136`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/ids.rs#L136) | Read the source/rustdoc for the exact contract. |
| `zero_boundary` | `private` | fn | [`crates/pcloud-model/src/ids.rs:142`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/ids.rs#L142) | Read the source/rustdoc for the exact contract. |
| `u64_max_boundary` | `private` | fn | [`crates/pcloud-model/src/ids.rs:147`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/ids.rs#L147) | Read the source/rustdoc for the exact contract. |
| `ids_are_ordered` | `private` | fn | [`crates/pcloud-model/src/ids.rs:152`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/ids.rs#L152) | Read the source/rustdoc for the exact contract. |
| `ids_serde_roundtrip` | `private` | fn | [`crates/pcloud-model/src/ids.rs:159`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/ids.rs#L159) | Read the source/rustdoc for the exact contract. |
| `distinct_id_types_same_value_equal_inside_type` | `private` | fn | [`crates/pcloud-model/src/ids.rs:167`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/ids.rs#L167) | Read the source/rustdoc for the exact contract. |
| `auth` | `pub` | mod | [`crates/pcloud-model/src/lib.rs:26`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/lib.rs#L26) | Authentication-state enum shared between the daemon, SDK, and CLI. |
| `conflict` | `pub` | mod | [`crates/pcloud-model/src/lib.rs:28`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/lib.rs#L28) | Conflict classification and resolver output types. |
| `crypto` | `pub` | mod | [`crates/pcloud-model/src/lib.rs:30`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/lib.rs#L30) | Crypto-subsystem state enum surfaced to clients. |
| `health` | `pub` | mod | [`crates/pcloud-model/src/lib.rs:32`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/lib.rs#L32) | Overall client health classification. |
| `ids` | `pub` | mod | [`crates/pcloud-model/src/lib.rs:34`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/lib.rs#L34) | Strongly-typed identifier newtypes (sync ids, remote ids, etc.). |
| `public_links` | `pub` | mod | [`crates/pcloud-model/src/lib.rs:36`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/lib.rs#L36) | Public-link and upload-link data types. |
| `shares` | `pub` | mod | [`crates/pcloud-model/src/lib.rs:38`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/lib.rs#L38) | Shared folder, share-request, and contact data types. |
| `sync` | `pub` | mod | [`crates/pcloud-model/src/lib.rs:40`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/lib.rs#L40) | Sync-engine domain types: candidates, planned operations, states. |
| `transfer` | `pub` | mod | [`crates/pcloud-model/src/lib.rs:42`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/lib.rs#L42) | Transfer-lifecycle and recovery decision types. |
| `CRATE_NAME` | `pub` | const | [`crates/pcloud-model/src/lib.rs:52`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/lib.rs#L52) | Canonical crate name, exposed for structured logs/metrics that tag events with the emitting crate. # Example… |
| `module_count` | `pub` | fn | [`crates/pcloud-model/src/lib.rs:66`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/lib.rs#L66) | Count of public submodules exposed by this crate. Kept as a function so it can be asserted by higher-level sm… |
| `tests` | `private` | mod | [`crates/pcloud-model/src/lib.rs:71`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/lib.rs#L71) | Read the source/rustdoc for the exact contract. |
| `crate_name_is_stable` | `private` | fn | [`crates/pcloud-model/src/lib.rs:75`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/lib.rs#L75) | Read the source/rustdoc for the exact contract. |
| `module_count_is_nine` | `private` | fn | [`crates/pcloud-model/src/lib.rs:80`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/lib.rs#L80) | Read the source/rustdoc for the exact contract. |
| `PublicLinkSummary` | `pub` | struct | [`crates/pcloud-model/src/public_links.rs:9`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/public_links.rs#L9) | Summary row for an existing public link, as returned by `listpublinks` / `showpublink`. |
| `PublicLinkContentsEntry` | `pub` | struct | [`crates/pcloud-model/src/public_links.rs:41`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/public_links.rs#L41) | Single entry inside a public folder-link listing. |
| `PublicLinkContents` | `pub` | struct | [`crates/pcloud-model/src/public_links.rs:59`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/public_links.rs#L59) | Contents of a public folder-link — the short code and the list of entries currently visible through it. |
| `CreatedPublicLink` | `pub` | struct | [`crates/pcloud-model/src/public_links.rs:68`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/public_links.rs#L68) | Result of creating a public link (file or folder). |
| `UploadLinkSummary` | `pub` | struct | [`crates/pcloud-model/src/public_links.rs:79`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/public_links.rs#L79) | Summary row for an upload-link. |
| `CreatedUploadLink` | `pub` | struct | [`crates/pcloud-model/src/public_links.rs:113`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/public_links.rs#L113) | Result of creating an upload-link. |
| `CreatedTreePublicLink` | `pub` | struct | [`crates/pcloud-model/src/public_links.rs:123`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/public_links.rs#L123) | Result of creating a "tree" public link that bundles several files or folders into a single share. |
| `PublicLinkAccessEntry` | `pub` | struct | [`crates/pcloud-model/src/public_links.rs:135`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/public_links.rs#L135) | Access-control entry for a public link: a specific recipient email and receiver id allowed to access the link. |
| `PublicLinkBookmark` | `pub` | struct | [`crates/pcloud-model/src/public_links.rs:145`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/public_links.rs#L145) | Bookmarked (pinned) public link as stored by the owner for quick reuse from the client UI. |
| `PublicLinkUploadPolicy` | `pub` | enum | [`crates/pcloud-model/src/public_links.rs:173`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/public_links.rs#L173) | Upload-policy for an existing public folder-link (controls who may upload into the shared folder through the… |
| `SharePermissions` | `pub` | struct | [`crates/pcloud-model/src/shares.rs:37`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L37) | Bitwise permission flags mirroring the legacy C `PSYNC_PERM_*` constants. The wire representation is a bitmas… |
| `READ` | `pub` | const | [`crates/pcloud-model/src/shares.rs:53`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L53) | Bitmask value for the implicit "read" flag. |
| `CREATE` | `pub` | const | [`crates/pcloud-model/src/shares.rs:55`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L55) | Bitmask value for the "create" flag. |
| `MODIFY` | `pub` | const | [`crates/pcloud-model/src/shares.rs:57`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L57) | Bitmask value for the "modify" flag. |
| `DELETE` | `pub` | const | [`crates/pcloud-model/src/shares.rs:59`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L59) | Bitmask value for the "delete" flag. |
| `MANAGE` | `pub` | const | [`crates/pcloud-model/src/shares.rs:61`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L61) | Bitmask value for the "manage" flag. |
| `from_bits` | `pub` | fn | [`crates/pcloud-model/src/shares.rs:82`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L82) | Decode a C bitmask into a typed permission set. Unknown upper bits are ignored; `read` is always set on the r… |
| `to_bits` | `pub` | fn | [`crates/pcloud-model/src/shares.rs:110`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L110) | Encode a typed permission set into the C bitmask form. `read` is always OR-ed in so the encoding roundtrips w… |
| `ShareDirection` | `pub` | enum | [`crates/pcloud-model/src/shares.rs:148`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L148) | Direction of a share relative to the currently authenticated user. Used to partition UI surfaces (incoming in… |
| `ShareEntry` | `pub` | struct | [`crates/pcloud-model/src/shares.rs:157`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L157) | Established share entry. Mirrors the retained subset of `psync_share_t`. |
| `ShareRequestEntry` | `pub` | struct | [`crates/pcloud-model/src/shares.rs:187`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L187) | Pending share request. Mirrors the retained subset of `psync_sharerequest_t`. |
| `ContactEntry` | `pub` | struct | [`crates/pcloud-model/src/shares.rs:212`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L212) | Business contact entry (`type==1`) or team (`type==3`) cache row. |
| `ShareMutationResult` | `pub` | struct | [`crates/pcloud-model/src/shares.rs:227`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L227) | Result of a share mutation call (`sharefolder`, `acceptshare`, …). |
| `tests` | `private` | mod | [`crates/pcloud-model/src/shares.rs:235`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L235) | Read the source/rustdoc for the exact contract. |
| `permissions_roundtrip_matches_c_bits` | `private` | fn | [`crates/pcloud-model/src/shares.rs:239`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L239) | Read the source/rustdoc for the exact contract. |
| `permissions_from_zero_bits_still_has_read` | `private` | fn | [`crates/pcloud-model/src/shares.rs:255`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L255) | Read the source/rustdoc for the exact contract. |
| `permissions_to_bits_default_is_read_only` | `private` | fn | [`crates/pcloud-model/src/shares.rs:262`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L262) | Read the source/rustdoc for the exact contract. |
| `permissions_all_set_roundtrip` | `private` | fn | [`crates/pcloud-model/src/shares.rs:268`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L268) | Read the source/rustdoc for the exact contract. |
| `permissions_ignores_unknown_bits` | `private` | fn | [`crates/pcloud-model/src/shares.rs:280`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L280) | Read the source/rustdoc for the exact contract. |
| `share_direction_serde_roundtrip` | `private` | fn | [`crates/pcloud-model/src/shares.rs:288`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/shares.rs#L288) | Read the source/rustdoc for the exact contract. |
| `SyncState` | `pub` | enum | [`crates/pcloud-model/src/sync.rs:33`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L33) | Lifecycle state of a single sync root. The engine-level state machine transitions through these as the daemon… |
| `ChangeSource` | `pub` | enum | [`crates/pcloud-model/src/sync.rs:78`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L78) | Origin of a \[`SyncCandidate`\] — which side observed the change. The planner uses this together with \[`ChangeK… |
| `EntryKind` | `pub` | enum | [`crates/pcloud-model/src/sync.rs:105`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L105) | File-vs-folder discriminator for an entry under reconciliation. Symlinks, sockets, FIFOs, and device nodes ar… |
| `ChangeKind` | `pub` | enum | [`crates/pcloud-model/src/sync.rs:130`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L130) | Coarse classification of a change event. "Upsert" conflates create and modify intentionally: the server-side… |
| `SyncType` | `pub` | enum | [`crates/pcloud-model/src/sync.rs:162`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L162) | Direction of data flow configured for a sync root. Mirrors the three C `psync_synctype_t` values declared in… |
| `as_u8` | `pub` | fn | [`crates/pcloud-model/src/sync.rs:199`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L199) | Encode as a stable numeric value. Values 1–3 mirror the legacy C `psync_synctype_t` encoding (`PSYNC_DOWNLOAD… |
| `from_u8` | `pub` | fn | [`crates/pcloud-model/src/sync.rs:221`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L221) | Decode from the stable numeric value. Returns `None` for any value outside `1..=4`. # Example ``` use pcloud_… |
| `label` | `pub` | fn | [`crates/pcloud-model/src/sync.rs:242`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L242) | Short kebab-case label suitable for log lines and CLI output. # Example ``` use pcloud_model::sync::SyncType;… |
| `SyncCandidate` | `pub` | struct | [`crates/pcloud-model/src/sync.rs:283`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L283) | A candidate change observed on one side of a sync pair. The local scanner, diff poller, and fs-event ingestor… |
| `PlannedOperation` | `pub` | enum | [`crates/pcloud-model/src/sync.rs:316`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L316) | A single actionable operation produced by the planner. Executed by the scheduler/transfer coordinators on the… |
| `sync_id` | `pub` | fn | [`crates/pcloud-model/src/sync.rs:394`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L394) | Return the sync-root id this operation belongs to. # Example ``` use pcloud_model::ids::SyncId; use pcloud_mo… |
| `priority` | `pub` | fn | [`crates/pcloud-model/src/sync.rs:435`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L435) | Execution priority. **Lower is more urgent.** The scheduler orders operations by this value so conflicts are… |
| `path` | `pub` | fn | [`crates/pcloud-model/src/sync.rs:460`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L460) | Return the path this operation acts on (relative to the sync root). # Example ``` use pcloud_model::ids::Sync… |
| `tests` | `private` | mod | [`crates/pcloud-model/src/sync.rs:474`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L474) | Read the source/rustdoc for the exact contract. |
| `sync_type_default_is_full` | `private` | fn | [`crates/pcloud-model/src/sync.rs:479`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L479) | Read the source/rustdoc for the exact contract. |
| `sync_type_u8_roundtrip_all_variants` | `private` | fn | [`crates/pcloud-model/src/sync.rs:484`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L484) | Read the source/rustdoc for the exact contract. |
| `sync_type_from_u8_rejects_invalid` | `private` | fn | [`crates/pcloud-model/src/sync.rs:496`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L496) | Read the source/rustdoc for the exact contract. |
| `sync_type_labels` | `private` | fn | [`crates/pcloud-model/src/sync.rs:503`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L503) | Read the source/rustdoc for the exact contract. |
| `planned_operation_priority_ordering` | `private` | fn | [`crates/pcloud-model/src/sync.rs:511`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L511) | Read the source/rustdoc for the exact contract. |
| `planned_operation_accessors_match_construction` | `private` | fn | [`crates/pcloud-model/src/sync.rs:537`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L537) | Read the source/rustdoc for the exact contract. |
| `planned_operation_empty_path_boundary` | `private` | fn | [`crates/pcloud-model/src/sync.rs:548`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L548) | Read the source/rustdoc for the exact contract. |
| `sync_candidate_serde_roundtrip` | `private` | fn | [`crates/pcloud-model/src/sync.rs:557`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/sync.rs#L557) | Read the source/rustdoc for the exact contract. |
| `TransferState` | `pub` | enum | [`crates/pcloud-model/src/transfer.rs:34`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/transfer.rs#L34) | Lifecycle state of an individual transfer task as it moves through the upload/download pipeline. # Normal pro… |
| `TransferTask` | `pub` | struct | [`crates/pcloud-model/src/transfer.rs:59`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/transfer.rs#L59) | A single planned transfer together with its current state and the most recent error (if any). |
| `planned` | `pub` | fn | [`crates/pcloud-model/src/transfer.rs:88`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/transfer.rs#L88) | Construct a new `TransferTask` in the \[`TransferState::Planned`\] state with no recorded error. # Example ```… |
| `FailureDisposition` | `pub` | enum | [`crates/pcloud-model/src/transfer.rs:123`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/transfer.rs#L123) | Decision returned by the failure classifier indicating how a failed transfer should be handled. Produced by t… |
| `RecoveryDecision` | `pub` | struct | [`crates/pcloud-model/src/transfer.rs:143`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-model/src/transfer.rs#L143) | Output of recovery classification for a failed transfer: the offending operation, the chosen disposition, and… |

## Usage guidance

Core workspace code may depend on this contract. External applications should prefer `pcloud-sdk` unless they intentionally own the lower-level runtime.
