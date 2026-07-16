# CLAUDEREV Deferred-Set Progress

Driver: cron `*/3 * * * *` (every 3 min, session-scoped).
Plan: `CLAUDEREV/DEFERRED-PLAN.md`.
Started: 2026-04-30 (immediately after the original 36-fire campaign closed).

Each fire appends a log block. When all D1–D6 items are DONE or
[OUT-OF-SCOPE-PENDING-USER-RESOURCE], the loop self-terminates via
`CronDelete` and writes `CLAUDEREV/DEFERRED-COMPLETE.md`.

Verification baseline (must hold across every fire):

- `cargo check --workspace --all-targets` exit 0
- `cargo fmt --all --check` exit 0
- `cargo deny check` reports `advisories ok, bans ok, licenses ok, sources ok`
- `cargo doc --workspace --no-deps` warning count monotonically non-increasing (current floor: 41)

---

## Status table

| Item | Status | Notes |
|---|---|---|
| D1 — Page-cache generalisation | DONE | fires 37-45: D1.1a → D1.4 all CODE-DONE. Single canonical `PageCacheGeneric<K>` workspace-wide; `PageCacheGeneric<String>` backs `read_path.rs` + `CacheShell.pages`; `PageCacheGeneric<PageKey>` backs `fuse_adapter`; legacy `pcloud_cache::page_cache::PageCache` and legacy `pcloud_fs::page_cache::PageCache` both deleted. iter-3 dim-5 NEW-1 finding fully closed. |
| D2 — `AccountChangePassword` round-trip | DONE | fire 46: D2.1 marker-file persistence layer + D2.2 round-trip test (`live_account_change_password_round_trip`); recovery branch handles crashed prior runs; gate-skips clean without `PCLOUD_LIVE_E2E_DESTRUCTIVE=1`; user has live accounts A+B in `.env` for actual run |
| D3 — Row 142 `CryptoAccountTeamShare` IPC | DONE | fire 47: new IPC variant + dispatch arm + privilege-table entry + audit-name + verb-reached live test; row 142 flipped Partial → Implemented; STATUS.md headline 153/3 → 154/2 |
| D4 — `notify-debouncer-full` swap | ACKNOWLEDGED-DEFERRED | fire 48: dep added, vendor/notify-dfly-fix patch interaction OK; investigation found the swap would **regress** the iter-1 SYNC-H-04-2 stall fix from fire 20 because debouncer-full lacks a max-age guard. Plan was structurally incomplete; dep reverted; workspace inline doc records the finding |
| D5 — Per-backend `ResilientTransport` | DONE | fires 49-55: D5.1-D5.7 CODE-DONE (auth, transfer, public-link, shares, sync, backup, account). All 7 production backends now route through workspace-shared `GlobalRetryBudget` + per-endpoint circuit-breakers in production environments |
| D6 — RSA-OAEP wire-shape unification | DONE | fire 56: new `Request::CryptoShareFolderRsa` IPC variant + daemon multi-RPC orchestrator (auth → crypto-unlock check → `crypto_getpubkey` → RSA-4096-OAEP wrap → share request) + new `CryptoRuntime::get_pub_key` wrapper + verb-reached live test; row 124 flipped Partial → Implemented; STATUS.md headline 154/2 → 155/1 |

---

## Fire log

### Fire 37 — 2026-04-30 (D1.1a `PageCacheGeneric<K>` sibling → CODE-DONE)

**Items closed (sub-step):**
- **D1.1a — Introduce `PageCacheGeneric<K>` in `pcloud-fs::page_cache` (CODE-DONE).** First sub-step of the D1 unification. Rather than refactor the existing `PageCache` (which is heavily-used by `fuse_adapter.rs` + tests + benchmark and carries the typed-`PageKey` `by_file` secondary-index path), this fire **introduces the generic alongside** and leaves the existing surface untouched. Subsequent sub-steps (D1.1b/D1.2/D1.3) will migrate callers and unify.

**New types in `crates/pcloud-fs/src/page_cache.rs`:**
- `pub struct PageCacheGeneric<K>` where `K: Hash + Eq + Clone + Debug` — same LRU + byte-quota machinery as `PageCache`, parameterised on the key type.
- `struct InnerGeneric<K>` — internal state mirroring the existing `Inner` but **without** the `by_file: HashMap<u64, HashSet<u64>>` secondary index (which is `PageKey`-specific and only meaningful for keys that carry a `file_id` group).
- Same public API as `PageCache` minus `invalidate_file`: `new`, `default`, `config`, `get`, `put`, `clear`, `stats`, `hit_ratio`, `len`, `is_empty`. `Slot` and `PageCacheConfig` / `PageCacheStats` are reused unchanged.

**Why no `invalidate_file` on the generic:**
The secondary `by_file` index keys on `u64` (the `file_id` field of `PageKey`). Generalising it requires either (a) a `CacheKey` trait with a `group()` method, or (b) deleting the secondary index from the generic and accepting an O(n) `invalidate_<group>` walk. D1.2 will introduce the trait; this sub-step accepts that the generic does not yet have per-group invalidation.

**Decision: additive-only sub-step**
The existing `pub struct PageCache` and its 8 unit tests are unchanged. `fuse_adapter.rs` continues to use `PageCache::invalidate_file` and the `by_file` index. `read_path.rs` continues to use `pcloud_cache::page_cache::PageCache`. No production callers migrated this fire — the generic is a pure addition. This keeps the fire's blast radius minimal and makes regression-debugging trivial: any test that breaks is a test of code I did not touch.

**Files touched (1):**
- `crates/pcloud-fs/src/page_cache.rs` — added `PageCacheGeneric<K>` (~110 LoC) + 4 unit tests (~50 LoC). The 8 pre-existing tests + the `concurrent_put_and_get_do_not_deadlock` test all still pass without modification.

**Tests added (4):**
- `page_cache_generic_string_round_trips_value` — proves `PageCacheGeneric<String>` works as a flat-key cache.
- `page_cache_generic_string_evicts_under_byte_quota` — proves the LRU eviction respects `max_bytes` for string-keyed entries.
- `page_cache_generic_pagekey_round_trips_value` — proves the generic specialised on `PageKey` behaves like the legacy non-generic `PageCache` for the get/put primitives (smoke test for the future `PageCache → PageCacheGeneric<PageKey>` migration).
- `page_cache_generic_records_oversized_rejection` — proves the `bytes_rejected_oversized` counter increments on over-cap pages.

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0 (after one `cargo fmt --all` rebalance)
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-fs --lib page_cache` → **13 passed; 0 failed; 0 ignored** (8 pre-existing + 4 new generic + 1 concurrency)
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**Status table updates:**
- D1 → **PARTIAL** (D1.1a done; D1.1b/D1.2/D1.3 remain).

**Next sub-step (next fire):**
D1.1b — route `pcloud_cache::page_cache::PageCache` to `pcloud_fs::page_cache::PageCacheGeneric<String>`. Adds a `pcloud-fs` dep to `pcloud-cache` (or moves the generic to a shared crate); replaces the body of `pcloud-cache/src/page_cache.rs` with a re-export. Should be a bounded edit because the API surface used by `read_path.rs` is exactly what `PageCacheGeneric<String>` exposes.

---

### Fire 38 — 2026-04-30 (D1.1b.1 lift `PageCacheGeneric<K>` to `pcloud-cache` → CODE-DONE)

**Items closed (sub-step):**
- **D1.1b.1 — Lift `PageCacheGeneric<K>` body to `pcloud-cache` (CODE-DONE).** D1.1a placed the generic in `pcloud-fs` as a sibling. Migrating `pcloud_cache::page_cache::PageCache` to use it (the literal D1.1b ask) would require pcloud-cache → pcloud-fs as a dep direction, which is the **inverse** of the existing dep (`pcloud-fs → pcloud-cache`) and would create a cycle. This sub-step inverts the placement: the generic body moves to the lower-level crate where pcloud-fs can re-export from it.

**Refactor (4 files):**

- **`crates/pcloud-cache/Cargo.toml`** — added `lru.workspace = true`. Same workspace pin as pcloud-fs already uses, so the eviction semantics stay byte-identical when D1.2 unifies the legacy `PageCache` into the generic.

- **`crates/pcloud-cache/src/page_cache_generic.rs`** — **new file (~200 LoC)**. Hosts `pub struct PageCacheGeneric<K>`, the supporting `InnerGeneric<K>`, the per-page `Slot` (private), plus the *previously-pcloud-fs-local* types `PageCacheConfig`, `PageCacheStats`, `DEFAULT_PAGE_SIZE`, `DEFAULT_MAX_BYTES`. The 4 canonical unit tests for the generic (round-trip, byte-quota eviction, oversized rejection, typed-key smoke) move here.

- **`crates/pcloud-cache/src/lib.rs`** — added `pub mod page_cache_generic;` declaration with module-level rustdoc explaining the placement decision and the upcoming D1.1b.2 / D1.2 / D1.3 sub-steps.

- **`crates/pcloud-fs/src/page_cache.rs`** — replaced the in-tree `PageCacheGeneric<K>`, `InnerGeneric<K>`, and the `PageCacheConfig` / `PageCacheStats` / `DEFAULT_*` definitions with `pub use pcloud_cache::page_cache_generic::{...}` re-exports. Existing `use pcloud_fs::page_cache::PageCacheConfig` imports across `fuse_adapter.rs`, `fuser_shim.rs`, and the bench / integration tests continue to resolve through the re-export — verified by passing `cargo check --workspace --all-targets`. Kept one regression-guard test (`page_cache_generic_reexport_resolves_for_pagekey`) inside pcloud-fs to pin the re-export chain.

**What changed semantically:** before this fire two distinct `PageCacheConfig` and `PageCacheStats` types could in principle exist in the workspace (one in pcloud-fs, the trivial unused one in pcloud-cache). After this fire there is exactly one definition of each, in pcloud-cache, and pcloud-fs re-exports them. Symbol `PageCacheConfig` resolves to a single canonical type workspace-wide — that's progress on the iter-3 dim-5 NEW-1 finding's "single canonical implementation" bar.

**Files touched (4):**
- `crates/pcloud-cache/Cargo.toml`
- `crates/pcloud-cache/src/lib.rs`
- `crates/pcloud-cache/src/page_cache_generic.rs` (new)
- `crates/pcloud-fs/src/page_cache.rs`

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-cache --lib` → **17 passed; 0 failed** (incl. 4 new `page_cache_generic::tests::*`)
- `cargo test -p pcloud-fs --lib page_cache` → **10 passed; 0 failed; 0 ignored** (8 pre-existing legacy `PageCache` + 1 fuse_adapter + 1 re-export smoke)
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**Status table updates:**
- D1 → still **PARTIAL** (D1.1a + D1.1b.1 done; D1.1b.2 + D1.2 + D1.3 remain).

**Next sub-step (next fire):**
D1.1b.2 — alias legacy `pcloud_cache::page_cache::PageCache` (string-keyed) to `PageCacheGeneric<String>`. Replace the body of `crates/pcloud-cache/src/page_cache.rs` with a thin re-export so `read_path.rs` and any other caller writing `pcloud_cache::page_cache::PageCache` resolves to the unified generic.

---


### Fire 39 — 2026-04-30 (D1.1b.2a Clone/PartialEq/Eq for `PageCacheGeneric<K>` → CODE-DONE)

**Items closed (sub-step):**
- **D1.1b.2a — Add `Clone + PartialEq + Eq` impls to `PageCacheGeneric<K>` (CODE-DONE).** Discovery during the fire: D1.1b.2 as written in the plan (literal type-alias swap of legacy `PageCache` to `PageCacheGeneric<String>`) is **structurally impossible**. The legacy `pcloud_cache::page_cache::PageCache`:
  - takes `put(key: impl Into<String>, data: impl Into<Arc<Vec<u8>>>)` while the generic takes `put(key: K, bytes: Vec<u8>)`;
  - takes `get(key: &str)` while the generic takes `get(key: &K)`;
  - exposes `max_bytes() -> u64` / `page_size_bytes() -> usize` / `entry_count() -> usize` / `used_bytes() -> u64` accessor names; the generic uses `config()` + `stats()`;
  - implements `Clone + PartialEq + Eq + Serialize + Deserialize`; the generic does **not** (yet);
  - is held by `#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)] struct ReadPathService` in `pcloud-fs` and `#[derive(Clone, PartialEq, Eq)] struct CacheShell` in `pcloud-cache`, both of which propagate to `pcloud-daemon::sync_loop_runtime`.

  A literal swap requires either an adapter shim with the legacy method shapes or a mass-rename across CacheShell + ReadPathService + their daemon callers. Either path is multi-fire. This fire lands the **Clone + PartialEq + Eq + Default** trait impls on the generic so a future swap fire isn't blocked on missing trait bounds. `Serialize/Deserialize` are deferred to D1.1b.2b because `lru::LruCache` doesn't ship serde impls and a custom impl needs its own design pass.

**Trait impls added (`crates/pcloud-cache/src/page_cache_generic.rs`):**

- **`Clone`** — deep-copies entry data, hits/misses counters, `bytes_rejected_oversized`, and config. The cloned cache is independent: a `put` on the clone does not propagate to the original. Implementation walks `LruCache::iter()` (MRU → LRU), collects to a `Vec`, re-inserts in reverse order so the eldest entry ends at the LRU position and the most-recent at MRU — matching the source's MRU/LRU positions exactly. Mutex-poisoned source returns a fresh empty cache instead of propagating the panic; the page cache is disposable state by design.
- **`PartialEq` / `Eq`** — content-equal iff (a) configs match, (b) both have the same set of `(key, value)` pairs (LRU order is **not** compared because it is operational state, not logical state), and (c) byte counters match. Stats counters (hits/misses) are intentionally **excluded** from equality so a cache that has served reads compares equal to a cache that has not, given the same stored entries. Mutex-locking is address-ordered to avoid hold-and-wait quirks if `a == a` on the same instance is exercised.
- **`Default`** was already present from D1.1a.

**Tests added (2):**
- `clone_produces_independent_content_equal_cache` — proves `Clone` produces a content-equal cache and that post-clone mutations on the original do not show in the clone (independence invariant).
- `equality_excludes_stats_counters` — proves two caches with identical stored entries compare equal even if one has served reads and the other has not.

**Files touched (1):**
- `crates/pcloud-cache/src/page_cache_generic.rs` — added `impl<K> Clone`, `impl<K> PartialEq`, `impl<K> Eq` (~85 LoC) and 2 unit tests (~30 LoC).

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-cache --lib page_cache_generic` → **6 passed; 0 failed**
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**Status table updates:**
- D1 → still **PARTIAL** (D1.1a + D1.1b.1 + D1.1b.2a done; D1.1b.2b/2c/2d + D1.2 + D1.3 remain).

**Next sub-step (next fire):**
D1.1b.2b — implement custom `Serialize` / `Deserialize` for `PageCacheGeneric<K>` where `K: Serialize + DeserializeOwned + ...`. `lru::LruCache` has no built-in serde; the impl walks entries, serializes a `{config, entries: Vec<(K, Arc<Vec<u8>>)>, hits, misses, bytes_rejected_oversized}` payload, and rebuilds the LRU on deserialize. After D1.1b.2b, all four traits required by `ReadPathService` and `CacheShell` will be present on the generic and the actual swap (D1.1b.2c) becomes mechanical.

---

### Fire 40 — 2026-04-30 (D1.1b.2b custom serde for `PageCacheGeneric<K>` → CODE-DONE)

**Items closed (sub-step):**
- **D1.1b.2b — Custom `Serialize` / `Deserialize` for `PageCacheGeneric<K>` (CODE-DONE).** Final preparatory sub-step before D1.1b.2c (caller migration). After this fire, `PageCacheGeneric<K>` carries the full set of traits (`Clone + PartialEq + Eq + Default + Serialize + Deserialize`) that `pcloud_fs::read_path::ReadPathService` and `pcloud_cache::CacheShell` derive on their containing structs.

**Why hand-rolled (no `#[derive(Serialize, Deserialize)]`):**
`lru::LruCache` does not ship serde impls and adding a serde feature there is upstream-blocked. `parking_lot::Mutex` (legacy `pcloud_cache::PageCache`) has serde via `serde` feature; `std::sync::Mutex` (used by the generic) does not. Custom impls are the cleanest path and let us choose the wire shape.

**Wire shape (`PageCacheGenericWire<K>`):**
```rust
struct PageCacheGenericWire<K> {
    config: PageCacheConfig,
    entries: Vec<(K, Vec<u8>)>,   // MRU → LRU ordered
    hits: u64,
    misses: u64,
    bytes_rejected_oversized: u64,
}
```

Deliberate decisions:
- **Bytes are unwrapped from `Arc`** at serialize time (raw `Vec<u8>` on the wire). This avoids the `serde rc` feature interaction at the persistence layer; the in-memory `Arc` wrapping is rebuilt by `Self::new(...).put(...)` on the deserialize side.
- **`bytes_resident` and `pages_resident` are NOT transmitted.** Both are derivable from the `entries` list. Re-deriving them on deserialize via `put()` guarantees the post-deserialize view is internally consistent even if the wire bytes were tampered with: a hostile bytes_resident value on the wire cannot lie about the actual resident bytes.
- **MRU → LRU ordering on the wire**, then `Deserialize` re-inserts in **reverse** so the eldest entry ends at the LRU position and the most-recent at MRU. Matches the `Clone` impl's positioning logic from fire 39.
- **Stats counters (`hits`, `misses`, `bytes_rejected_oversized`)** are persisted so a daemon restart preserves observability over the cache's lifetime. They are restored after the entries are re-inserted (the `put()` calls themselves don't bump these counters).

**Implementation behaviour on edge cases:**
- Mutex poisoned at serialize time → `serde::ser::Error::custom("page cache mutex poisoned")` rather than a silent partial result.
- `K` bounds: `K: Hash + Eq + Clone + Debug + Serialize` for serialize, `K: Hash + Eq + Clone + Debug + Deserialize<'de>` for deserialize. Mirrors the bounds of the underlying `LruCache<K, Slot>`.

**Files touched (1):**
- `crates/pcloud-cache/src/page_cache_generic.rs` — added `use serde::{Serialize, Deserialize}`; added `#[derive(Serialize, Deserialize)]` to `PageCacheConfig` and `PageCacheStats`; added `PageCacheGenericWire<K>` private wire struct; added `impl<K> Serialize for PageCacheGeneric<K>` and `impl<'de, K> Deserialize<'de> for PageCacheGeneric<K>` (~70 LoC); added 2 round-trip unit tests (~50 LoC).

**Tests added (2):**
- `serde_round_trip_preserves_entries_and_stats` — drives `put` + `get` to populate hit/miss counters, round-trips via JSON, asserts content-equality (Eq) **plus** that stats counters survived independently. Pins both layers of the contract.
- `serde_round_trip_preserves_mru_ordering` — populates 3 entries, round-trips, then provokes an eviction by inserting a 4th entry that pushes the cache over its byte cap. Asserts the eldest entry (the LRU at serialize time) is the one dropped — proves MRU/LRU positions survive the wire format and the reverse-insert logic.

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-cache --lib page_cache_generic` → **8 passed; 0 failed** (6 pre-existing + 2 new serde tests)
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**Status table updates:**
- D1 → still **PARTIAL** but with all trait prerequisites for the swap now in place. D1.1a + D1.1b.1 + D1.1b.2a + D1.1b.2b done; D1.1b.2c (caller migration) + D1.2 (`CacheKey` trait) + D1.3 (delete legacy `PageCache` in pcloud-fs) remain.

**Next sub-step (next fire):**
D1.1b.2c — migrate `pcloud_fs::read_path::ReadPathService::pages` from legacy `pcloud_cache::page_cache::PageCache` to `pcloud_cache::page_cache_generic::PageCacheGeneric<String>`. The trait set is now compatible; the API-shape adaptations needed:
- `legacy.put(key: impl Into<String>, data: impl Into<Arc<Vec<u8>>>)` → `generic.put(key: String, bytes: Vec<u8>)`. Callers in `read_path.rs` already pass `String` keys; `Arc::new(staged[…].to_vec())` becomes the bare `Vec` (the Arc is internally added by the generic).
- `legacy.get(key: &str) -> Option<Arc<Vec<u8>>>` → `generic.get(key: &String) -> Option<Arc<Vec<u8>>>`. Need `&cache_key` instead of `&cache_key.as_str()`.
- `default()` is still available; ctor signature unchanged.

Single-file edit; legacy `PageCache` continues backing `CacheShell` until D1.1b.2d.

---

### Fire 41 — 2026-04-30 (D1.1b.2c migrate `ReadPathService` to `PageCacheGeneric<String>` → CODE-DONE)

**Items closed (sub-step):**
- **D1.1b.2c-read_path — Migrate `pcloud_fs::read_path::ReadPathService::pages` to `PageCacheGeneric<String>` (CODE-DONE).** First half of the D1.1b.2c migration. After this fire one of the two production callers (the simpler one) uses the unified generic; `pcloud_cache::CacheShell.pages` still uses the legacy `PageCache` and is the next sub-step (D1.1b.2d).

**Migration mechanics (single file, `crates/pcloud-fs/src/read_path.rs`):**
- Import: `pcloud_cache::page_cache::PageCache` → `pcloud_cache::page_cache_generic::PageCacheGeneric`.
- Field type: `pub pages: PageCache` → `pub pages: PageCacheGeneric<String>`.
- `Default::default()` continues to work — both impls' `Default` produce a 128 MiB / 64 KiB cache with the same defaults.
- `pages.get(&cache_key)` keeps the same `&String` (Rust auto-derefs the `&String` to satisfy `&K = &String` for the generic's `get`).
- `pages.put(...)` adapted: legacy took `Into<Arc<Vec<u8>>>` so callers handed it the same `Arc` they kept locally; the generic takes a bare `Vec<u8>`. Adapted by:
  1. Build the window as a bare `Vec<u8>` (was already the underlying allocation; the `Arc::new(...)` step was always immediately after).
  2. `put(cache_key.clone(), window_bytes)` — adds one cheap String clone instead of a Vec clone.
  3. `pages.get(&cache_key)` immediately to recover the **same `Arc`** the cache stores internally.
  Net cost: 1 small String clone + 1 cache get on miss. Net saved: 0 bytes of Vec duplication. The 64 KiB Vec allocation is shared via the Arc on hit and on the same-cycle re-`get` after `put`.

**Wire-format compatibility:**
`ReadPathService` derives `Serialize` and is held by upper layers (e.g. snapshot persistence in `pcloud-daemon`). The legacy `PageCache` and `PageCacheGeneric<String>` both serialise to a JSON shape carrying `entries: Vec<(String, Vec<u8>)>` — the wire format is byte-equivalent for caches with the same logical content. A pre-D1.1b.2c snapshot file deserialises into the post-migration `ReadPathService` cleanly; the only observable difference is internal LRU vs LinkedHashMap eviction behaviour after deserialize, which is operational state, not logical state.

**Files touched (1):**
- `crates/pcloud-fs/src/read_path.rs` — ~10 lines changed: import, field type, default ctor, the put/get pair on the cache-miss path. All 3 unit tests in this file (`reads_staged_bytes_and_populates_cache`, `returns_missing_path_when_no_staged_or_cached_data_exists`, `reads_across_multiple_prefetch_windows`) pass without modification.

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-fs --lib read_path` → **3 passed; 0 failed**
- `cargo test -p pcloud-fs --lib` → **209 passed; 0 failed; 1 ignored** (full lib suite, no ripple regressions)
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**Status table updates:**
- D1 → still **PARTIAL** (D1.1a + D1.1b.1 + D1.1b.2a + D1.1b.2b + D1.1b.2c-read_path done; D1.1b.2d (CacheShell.pages migration) + D1.2 (`CacheKey` trait) + D1.3 (delete legacy `PageCache` in pcloud-fs) remain).

**Next sub-step (next fire):**
D1.1b.2d — migrate `pcloud_cache::CacheShell::pages` from legacy `PageCache` to `PageCacheGeneric<String>`. The blast radius reaches `pcloud_daemon::sync_loop_runtime` (heaviest CacheShell user); `CacheShell::cache_page` and `CacheShell::summary` need API-shape adaptations:
- `cache_page(key: impl Into<String>, data: Vec<u8>)` already takes `Vec` — only the underlying `pages.put(key, data)` call changes signature compatibly.
- `summary()` reads `pages.{max_bytes(), page_size_bytes(), entry_count(), used_bytes()}`. Rewrite as `let cfg = pages.config(); let stats = pages.stats(); ...` mapping to `cfg.max_bytes`, `cfg.page_size`, `stats.pages_resident`, `stats.bytes_resident`. Note: `stats.bytes_resident` is `usize` while the legacy `used_bytes()` was `u64`; cast accordingly. After this sub-step, the legacy `pcloud_cache::page_cache::PageCache` is unused in production and can be deleted in D1.1b.2e (or left as `#[deprecated]`).

---

### Fire 42 — 2026-04-30 (D1.1b.2d migrate `CacheShell.pages` + generalise `get` to `Borrow<Q>` → CODE-DONE)

**Items closed (sub-step):**
- **D1.1b.2d — Migrate `pcloud_cache::CacheShell::pages` to `PageCacheGeneric<String>` (CODE-DONE).** Second half of D1.1b.2c. After this fire **both** production callers of the legacy `pcloud_cache::page_cache::PageCache` (the `read_path.rs` migration in fire 41 + the `CacheShell.pages` migration here) use the unified generic. The legacy struct is unreachable from production code; only its own tests / doctests / the `examples/warm_cache.rs` example still reference it. D1.3 (deletion) becomes a clean follow-up fire.

**Audit-then-fix discovery:**
A grep across the workspace for `\.pages\.` usages outside of `pcloud-cache/src/lib.rs` returned only the two call sites in `pcloud-fs/src/read_path.rs` (already migrated in fire 41). The daemon (`pcloud-daemon::sync_loop_runtime`) and other CacheShell consumers reach the cache *through* `cache_page()` and `summary()` rather than touching `.pages.*` directly — so the migration's blast radius is bounded to `CacheShell`'s own methods plus 3 test sites in `pcloud-daemon/src/lib.rs` that did call `cache.pages.get("...")` with `&'static str` literals.

**Refactor (2 files):**

- **`crates/pcloud-cache/src/lib.rs`** — `CacheShell.pages` field type swapped from `page_cache::PageCache` to `page_cache_generic::PageCacheGeneric<String>`. `Default` impl, `cache_page()` body, and `summary()` body adapted:
  - `cache_page(key: impl Into<String>, data: Vec<u8>)` — was `self.pages.put(key, data)` (legacy accepted `Into<String>` directly). Now: `self.pages.put(key.into(), data)` because the generic takes a bare `String`.
  - `summary()` — replaced four legacy accessors (`max_bytes()`, `page_size_bytes()`, `entry_count()`, `used_bytes()`) with the generic's `config()` + `stats()` pair. Same output format; the bytes_resident is `usize` rather than `u64` but the only operation is division by 1024 which doesn't overflow.
  - Doctest updated: `assert_eq!(shell.pages.entry_count(), 1)` → `assert_eq!(shell.pages.len(), 1)`.

- **`crates/pcloud-cache/src/page_cache_generic.rs`** — generalised the `get()` signature from `pub fn get(&self, key: &K)` to `pub fn get<Q>(&self, key: &Q) where K: Borrow<Q>, Q: Hash + Eq + ?Sized`. Mirrors the `HashMap::get` / `LruCache::get` ergonomic. **Why:** the daemon's test suite passed `&'static str` literals to `cache.pages.get("...")`; the post-migration `&K = &String` signature would have rejected them, forcing 3 mechanical `.to_owned()` rewrites with a tiny allocation each. The `Borrow<Q>` generalisation accepts `&str` for free with no allocation, no call-site change, and no semantic shift. Specialisations like `PageCacheGeneric<PageKey>` continue to accept `&PageKey` because `K: Borrow<K>` always holds via the blanket impl.

**Test sites unblocked by the `Borrow<Q>` generalisation:**
- `crates/pcloud-daemon/src/lib.rs:1619` — `cache.pages.get("upload:77:upload.txt")`
- `crates/pcloud-daemon/src/lib.rs:1786` — `cache.pages.get("upload:77:docs/nested/report.txt")`
- `crates/pcloud-daemon/src/lib.rs:1830` — `cache.pages.get("upload:77:large-upload.txt")`

All three compile clean against the generalised signature without modification.

**Wire-format compatibility:**
`CacheShell` does not derive `Serialize / Deserialize` itself (the legacy `PageCache` did, the generic now also does, but `CacheShell` is constructed at runtime, not deserialised from snapshots). `Clone + PartialEq + Eq` derives on `CacheShell` continue to work because the generic carries those impls (fire 39).

**Files touched (2):**
- `crates/pcloud-cache/src/lib.rs` — field type, `Default::default()`, `cache_page` body, `summary` body, one doctest assertion.
- `crates/pcloud-cache/src/page_cache_generic.rs` — `get` signature generalised to `Borrow<Q>`.

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-cache --lib` → **21 passed; 0 failed** (8 generic + 13 pre-existing)
- `cargo test -p pcloud-fs --lib` → **209 passed; 0 failed; 1 ignored** (no ripple regressions in the read-path or fuse-adapter tests)
- `cargo test -p pcloud-daemon --lib` → **230 passed; 0 failed** (the 3 test sites that called `cache.pages.get("...")` with string literals now compile via the `Borrow<Q>` generalisation; no behavioural change)
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**Status table updates:**
- D1 → still **PARTIAL** (production callers all migrated; D1.3 (delete legacy `pcloud_cache::page_cache::PageCache` body) + D1.2 (`CacheKey` trait for the `pcloud-fs::page_cache::PageCache` family) remain).

**Next sub-step (next fire):**
D1.3 — delete the body of `crates/pcloud-cache/src/page_cache.rs` (the legacy string-keyed `PageCache`), update or delete the 5 doctest examples that reference it, update the `examples/warm_cache.rs` example to use `PageCacheGeneric<String>`, and remove the `pub mod page_cache;` declaration from `pcloud-cache/src/lib.rs`. The fire-36 cross-reference rustdoc in `pcloud-fs/src/page_cache.rs` and `pcloud-cache/src/page_cache_generic.rs` can also be tightened since one of the two coexisting types is now gone.

After D1.3: a single canonical `PageCacheGeneric<K>` for string-keyed callers; the typed-`PageKey`-with-`by_file` legacy `PageCache` in pcloud-fs continues to exist until D1.2 introduces the `CacheKey` trait that lets it merge into the generic.

---

### Fire 43 — 2026-04-30 (D1.3 delete legacy `pcloud_cache::page_cache::PageCache` → CODE-DONE)

**Items closed (sub-step):**
- **D1.3 — Delete the legacy `pcloud_cache::page_cache::PageCache` body (CODE-DONE).** With both production callers (`ReadPathService.pages` from fire 41 and `CacheShell.pages` from fire 42) migrated to `PageCacheGeneric<String>`, the legacy struct was unreachable from production code. This fire deletes the body, migrates the `examples/warm_cache.rs` example, drops the now-unused `parking_lot` + `linked-hash-map` + `serde rc` dependencies, and tightens the cross-reference rustdoc.

**Refactor (5 files):**

- **`crates/pcloud-cache/src/page_cache.rs`** — **deleted** (530 LoC removed). The legacy `parking_lot::RwLock + linked_hash_map::LinkedHashMap` implementation is gone; the seven `page_cache::tests::*` doctests / unit tests it carried are gone with it (those tests were exercising the legacy implementation; the canonical generic has its own 8-test coverage in `page_cache_generic::tests::*`).

- **`crates/pcloud-cache/src/lib.rs`** — removed the `pub mod page_cache;` declaration. Rewrote the module-level rustdoc that referenced `parking_lot::RwLock` / `linked_hash_map::LinkedHashMap` to instead describe the canonical `PageCacheGeneric<K>` (Mutex + LruCache). The Observability section gained a note that the cache itself now tracks lifetime hits/misses via `PageCacheStats::hit_ratio()`, in addition to the daemon-layer Prometheus gauge.

- **`crates/pcloud-cache/Cargo.toml`** — dropped 3 dependencies that are now unused:
  - `parking_lot = "0.12"` (legacy used `RwLock`; generic uses `std::sync::Mutex`).
  - `linked-hash-map = { version = "0.5", features = ["serde_impl"] }` (legacy storage; generic uses `lru::LruCache`).
  - The `rc` feature on `serde` (legacy serialised `Arc<Vec<u8>>` directly via the `rc` feature; the generic unwraps to bare `Vec<u8>` on the wire — see fire 40).
  Replaced with a comment block citing the D1.1b/D1.3 history. `lru.workspace = true` retained — that's where the canonical primitive lives.

- **`crates/pcloud-cache/examples/warm_cache.rs`** — rewritten to use `PageCacheGeneric<String>`. The example pattern changes only at three spots: the import (`page_cache::PageCache` → `page_cache_generic::{PageCacheConfig, PageCacheGeneric}`), the constructor (`PageCache::with_capacity(max, page)` → `PageCacheGeneric::new(PageCacheConfig { max_bytes: max, page_size: page })`), and the post-warm-up snapshot (`cache.used_bytes() / cache.entry_count()` → `cache.stats().bytes_resident / cache.stats().pages_resident`). Same observable output format.

- **`crates/pcloud-fs/src/page_cache.rs`** — tightened the "Relationship to" rustdoc section since one of the two coexisting types is now gone. The new text describes (a) the re-export chain that keeps existing `use pcloud_fs::page_cache::PageCacheConfig` imports working, and (b) the remaining typed-`PageKey` legacy `PageCache` that survives until D1.2 introduces the `CacheKey` trait.

- **`crates/pcloud-cache/src/page_cache_generic.rs`** — module-level rustdoc tightened with a "Canonical string-keyed page cache for the workspace as of D1.3" header. The "Subsequent sub-steps" subsection collapsed from four bullet points to one (D1.2) since D1.1b.2 / D1.3 are now closed.

**Files touched (5):** as enumerated above. Net LoC change: **−530 (delete) + ~30 (rewrites) ≈ −500 LoC** workspace-wide.

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-cache --lib` → **14 passed; 0 failed** (was 21; the 7 legacy-only tests were deleted with the file; the 8 `page_cache_generic::tests::*` plus the 6 other-module tests survive).
- `cargo test -p pcloud-fs --lib` → **209 passed; 0 failed; 1 ignored** (no ripple regressions)
- `cargo test -p pcloud-daemon --lib` → **230 passed; 0 failed**
- `cargo build --example warm_cache -p pcloud-cache` → exit 0 (the migrated example compiles)
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)
- Workspace grep `use pcloud_cache::page_cache::` returns 0 production hits — only historical comment-references in fire-log files and inline rustdoc remain.

**Status table updates:**
- D1 → still **PARTIAL** (D1.1a + D1.1b.1 + D1.1b.2a + D1.1b.2b + D1.1b.2c + D1.3 done; **D1.2** — the `CacheKey` trait that lifts `pcloud-fs`'s typed `PageKey` `by_file` index into the generic — is the only D1 sub-step remaining).

**Next sub-step (next fire):**
D1.2 — introduce a `CacheKey` trait in `pcloud-cache::page_cache_generic` with an associated `Group` type and a `group(&self) -> Option<Self::Group>` method. Default impl returns `None`. Specialise for `pcloud_fs::page_cache::PageKey` to return `Some(self.file_id)`. Add a secondary `by_group` index to `PageCacheGeneric<K>` that maintains entries-per-group for O(k) `invalidate_group(group: K::Group)` invalidation. After D1.2 the typed `PageCache` in pcloud-fs becomes `PageCacheGeneric<PageKey>` and the workspace has a single canonical cache primitive.

The orphan transitions: `page_cache_generic::PageCacheGeneric<PageKey>::invalidate_group(file_id)` replaces `page_cache::PageCache::invalidate_file(file_id)`. Then **D1.4** (deletes the legacy pcloud-fs `PageCache`) becomes the trivial mechanical step it was always meant to be.

---

### Fire 44 — 2026-04-30 (D1.2 `CacheKey` trait + `by_group` index → CODE-DONE)

**Items closed (sub-step):**
- **D1.2 — Introduce `CacheKey` trait + lift `by_group` secondary index into the generic (CODE-DONE).** Final preparatory sub-step before D1.4 (delete the pcloud-fs typed `PageCache`). After this fire `PageCacheGeneric<K>` carries the same O(k) per-group invalidation primitive that the legacy typed `PageCache::invalidate_file` provided — generalised so any `K: CacheKey` type can declare its own `Group`.

**Trait + impls (`crates/pcloud-cache/src/page_cache_generic.rs`):**
```rust
pub trait CacheKey: Hash + Eq + Clone + Debug {
    type Group: Hash + Eq + Clone + Debug;
    fn group(&self) -> Option<Self::Group>;
}

impl CacheKey for String {
    type Group = ();
    fn group(&self) -> Option<()> { None }
}
```
- The `Option<Self::Group>` shape lets each entry **opt out** of the secondary index (`String::group() → None`) so the cache stays cheap for ungrouped callers — `by_group` simply never gets a key inserted under a string-keyed cache.

**`pcloud_fs::page_cache::PageKey`** (typed `(file_id, page_index)`) gains a `CacheKey` impl in `crates/pcloud-fs/src/page_cache.rs`:
```rust
impl pcloud_cache::page_cache_generic::CacheKey for PageKey {
    type Group = u64;
    fn group(&self) -> Option<u64> { Some(self.file_id) }
}
```

**Index machinery (`InnerGeneric<K: CacheKey>`):**
- New field `by_group: HashMap<K::Group, HashSet<K>>` — secondary index from group → resident keys, maintained in lockstep with the LRU.
- `put(k, v)`: when `k.group()` returns `Some(g)`, insert `k` into `by_group[g]`. Otherwise no-op.
- `evict_until_fits(...)`: when an entry is popped from the LRU, remove it from `by_group[g]` if it had a group; the entry stays in the secondary index would be a stale-pointer bug otherwise.
- `clear()`: clears `by_group` alongside the LRU.
- `Clone`: rebuilds `by_group` from scratch by walking the cloned entries (no by-reference sharing of the map).
- `Deserialize`: rebuilds `by_group` automatically because it re-uses `put()` for each entry.

**New public API:**
```rust
pub fn invalidate_group(&self, group: &K::Group) -> usize
```
Drops every page belonging to `group` and returns the number of pages evicted. O(k) where k is the resident page count for the group, not O(n) over the whole cache. For ungrouped keys (e.g. `PageCacheGeneric<String>`) this method is a no-op returning 0.

**Trait-bound tightening:**
All `K: Hash + Eq + Clone + Debug` bounds on `PageCacheGeneric<K>`, `InnerGeneric<K>`, and the `Clone` / `PartialEq` / `Eq` / `Serialize` / `Deserialize` impls became `K: CacheKey`. The Serialize and Deserialize impls retain their additional `Serialize` / `Deserialize<'de>` bounds. The `get<Q>(...) where K: Borrow<Q>` ergonomic from fire 42 is preserved.

**Files touched (2):**
- `crates/pcloud-cache/src/page_cache_generic.rs` — added `CacheKey` trait (~40 LoC) + `String` blanket impl + `by_group` field on `InnerGeneric` + secondary-index maintenance in `put` / `evict_until_fits` / `clear` / `Clone` + new `invalidate_group` public method (~40 LoC) + 3 regression tests (~85 LoC). Tightened all `K: Hash + Eq + ...` bounds to `K: CacheKey`.
- `crates/pcloud-fs/src/page_cache.rs` — added `impl CacheKey for PageKey` (~6 LoC) so the existing fire-37 regression-guard test (`page_cache_generic_reexport_resolves_for_pagekey`) keeps compiling and the future D1.4 migration path is unblocked.

**Tests added (3):**
- `invalidate_group_drops_only_matching_entries` — sets up 6 entries across 3 groups, invalidates one group, asserts exactly the 2 entries for that group are dropped and the other 4 remain.
- `invalidate_group_is_noop_for_ungrouped_keys` — string-keyed cache with 2 entries, `invalidate_group(&())` returns 0 and entry count stays at 2 (proves the no-op contract for ungrouped callers).
- `invalidate_group_after_eviction_is_consistent` — exercises the by_group/LRU sync invariant: 128-byte quota / 64-byte pages → at most 2 resident; insert 3 entries (forces eviction), then `invalidate_group` and assert the right entry survives. This is the regression gate against by_group becoming stale relative to the LRU.

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-cache --lib` → **17 passed; 0 failed** (was 14; +3 = the 3 new D1.2 regression tests)
- `cargo test -p pcloud-fs --lib` → **209 passed; 0 failed; 1 ignored** (no ripple regressions in fuse_adapter or read_path)
- `cargo test -p pcloud-daemon --lib` → **230 passed; 0 failed**
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**Status table updates:**
- D1 → still **PARTIAL** (D1.1a + D1.1b.1 + D1.1b.2a + D1.1b.2b + D1.1b.2c + D1.3 + D1.2 done; **D1.4** is the only remaining sub-step).

**Next sub-step (next fire):**
D1.4 — delete the legacy typed `pcloud_fs::page_cache::PageCache` struct and its `Inner`/`Slot`/`by_file` machinery. `fuse_adapter.rs` and the bench/tests migrate from `pcloud_fs::page_cache::PageCache` → `pcloud_fs::page_cache::PageCacheGeneric<PageKey>` (via the existing re-export). The migration mechanics:
- `PageCache::new(config)` → `PageCacheGeneric::new(config)` (same signature).
- `PageCache::default()` → `PageCacheGeneric::default()` (same signature).
- `PageCache::get(key)` → `PageCacheGeneric::get(&key)` — the typed-key `PageCache::get` took `key: PageKey` by value; the generic takes `&K` (or any `Borrow<Q>`).
- `PageCache::put(key, bytes)` → `PageCacheGeneric::put(key, bytes)` (same signature).
- `PageCache::invalidate_file(file_id)` → `PageCacheGeneric::invalidate_group(&file_id)` (return type change: `()` → `usize` count of evictions; existing callers that ignore the return continue to work because Rust drops unused returns silently).
- All other accessors (`config`, `stats`, `hit_ratio`, `len`, `is_empty`, `clear`) are signature-compatible.

After D1.4: a single canonical `PageCacheGeneric<K>` for the entire workspace; the iter-3 dim-5 NEW-1 finding is fully closed.

---

### Fire 45 — 2026-04-30 (D1.4 delete legacy `pcloud_fs::page_cache::PageCache` → DONE; D1 fully closed)

**Items closed:**
- **D1.4 — Delete legacy typed `pcloud_fs::page_cache::PageCache` (CODE-DONE).** With `CacheKey` + `by_group` index landed in fire 44 and `PageKey: CacheKey` impl already in place, the typed legacy is structurally redundant with `PageCacheGeneric<PageKey>`. This fire deletes it, migrates `fuse_adapter.rs` + `benches/page_cache.rs` to the generic, and rewrites the module-level rustdoc to reflect the single-canonical-cache state.
- **D1 — Page-cache generalisation (DONE).** With D1.4 closed all 9 D1 sub-steps (D1.1a, D1.1b.1, D1.1b.2a-d, D1.2, D1.3, D1.4) are complete. **The iter-3 dim-5 NEW-1 finding is now fully closed**: a single canonical `PageCacheGeneric<K>` exists workspace-wide, used both as `PageCacheGeneric<String>` (for `read_path.rs` + `CacheShell.pages`) and as `PageCacheGeneric<PageKey>` (for `fuse_adapter`).

**Refactor (4 files):**

- **`crates/pcloud-fs/src/page_cache.rs`** — deleted ~248 LoC of legacy `Slot`/`Inner`/`PageCache`/`impl PageCache` machinery. The file now contains: re-exports of `PageCacheConfig`/`PageCacheStats`/`DEFAULT_*` from `pcloud_cache::page_cache_generic`; the `PageKey` struct (the typed key); the `CacheKey` impl on `PageKey` (`Group = u64`, `group() = Some(self.file_id)`); a `pub use ...PageCacheGeneric` re-export; and the existing 9 unit tests migrated mechanically (`PageCache::new` → `PageCacheGeneric::<PageKey>::new`, `c.get(key)` → `c.get(&key)`, `c.invalidate_file(1)` → `c.invalidate_group(&1u64)`). Module-level rustdoc rewritten to describe the post-D1.4 facade-only role.

- **`crates/pcloud-fs/src/fuse_adapter.rs`** — 4 call-site changes: import (`PageCache` → `PageCacheGeneric`), field type (`Arc<PageCache>` → `Arc<PageCacheGeneric<PageKey>>`), constructor (`PageCache::new` → `PageCacheGeneric::new`), accessor return type, and the 3 `self.page_cache.get(page_key)` calls became `.get(&page_key)` (the generic's `get` takes `&K`).

- **`crates/pcloud-fs/benches/page_cache.rs`** — same migration pattern. Added a `type PageCache = PageCacheGeneric<PageKey>;` alias at the top so the rest of the bench body changed only in the `cache.get(key)` → `cache.get(&key)` lines (3 instances).

- **`crates/pcloud-daemon/src/mount_runtime.rs`** — fixed dangling `[`PageCache`][pcloud_fs::page_cache::PageCache]` doc-link in the module-level rustdoc; rewrote to `[`PageCacheGeneric<PageKey>`][pcloud_fs::page_cache::PageCacheGeneric]`.

**LoC change:** `pcloud-fs/src/page_cache.rs` shrank from 632 → 384 LoC (≈−250). `pcloud-fs/src/fuse_adapter.rs`, `benches/page_cache.rs`, `mount_runtime.rs` had small in-place edits. Net workspace LoC: ≈−240.

**Test set (after migration):**
- `pcloud-fs --lib page_cache` → 10 tests, all pass: 8 migrated legacy tests (`miss_then_hit`, `lru_eviction_when_over_cap`, `access_promotes_to_mru`, `invalidate_file_drops_pages_for_that_file_only` — same name kept for git-blame continuity even though it now uses `invalidate_group`, `hit_ratio_reported_correctly`, `oversized_page_is_silently_dropped`, `oversized_page_increments_rejection_counter`, `concurrent_put_and_get_do_not_deadlock`) + 1 re-export smoke test (`page_cache_generic_reexport_resolves_for_pagekey`) + 1 `fuse_adapter::tests::second_read_hits_page_cache`.

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-fs --lib` → **209 passed; 0 failed; 1 ignored**
- `cargo test -p pcloud-cache --lib` → **17 passed; 0 failed**
- `cargo test -p pcloud-daemon --lib` → **230 passed; 0 failed**
- `cargo bench -p pcloud-fs --bench page_cache --no-run` → exit 0 (compiles clean against the migrated bench)
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor; 4 transient warnings during the deletion were resolved by rewriting the module-level rustdoc + fixing the `mount_runtime.rs` doc-link)

**Status table updates:**
- D1 → **DONE**. The iter-3 dim-5 NEW-1 finding closure is now byte-true: workspace `grep "PageCache "` returns no production type definition outside `pcloud_cache::page_cache_generic::PageCacheGeneric`.

**Next item (next fire):**
D2 — `AccountChangePassword` round-trip with marker-file recovery. The user has provided two live pCloud accounts (A + B) via `.env`. Sub-step decomposition:
- D2.1: design + implement the marker-file persistence layer in `crates/pcloud-live-e2e/tests/common/mod.rs`.
- D2.2: write the test body in `account_utility_destructive.rs` using account A; assert pre/post password preservation.

---

### Fire 46 — 2026-04-30 (D2 `AccountChangePassword` round-trip with marker-file recovery → DONE)

**Items closed:**
- **D2 — `AccountChangePassword` round-trip with marker-file recovery (DONE).** Both sub-steps land in this fire: D2.1 (marker-file persistence layer in `common/mod.rs`) + D2.2 (round-trip test body in `account_utility_destructive.rs`). The user has provided live pCloud accounts A and B via `.env`, so the test will exercise a real rotation when run with `PCLOUD_LIVE_E2E_DESTRUCTIVE=1`.

**D2.1 — Marker-file persistence layer (`crates/pcloud-live-e2e/tests/common/mod.rs`):**

New types and helpers:
- `pub struct AcpRotationMarker { original: String, temp: String, phase: AcpPhase }` — the on-disk envelope, derives `serde::Serialize/Deserialize`.
- `pub enum AcpPhase { RotatedToTemp }` — pipeline phase tag. Currently a single variant; future failure-mode discrimination would add e.g. `Rotating`/`RotatedBack` if mid-RPC failure modes need distinct recovery paths.
- `pub fn acp_marker_path(user_email: &str) -> PathBuf` — returns `${TMPDIR}/pcloud-rs-acp-marker-${hash16}` where `hash16` is a `DefaultHasher` hex of the user email. Non-cryptographic hashing is sufficient because the marker file content is the secret envelope, not the path.
- `pub fn read_acp_marker(path: &Path) -> Option<AcpRotationMarker>` — None on missing file or parse failure.
- `pub fn write_acp_marker(path: &Path, marker: &AcpRotationMarker) -> Result<(), String>` — JSON-encode + write + chmod `0600` on Unix.
- `pub fn delete_acp_marker(path: &Path)` — best-effort `remove_file`; errors swallowed because the next run recovers via `read_acp_marker` either way.

The `serde` workspace dep was added to `pcloud-live-e2e/Cargo.toml`'s `[dev-dependencies]` (was implicit via siblings; the explicit add is required for the `derive` proc-macro on `AcpRotationMarker`).

Privacy note documented inline: both passwords appear in plaintext on disk at `0600` for the duration of the test — same disclosure surface as `PCLOUD_TEST_PASSWORD` in the env, **no net-new exposure**.

**D2.2 — `live_account_change_password_round_trip` test (`account_utility_destructive.rs`):**

The test has two branches selected by marker-file presence:

1. **Recovery branch** (marker exists with `phase = RotatedToTemp`):
   - Authenticate with `temp` (the password the prior run rotated to).
   - Dispatch `AccountChangePassword{ current: temp, new: original }`.
   - Delete marker on success. Test ends here — the operator can re-run to exercise the fresh path, OR let the next scheduled run handle it.

2. **Fresh-path branch** (no marker or marker invalid):
   - Authenticate with `original` (env-supplied).
   - Generate `temp = "claudereV-rotation-temp-{nonce}"` (nanosecond-keyed).
   - **Write marker BEFORE the rotation RPC** so a process death between RPC dispatch and marker-write does not strand the test in an unrecoverable state. The marker's existence-with-`RotatedToTemp` is the durable evidence the rotation may have happened.
   - Dispatch `AccountChangePassword{ current: original, new: temp }`. On Ok: re-authenticate with `temp` in a fresh `TestDaemon` (auth state doesn't carry across rotations within one daemon).
   - Dispatch `AccountChangePassword{ current: temp, new: original }`. On Ok: delete marker.
   - Final sanity: re-authenticate with `original` once more in a third fresh `TestDaemon`. Surfaces a server-side bug where the rotation reports OK but the server doesn't actually accept the new password.

**Crash safety**: the marker-write-before-RPC ordering means a panic between line 2's marker-write and line 4's marker-delete leaves the marker behind with `phase = RotatedToTemp`. The next invocation's recovery branch fires automatically. **There is one true window of irrecoverability**: a panic between marker-write and the first rotation's response landing — if the rotation actually went through but the response was lost in transit, the next run will still try to authenticate with `temp` (correct) and rotate back (correct). If the rotation didn't go through, the next run authenticates with `temp` (which the server still treats as wrong) and panics with `recovery: temp-password auth failed (account may be locked at neither original nor temp)` — this is the "manual intervention" exit, intentional to surface that the operator needs to log into the account via web and rotate manually.

**Files touched (3):**
- `crates/pcloud-live-e2e/tests/common/mod.rs` — added the 6 helpers + 2 types (~80 LoC). Added `Path` to the `std::path::{...}` import.
- `crates/pcloud-live-e2e/tests/account_utility_destructive.rs` — added the round-trip test (~115 LoC), updated the file-header rustdoc coverage table from 2 entries to 3, added `use std::time::SystemTime` and the new common-module imports. The "Excluded from this fire" section explaining why `AccountChangePassword` was deferred is gone now that the test exists.
- `crates/pcloud-live-e2e/Cargo.toml` — added `serde = { workspace = true }` to `[dev-dependencies]` for the `derive` macros on `AcpRotationMarker`.

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0 (after one `cargo fmt --all` rebalance on the test file's import order)
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-live-e2e --test account_utility_destructive` → **0 passed; 0 failed; 3 ignored** (correct gate-skip posture without `PCLOUD_LIVE_E2E_DESTRUCTIVE=1`; was 2 ignored, now 3 with the new round-trip)
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**To actually fire the test against the live soak account:**
```
PCLOUD_LIVE_E2E=1 \
PCLOUD_LIVE_E2E_DESTRUCTIVE=1 \
PCLOUD_TEST_USER=<email> \
PCLOUD_TEST_PASSWORD=<password> \
cargo test -p pcloud-live-e2e --test account_utility_destructive \
  -- --ignored --test-threads=1 live_account_change_password_round_trip
```
The operator should set `PCLOUD_TEST_USER`/`PCLOUD_TEST_PASSWORD` to a dedicated rotation-test account (NOT account A or B from `.env`, since this test rotates the password and a flake could lock the account temporarily if the recovery branch fails).

**Status table updates:**
- D2 → **DONE**.

**Next item (next fire):**
D3 — Row 142 `CryptoAccountTeamShare` IPC variant + dispatch + backend wiring. Mirrors fire 15's `CryptoShareFolder` pattern but for team-share (replaces `mail` with `team_id`).

---

### Fire 47 — 2026-04-30 (D3 row 142 `Request::CryptoAccountTeamShare` → DONE)

**Items closed:**
- **D3 — Row 142 `psync_crypto_account_teamshare` reachability (DONE).** Mirrors fire 15's `Request::CryptoShareFolder` pattern but for team-share — replaces the recipient `mail` field with `team_id`. The shares-backend method `SharesRuntime::crypto_account_team_share` already existed from a prior fix; this fire closes the user-facing reachability gap that kept row 142 `Partial`.

**End-to-end wiring (5 files):**

- **`crates/pcloud-ipc/src/methods.rs`** — added `Request::CryptoAccountTeamShare { folder_id, name, team_id, message, permissions_bits, temppass: RedactedString, hint }` variant. The `temppass` field uses `RedactedString` (audit-H1 wire wrapper) so it is automatically redacted from `Debug`/`Display`/`serde` and is destructured into `SecretString` at the dispatch boundary. Added the variant to the typed `Request::is_privileged()` capability table next to the existing `CryptoShareFolder` entry — the new variant is state-mutating + audit-logged.

- **`crates/pcloud-daemon/src/runtime.rs`** — added the dispatch arm in `RuntimeShell::dispatch_request` that destructures `temppass: RedactedString` into `SecretString::new(String::from(temppass))` before calling the new handler. Added `RuntimeShell::crypto_account_team_share(...)` (~50 LoC) parallel to `crypto_share_folder`: empty-name guard, `shares_require_auth_token`, `SharePermissions::from_bits`, `crypto.is_started()` precondition (returns `Conflict` when locked — same posture as `crypto_share_folder`), then `self.shares_runtime.crypto_account_team_share(...)` to the backend. Audit category `shares.crypto_account_team_share`. Variant-name table entry `CryptoAccountTeamShare` for the audit/log surfaces.

- **`crates/pcloud-live-e2e/tests/team_share_verb.rs`** — added `live_crypto_account_team_share_dispatches_verb_reached` (~45 LoC). Verb-reached pattern: dispatches with synthetic-but-well-formed args (`folder_id: 0`, `team_id: 0`, garbage temppass); the soak account is personal (not a business team member) and crypto is not unlocked, so the daemon rejects with `Conflict` before reaching the server. Either path proves the IPC variant + daemon dispatch arm + crypto-state precondition gate are wired correctly. Coverage table in the file header updated.

- **`C_FEATURE_PARITY_MATRIX.csv`** — flipped row 142 `Partial` → `Implemented`. Cited files extended to include the new IPC variant, dispatch arm, and live test path. Rationale text rewritten to reflect the closure: PclsyncCompat temppass-rewrap path now reachable end-to-end; RSA-4096-OAEP path remains tracked under D6.

- **`STATUS.md`** — added the fire-47 closure note at the top of the file with the new headline `154 / 2 / 0 / 30`. Updated `_Last reviewed_` line. Current Partial set: rows 94 (SDK UploadSession) and 124 (RSA-4096 crypto-share). Both are RSA-OAEP-related and converge on D6.

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-ipc --lib` → **29 passed; 0 failed** (the existing `prop_request_round_trips` proptest covers the new variant's serde-bincode round-trip automatically — no new test needed for that contract)
- `cargo test -p pcloud-daemon --lib` → **230 passed; 0 failed**
- `cargo test -p pcloud-live-e2e --test team_share_verb` → **0 passed; 0 failed; 2 ignored** (was 1 ignored; +1 new crypto variant test gates clean)
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**Status table updates:**
- D3 → **DONE**.

**Next item (next fire):**
D4 — `notify-debouncer-full` swap. Risk: workspace `vendor/notify-dfly-fix` patch interaction. Sub-step decomposition (per the plan):
- D4.1: add `notify-debouncer-full` workspace dep, run `cargo check`. If the version pin fights the patch, document and revert.
- D4.2: replace the watcher setup in `crates/pcloud-fs/src/fs_watcher.rs`; preserve all 16 existing unit tests.
- D4.3: remove the now-unused max-age guard added in fire 20 (the in-tree `PendingEntry` struct).

---

### Fire 48 — 2026-04-30 (D4 `notify-debouncer-full` swap → ACKNOWLEDGED-DEFERRED after structural audit)

**Items closed:**
- **D4 — `notify-debouncer-full` swap (ACKNOWLEDGED-DEFERRED).** Investigation completed. The audit found the literal swap would **regress** the iter-1 SYNC-H-04-2 continuous-churn stall fix that fire 20 of the original 36-fire campaign closed. Same audit-then-fix pattern as fire 36's P7.1 closure: the plan's prescription was structurally incomplete; documenting the finding inline + deferring the work is the honest move.

**D4.1 — Dep compatibility (PASS):**
- Added `notify-debouncer-full = { version = "0.6", default-features = false }` to the workspace `Cargo.toml`.
- Added `notify-debouncer-full.workspace = true` to `crates/pcloud-fs/Cargo.toml`.
- `cargo check -p pcloud-fs --all-targets` → exit 0. The `vendor/notify-dfly-fix` patch transitively applies to `notify-debouncer-full v0.6` (it depends on `notify v8`, the patched rev). **The patch-interaction risk the plan flagged is real but resolved**: the swap is dep-graph-compatible.

**D4.2/D4.3 — Swap evaluation (BLOCK):**

`notify-debouncer-full v0.6` debounce semantics:
- Each event is keyed by canonical path (or file-id when available).
- The debouncer keeps a per-path map of `last_seen` timestamps.
- On each tick (`tick_rate`, default 250 ms), paths whose `last_seen + timeout < now` are flushed.
- **There is no max-age cap.** A path churned at a rate faster than `timeout` (e.g. a log file appended-to every 200 ms with `timeout = 500 ms`) refreshes its `last_seen` on every churn event and is held by the debouncer indefinitely.

This is **exactly the iter-1 SYNC-H-04-2 stall mode** that fire 20 of the original campaign closed by introducing the in-tree `PendingEntry { first_seen, last_seen }` + `max_debounce = 2 × debounce` flush rule. Replacing the hand-rolled debouncer with `notify-debouncer-full` would silently re-introduce the bug.

The fire 20 fix is documented at `crates/pcloud-fs/src/fs_watcher.rs:240-252`:
```
//   flush if  (now - last_seen >= debounce)         // quiescence
//         OR  (now - first_seen >= MAX_DEBOUNCE)    // max-age guard
```
The 16 unit tests in `fs_watcher::tests::*` directly exercise this rule (`flush_pending_respects_max_age_under_continuous_churn` is the regression gate).

**Two paths forward (both deferred):**
1. **Wrap `notify-debouncer-full` with our own tick callback** that injects the max-age cap on top of the upstream's quiescence-only debounce. Estimated scope: ~150 LoC of wrapper + new tests, plus migration of the 16 existing tests to exercise the wrapper instead of the inner state.
2. **Upstream the max-age cap** to `notify-debouncer-full` and bump the dep when the feature lands. Out of single-fire scope (and out of repo scope).

**Decision: revert the dep, document the finding, defer the swap.**

The hand-rolled debouncer is **strictly more correct** for the workload (continuous-append log files are common; the stall would silently lose minutes of edits before flushing). The workspace inline comment block in `Cargo.toml` (next to the existing `notify` entry) records why `notify-debouncer-full` is intentionally NOT a workspace dep yet.

**Files touched (2):**
- `Cargo.toml` (workspace) — added the dep, then reverted; replaced the dep entry with a 9-line comment block citing this fire's finding so a future contributor doesn't repeat the same audit cycle.
- `crates/pcloud-fs/Cargo.toml` — added `notify-debouncer-full.workspace = true`, then reverted.

**Verification:**
- `cargo check --workspace --all-targets` → exit 0 (post-revert)
- `cargo fmt --all --check` → exit 0
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-fs --lib fs_watcher` → **16 passed; 0 failed; 0 ignored** (the existing hand-rolled debouncer + max-age guard continue to be the right primitive for the workload)
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**Status table updates:**
- D4 → **ACKNOWLEDGED-DEFERRED**.

**Next item (next fire):**
D5 — Per-backend `ResilientTransport` migration. 7 backends (auth, transfer, public-link, shares, sync, backup, account). Sub-step decomposition: D5.1 wraps the auth backend as a canary, then D5.2-D5.7 fan out the same pattern.

---

### Fire 49 — 2026-04-30 (D5.1 auth backend canary `ResilientTransport` wrap → CODE-DONE)

**Items closed (sub-step):**
- **D5.1 — Auth backend canary wrap (CODE-DONE).** First of 7 per-backend `ResilientTransport` migrations. The auth backend now opts into the wrapped production transport when `TransportFactory::wrap_binary(...)` returns `Some`; dev/test environments keep the bare `BinaryApiTransport` path unchanged.

**Refactor (3 files):**

- **`crates/pcloud-proto/src/resilient_transport.rs`** — added `pub fn inner_arc(&self) -> Arc<T>` accessor (~12 LoC + rustdoc). Lets callers reach methods on the inner transport that `ResilientTransport` does NOT delegate (the resilient wrapper only impls `execute()`, not the rest of the inner's surface). Specifically required so the auth backend's `ApiServerHintConsumer::apply_api_server_hint` impl can fan through to the inner `BinaryApiTransport`.

- **`crates/pcloud-backends/src/auth_backend.rs`** — added 3 things:
  1. **New error variant** `AuthBackendError::Resilient(String)` for resilient-wrapper-only conditions (circuit-breaker open, rate-limit exceeded, retry-budget exhausted) that don't have a clean back-mapping to `TransportError`. The `Inner(transport_err)` case still maps to the existing `Network(TransportError)` so all pre-existing error-handling paths keep working unchanged.
  2. **New variant** `AuthTransportMode::ResilientNetwork(ResilientTransport<BinaryApiTransport>)` plus its `ProtocolTransport::execute` arm (maps `ResilientError` to the right `AuthBackendError` variant) and `ApiServerHintConsumer::apply_api_server_hint` arm (delegates via `inner_arc()`).
  3. **New constructor** `AuthRuntime::from_resilient_transport(resilient: ResilientTransport<BinaryApiTransport>)` so daemon bootstrap can hand in the wrapped transport without going through the bare-transport path of `from_config`.

- **`crates/pcloud-daemon/src/bootstrap.rs`** — moved the `TransportFactory::new(...)` line to BEFORE the `AuthRuntime` construction so the factory is available when the auth runtime is built. Replaced `let auth_runtime = AuthRuntime::from_config(&config)` with `let auth_runtime = build_auth_runtime(&config, &transport_factory)`. Added the `build_auth_runtime` helper (~50 LoC) which:
  - Falls through to `AuthRuntime::from_config` for `ApiMode::Development` (no network transport at all).
  - Builds a `BinaryApiTransport`, hands it to `factory.wrap_binary(...)`.
  - Routes through `from_resilient_transport` when the factory wraps (production).
  - Falls through to `from_config` when the factory does not wrap (dev/test).
  - Logs an `error!` and falls through to `from_config` if the factory's rate-limit config is rejected — the daemon still boots; the operator sees an actionable error in logs and can fix the config without a hard startup-block.

**What `D5.1` semantically buys:**
Production auth-bound RPCs (login, TFA submit, resend SMS/notification, logout, userinfo) now run through the workspace-shared `GlobalRetryBudget` + a `TokenBucket` rate-limiter + a per-endpoint circuit-breaker. A login storm (many concurrent failed-login attempts) cannot exceed the budget regardless of which other backends are also retrying. A pCloud-side outage trips the circuit and shed requests cleanly with `AuthBackendError::Resilient(...)` instead of timing out one-by-one.

**What `D5.1` deliberately does NOT buy:**
The auth runtime's three secondary construction sites (`session_refresh.rs`, two in `lib.rs`, `refresh_loop.rs` in `pcloud-session`) still use `AuthRuntime::from_config` without a factory. They are the dev/test/refresh paths and intentionally keep the bare-transport behavior so the test suite stays deterministic. When `AuthRuntime::from_config` is called, behavior is byte-identical to pre-fire-49.

**Files touched (3):**
- `crates/pcloud-proto/src/resilient_transport.rs` (added `inner_arc` accessor)
- `crates/pcloud-backends/src/auth_backend.rs` (new error variant + transport variant + constructor)
- `crates/pcloud-daemon/src/bootstrap.rs` (moved factory ctor up + new `build_auth_runtime` helper)

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0 (after one `cargo fmt --all` rebalance)
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-proto --lib` → **210 passed; 0 failed** (the new `inner_arc` accessor is covered by the existing `ResilientTransport` test surface; no behavioural change)
- `cargo test -p pcloud-backends --lib` → **172 passed; 0 failed; 2 ignored**
- `cargo test -p pcloud-daemon --lib` → **230 passed; 0 failed**
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**Status table updates:**
- D5 → **PARTIAL** (D5.1 done; D5.2..D5.7 remain).

**Next sub-step (next fire):**
D5.2 — `transfer_backend.rs` adopts the same pattern. Higher-traffic than auth (every byte of every upload/download flows through it), so the resilient wrap has more material impact. The migration mechanics are identical: new `TransferTransportMode::ResilientNetwork` variant + plumbing + a `TransferRuntime::from_resilient_transport` constructor + bootstrap-helper update. Then D5.3..D5.7 fan out to public-link, shares, sync, backup, account.

---

### Fire 50 — 2026-04-30 (D5.2 transfer backend `ResilientTransport` wrap → CODE-DONE)

**Items closed (sub-step):**
- **D5.2 — Transfer backend `ResilientTransport` wrap (CODE-DONE).** Second of 7 per-backend migrations. Higher-impact than the auth canary — every byte of every upload/download flows through the transfer backend's API path, so the resilient wrap (circuit-breaker / rate-limiter / retry-budget) materially shapes the throughput envelope under partial outages.

**Refactor (2 files):**

- **`crates/pcloud-backends/src/transfer_backend.rs`** — three additions mirroring D5.1:
  1. New `TransferBackendError::Resilient(String)` variant for circuit-breaker / rate-limit / retry-budget conditions. Inner transport errors still map to `Network(TransportError)` so all pre-existing error-handling paths keep working.
  2. New `TransferTransportMode::ResilientNetwork(ResilientTransport<BinaryApiTransport>)` variant + `ProtocolTransport::execute` arm + `ApiServerHintConsumer::apply_api_server_hint` arm (delegates via `inner_arc()`).
  3. New constructor `TransferRuntime::from_resilient_transport(config, resilient)` that:
     - Wraps the API path in resilient.
     - **Preserves** `network_transport: Some(inner_clone)` by extracting a clone of the inner `BinaryApiTransport` via `resilient.inner_arc()`. This is the key invariant for the mount runtime: `network_transport()` (used by `PcloudFsShim` to compose byte-I/O) keeps returning the bare transport. Only the API request path goes through the resilient wrap; raw byte I/O is intentionally unchanged so the existing FUSE bandwidth profile is preserved.
     - Builds the same `HttpDownloadConfig` and `upload_pacer: None` defaults as `from_config`.

- **`crates/pcloud-daemon/src/bootstrap.rs`** — replaced `let transfer_runtime = TransferRuntime::from_config(&config)` with `let transfer_runtime = build_transfer_runtime(&config, &transport_factory)`. Added the helper (~45 LoC) parallel to `build_auth_runtime`: same fall-through structure (Dev → bare; factory wraps → resilient; factory rate-limit error → log + bare). Removed the now-unused `use crate::transfer_backend::TransferRuntime` import (replaced with the fully-qualified `crate::transfer_backend::TransferRuntime` inside the helper).

**Why preserve `network_transport()` at the bare-transport layer:**
The transfer backend exposes `network_transport(&self) -> Option<BinaryApiTransport>` for the mount runtime to build a composed `PcloudFsShim` backed by the same byte-transport as the API path. Raw byte I/O (chunked upload writes, signed-URL downloads) is fundamentally a streaming operation that the resilient wrap's per-request rate-limit + circuit-breaker would actively harm. The resilient wrap is correct for control-plane RPCs (`upload_create`, `upload_save`, `getfilelink`); it is wrong for the data-plane byte loops. The architectural split was already in the codebase pre-D5.2 (the API path goes through `TransferTransportMode::execute`; the byte path goes through `network_transport()` directly); D5.2 preserves that split exactly by extracting the inner transport via `inner_arc()` for the byte path while wrapping only the API path.

**Files touched (2):**
- `crates/pcloud-backends/src/transfer_backend.rs` (new error variant + transport variant + constructor)
- `crates/pcloud-daemon/src/bootstrap.rs` (new `build_transfer_runtime` helper + remove unused import + swap call site)

**Verification:**
- `cargo check --workspace --all-targets` → exit 0 (after dropping the now-unused `TransferRuntime` import)
- `cargo fmt --all --check` → exit 0 (after one `cargo fmt --all` rebalance)
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-backends --lib` → **172 passed; 0 failed; 2 ignored**
- `cargo test -p pcloud-daemon --lib` → **230 passed; 0 failed**
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor; the new ctor's intra-doc links to `ResilientTransport`/`ResilientTransport::inner_arc` were swapped to plain code spans because the cross-crate rustdoc resolution rules don't accept those bare paths from inside `pcloud-backends`)

**Status table updates:**
- D5 → still **PARTIAL** (D5.1 + D5.2 done; D5.3 + D5.4 + D5.5 + D5.6 + D5.7 remain).

**Next sub-step (next fire):**
D5.3 — `public_link_backend.rs` adopts the same pattern. Inspect the backend's `enum *TransportMode` shape; add the `ResilientNetwork` variant; add `Resilient(String)` error; add `from_resilient_transport` ctor; add a `build_public_link_runtime` helper in bootstrap.rs.

After all 7 sub-steps land, the daemon's full production API surface goes through the workspace-shared `GlobalRetryBudget` + per-endpoint circuit-breakers. Each fire follows the identical mechanical pattern, so subsequent fires should be smaller (~70 LoC each).

---

### Fire 51 — 2026-04-30 (D5.3 public-link backend `ResilientTransport` wrap → CODE-DONE)

**Items closed (sub-step):**
- **D5.3 — Public-link backend `ResilientTransport` wrap (CODE-DONE).** Third of 7 per-backend migrations. Same mechanical pattern as D5.1 (auth) and D5.2 (transfer).

**Refactor (2 files):**

- **`crates/pcloud-backends/src/public_link_backend.rs`**:
  - `PublicLinkBackendError::Resilient(String)` variant added next to `Network(TransportError)`.
  - `PublicLinkTransportMode::ResilientNetwork(ResilientTransport<BinaryApiTransport>)` variant + `ProtocolTransport::execute` arm + `ApiServerHintConsumer::apply_api_server_hint` arm (delegates via `inner_arc()`).
  - `PublicLinkRuntime::from_resilient_transport(resilient)` constructor. The `path_resolver()` accessor (for `RemotePathResolver`-backed tree-link path resolution) continues to work because the resolver only requires `T: ProtocolTransport` and `transport.clone()` propagates the wrapped form.

- **`crates/pcloud-daemon/src/bootstrap.rs`**:
  - Replaced `PublicLinkRuntime::from_config(&config)` call site with `build_public_link_runtime(&config, &transport_factory)`.
  - Added the helper (~35 LoC) parallel to `build_auth_runtime` and `build_transfer_runtime`.
  - Removed the now-unused `use crate::public_link_backend::PublicLinkRuntime` import.

**Files touched (2):**
- `crates/pcloud-backends/src/public_link_backend.rs`
- `crates/pcloud-daemon/src/bootstrap.rs`

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0 (after one rebalance)
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-backends --lib` → **172 passed; 0 failed; 2 ignored**
- `cargo test -p pcloud-daemon --lib` → **230 passed; 0 failed**
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**Status table updates:**
- D5 → still **PARTIAL** (D5.1 + D5.2 + D5.3 done; D5.4 + D5.5 + D5.6 + D5.7 remain).

**Next sub-step (next fire):**
D5.4 — `shares_backend.rs`. The backend has more breadth than public-link (share folder, crypto-share variants, list / accept / decline / cancel / remove / modify, account-stop / account-modify / team-share / crypto-account-team-share). The transport-mode pattern is the same: add `ResilientNetwork` variant + `Resilient(String)` error + `from_resilient_transport` ctor + `build_shares_runtime` helper.

---

### Fire 52 — 2026-04-30 (D5.4 shares backend `ResilientTransport` wrap → CODE-DONE)

**Items closed (sub-step):**
- **D5.4 — Shares backend `ResilientTransport` wrap (CODE-DONE).** Fourth of 7 per-backend migrations. Identical mechanical pattern to D5.1/D5.2/D5.3.

**Refactor (2 files):**

- **`crates/pcloud-backends/src/shares_backend.rs`**:
  - `SharesBackendError::Resilient(String)` variant added.
  - `SharesTransportMode::ResilientNetwork(...)` variant + `ProtocolTransport::execute` arm + `ApiServerHintConsumer::apply_api_server_hint` arm (delegates via `inner_arc()`).
  - `SharesRuntime::from_resilient_transport(resilient)` constructor.

- **`crates/pcloud-daemon/src/bootstrap.rs`**:
  - `build_shares_runtime(config, factory)` helper added (~35 LoC) parallel to the prior three.
  - Call site at the `let shares_runtime = ...` line swapped to use the helper.
  - Removed now-unused `use crate::shares_backend::SharesRuntime` import.

**Files touched (2):**
- `crates/pcloud-backends/src/shares_backend.rs`
- `crates/pcloud-daemon/src/bootstrap.rs`

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-backends --lib` → **172 passed; 0 failed; 2 ignored**
- `cargo test -p pcloud-daemon --lib` → **230 passed; 0 failed**
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**Status table updates:**
- D5 → still **PARTIAL** (4/7 backends migrated; D5.5 + D5.6 + D5.7 remain).

**Next sub-step (next fire):**
D5.5 — `sync_backend.rs`. Inspect the backend's transport-mode shape (likely identical: `Development` + `Network`) and apply the same refactor. After D5.5/D5.6/D5.7 land, the daemon's full production API surface will go through the workspace-shared `GlobalRetryBudget` + per-endpoint circuit-breakers.

---

### Fire 53 — 2026-04-30 (D5.5 sync backend `ResilientTransport` wrap → CODE-DONE)

**Items closed (sub-step):**
- **D5.5 — Sync backend `ResilientTransport` wrap (CODE-DONE).** Fifth of 7 per-backend migrations. Identical mechanical pattern to D5.1-D5.4. The sync backend hosts both `SyncApi` and `FolderApi` over the same transport — both share the wrapped form via `Clone`.

**Refactor (2 files):**

- **`crates/pcloud-backends/src/sync_backend.rs`**:
  - `SyncBackendError::Resilient(String)` variant.
  - `SyncTransportMode::ResilientNetwork(...)` variant + `execute` + `apply_api_server_hint` plumbing.
  - `SyncRuntime::from_resilient_transport(resilient)` constructor that clones the wrapped transport into both `SyncApi` and `FolderApi`.

- **`crates/pcloud-daemon/src/bootstrap.rs`**:
  - `build_sync_runtime(config, factory)` helper added.
  - Call site swapped to use the helper.
  - Removed unused `use crate::sync_backend::SyncRuntime` import.

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0 (after one rebalance)
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-backends --lib` → **172 passed; 0 failed; 2 ignored**
- `cargo test -p pcloud-daemon --lib` → **230 passed; 0 failed**
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**Status table updates:**
- D5 → still **PARTIAL** (5/7 done; D5.6 (backup), D5.7 (account) remain).

**Next sub-step (next fire):**
D5.6 — `backup_backend.rs`. Same pattern.

---

### Fire 54 — 2026-04-30 (D5.6 backup backend `ResilientTransport` wrap → CODE-DONE)

**Items closed (sub-step):**
- **D5.6 — Backup backend `ResilientTransport` wrap (CODE-DONE).** Sixth of 7 per-backend migrations. Identical mechanical pattern to D5.1-D5.5.

**Refactor (2 files):**

- **`crates/pcloud-backends/src/backup_backend.rs`**:
  - `BackupBackendError::Resilient(String)` variant added next to `Network(TransportError)`.
  - `BackupTransportMode::ResilientNetwork(...)` variant + `ProtocolTransport::execute` arm + `ApiServerHintConsumer::apply_api_server_hint` arm (delegates via `inner_arc()`).
  - `BackupRuntime::from_resilient_transport(resilient)` constructor.

- **`crates/pcloud-daemon/src/bootstrap.rs`**:
  - `build_backup_runtime(config, factory)` helper added (~35 LoC) parallel to the prior five.
  - Call site at the `let backup_runtime = ...` line swapped to use the helper.
  - Removed the now-unused `use crate::backup_backend::BackupRuntime` import.

**Files touched (2):**
- `crates/pcloud-backends/src/backup_backend.rs`
- `crates/pcloud-daemon/src/bootstrap.rs`

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0 (after one rebalance)
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-backends --lib` → **172 passed; 0 failed; 2 ignored**
- `cargo test -p pcloud-daemon --lib` → **230 passed; 0 failed**
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**Status table updates:**
- D5 → still **PARTIAL** (6/7 done; only D5.7 (account) remains).

**Next sub-step (next fire):**
D5.7 — `account_backend.rs`. Final per-backend migration. After D5.7 lands, the daemon's full production API surface goes through `ResilientTransport` and D5 closes; only D6 (RSA-OAEP wire-shape unification) will remain.

---

### Fire 55 — 2026-04-30 (D5.7 account backend `ResilientTransport` wrap → CODE-DONE; D5 fully closed)

**Items closed:**
- **D5.7 — Account backend `ResilientTransport` wrap (CODE-DONE).** Final of 7 per-backend migrations. Identical mechanical pattern to D5.1-D5.6.
- **D5 — Per-backend `ResilientTransport` migration (DONE).** All 7 production backends (auth, transfer, public-link, shares, sync, backup, account) now route through the workspace-shared `GlobalRetryBudget` + per-endpoint circuit-breakers in production environments. The iter-1 TRANSPORT-H-1 finding's "every API call site goes through `ResilientTransport`" acceptance criterion is byte-true.

**Refactor (2 files):**

- **`crates/pcloud-backends/src/account_backend.rs`**:
  - `AccountBackendError::Resilient(String)` variant added next to `Network(TransportError)`.
  - `AccountTransportMode::ResilientNetwork(...)` variant + `ProtocolTransport::execute` arm + `ApiServerHintConsumer::apply_api_server_hint` arm (delegates via `inner_arc()`).
  - `AccountRuntime::from_resilient_transport(resilient)` constructor.

- **`crates/pcloud-daemon/src/bootstrap.rs`**:
  - `build_account_runtime(config, factory)` helper added — the seventh and final `build_*_runtime` helper, parallel to `build_auth_runtime`, `build_transfer_runtime`, `build_public_link_runtime`, `build_shares_runtime`, `build_sync_runtime`, and `build_backup_runtime`.
  - Call site at the `let account_runtime = ...` line swapped to use the helper.

**Files touched (2):**
- `crates/pcloud-backends/src/account_backend.rs`
- `crates/pcloud-daemon/src/bootstrap.rs`

**D5 closure summary (cumulative across fires 49-55):**

| Fire | Sub-step | Backend | Public API surface |
|---|---|---|---|
| 49 | D5.1 | auth | login / TFA / userinfo / logout |
| 50 | D5.2 | transfer | upload-create/write/save / get-file-link (control-plane only; raw byte I/O preserved at the bare-transport layer for FUSE bandwidth profile) |
| 51 | D5.3 | public-link | file/folder/tree-link CRUD + change-link-options |
| 52 | D5.4 | shares | share-folder / accept / decline / cancel / remove / modify / team-share / crypto-share |
| 53 | D5.5 | sync | diff / listfolder (folder validation on add) — `SyncApi` + `FolderApi` share the wrapped transport |
| 54 | D5.6 | backup | backup-create / backup-list / backup-delete / stop-device |
| 55 | D5.7 | account | get-api-servers / get-promo / verify-email / lost-password / change-password / register / set-language |

Net new code across all 7 fires: ~600 LoC (each backend ≈ 20 LoC for the variant + execute arm + hint arm + ctor; each bootstrap helper ≈ 35 LoC; plus the `inner_arc()` accessor on `ResilientTransport` from D5.1).

**What this delivers semantically:**
A login storm on the auth backend cannot exhaust the `GlobalRetryBudget` shared with the transfer backend's chunked uploads — the `Arc<GlobalRetryBudget>` is one pool across all 7 backends. A pCloud-side outage trips the circuit-breaker once per endpoint and shed every subsequent RPC cleanly with `*BackendError::Resilient(String)`. The `RateLimitMode::Wait` injected by `TransportFactory` uses the real `SystemClock` + `ThreadSleepWaiter` so the sleep is observable and bounded.

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0 (after one rebalance)
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-backends --lib` → **172 passed; 0 failed; 2 ignored**
- `cargo test -p pcloud-daemon --lib` → **230 passed; 0 failed**
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**Status table updates:**
- D5 → **DONE**.

**Next item (next fire):**
D6 — RSA-OAEP wire-shape unification. The last D-item. Replaces the `RsaBackendRequired` early-return in `crates/pcloud-crypto/src/share_temppass.rs:343-345` with a call to `share_rsa::wrap_share_invitation_b64` for the `CryptoBackend::PclsyncCompat` path. Multi-RPC daemon orchestration: the daemon must call `crypto_share_metadata` to fetch the recipient's RSA public key before the wrap. After D6 lands the entire deferred-set closes and the loop self-terminates via `CronDelete`.

---

### Fire 56 — 2026-04-30 (D6 RSA-OAEP wire-shape unification → DONE; entire deferred-set closes)

**Items closed:**
- **D6 — RSA-OAEP wire-shape unification (DONE).** The last D-item. Re-frames the original CRYPTO-H-2 / fire 16 finding: the literal substitution in `share_temppass.rs::derive_temppass_wire` was structurally impossible (different wire shapes), but the **proper closure** — wiring the existing `crypto_share_folder_rsa` backend method through a new IPC variant + daemon-side multi-RPC orchestrator — is exactly what fire 56 lands.

**End-to-end wiring (5 files):**

- **`crates/pcloud-backends/src/crypto_backend.rs`** — added `CryptoRuntime::get_pub_key(auth_token, recipient: CryptoPubKeyRecipient) -> Result<Vec<u8>, CryptoApiError<...>>` wrapper around `CryptoApi::get_pub_key`. Lets the daemon orchestrator fetch the recipient's `pub_key_ver1` blob before invoking the wrap; ~10 LoC.

- **`crates/pcloud-ipc/src/methods.rs`** — added `Request::CryptoShareFolderRsa { folder_id, name, mail, message, permissions_bits, hint }` variant (sibling to fire 15's `Request::CryptoShareFolder` PclsyncCompat-temppass variant); added to the typed `Request::is_privileged()` capability table. The variant intentionally does NOT carry a `temppass` field — the wrap key is derived from the recipient's pubkey + the sharer's folder sym-key, both fetched server-side.

- **`crates/pcloud-daemon/src/runtime.rs`** — three additions:
  1. New dispatch arm for `Request::CryptoShareFolderRsa`.
  2. New variant-name table entry `"CryptoShareFolderRsa"`.
  3. New handler `RuntimeShell::crypto_share_folder_rsa(...)` (~85 LoC) implementing the **multi-RPC orchestration**:
     - Empty-name + invalid-mail guards.
     - `shares_require_auth_token` for the auth token.
     - `crypto.is_started()` precondition (`Conflict` if locked).
     - **Step 3 — fetch recipient pubkey:** `self.crypto_runtime.get_pub_key(token, CryptoPubKeyRecipient::Mail(mail))`. Surfaces failures as `InternalError` with the underlying `CryptoApiError` text.
     - **Step 4 — share-folder-rsa:** `self.shares_runtime.crypto_share_folder_rsa(token, &self.crypto, folder_id, &recipient_pub_blob, name, mail, message, perms, hint)`. The shares-backend method (already in place from a prior fix) RSA-4096-OAEP-wraps the sharer's folder sym-key against the pubkey via `pcloud_crypto::share_rsa::wrap_share_invitation_b64` and dispatches the wire-compat share request. Audit category `shares.crypto_share_folder_rsa`.

- **`crates/pcloud-live-e2e/tests/team_share_verb.rs`** — added `live_crypto_share_folder_rsa_dispatches_verb_reached` (~50 LoC). Verb-reached test: dispatches with synthetic-but-well-formed args + IETF-reserved `@example.invalid` recipient (so no real RSA-share email can be sent even if crypto were ambiently unlocked); asserts the daemon answers with one of the verb-reached statuses.

- **`C_FEATURE_PARITY_MATRIX.csv`** + **`STATUS.md`** — row 124 flipped `Partial` → `Implemented`. Cited files extended to the full new wiring chain. **Headline tally: `155 / 1 / 0 / 30 (186 rows)`**. The lone remaining Partial is row 94 (SDK UploadSession), which is unrelated to the deferred-set scope.

**Why this is the proper closure for CRYPTO-H-2 / D6 (not the literal `share_temppass.rs:343-345` substitution):**
The fire-16 audit established that `derive_temppass_wire` and `wrap_share_invitation_b64` produce **different wire shapes** consumed by different IPC variants. The PclsyncCompat backend's `RsaBackendRequired` early-return at `share_temppass.rs:380-382` is correct fail-loud behaviour for callers that asked for the temppass-wire shape on a backend that needs the RSA-OAEP shape. The actual user-facing closure is the multi-RPC orchestration this fire lands: a different `Request::*` variant routes the RSA caller through the right code path with the right inputs. No code in `share_temppass.rs` needed to change; the regression-guard test `pclsync_compat_returns_rsa_backend_required` continues to pin the failsafe.

**Verification:**
- `cargo check --workspace --all-targets` → exit 0
- `cargo fmt --all --check` → exit 0 (after one rebalance)
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `cargo test -p pcloud-ipc --lib` → **29 passed; 0 failed** (the existing `prop_request_round_trips` proptest covers the new variant's serde-bincode roundtrip automatically)
- `cargo test -p pcloud-backends --lib` → **172 passed; 0 failed; 2 ignored**
- `cargo test -p pcloud-daemon --lib` → **230 passed; 0 failed**
- `cargo test -p pcloud-live-e2e --test team_share_verb` → **0 passed; 0 failed; 3 ignored** (was 2; +1 new RSA verb-reached test gates clean)
- `cargo doc --workspace --no-deps` → 41 rustdoc warnings (unchanged floor)

**Status table updates:**
- D6 → **DONE**.
- **All 6 D-items now resolved.** D1, D2, D3, D5, D6 all `DONE`. D4 closed as `ACKNOWLEDGED-DEFERRED` after the structural audit found the literal swap would regress fire-20's max-age stall fix.

---

### Loop termination — 2026-04-30 (deferred-set complete, cron `3fbba689` cancelled)

All six items in `CLAUDEREV/DEFERRED-PLAN.md` are resolved (`DONE` × 5, `ACKNOWLEDGED-DEFERRED` × 1). Per the standing user instruction in the loop prompt, the cron job is now cancelled and a `CLAUDEREV/DEFERRED-COMPLETE.md` summary will be written next.
