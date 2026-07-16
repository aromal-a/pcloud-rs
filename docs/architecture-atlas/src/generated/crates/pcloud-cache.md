# `pcloud-cache`

**Maturity:** Evolving product surface

**Version:** `0.1.0`

**Directory:** `crates/pcloud-cache`

**Manifest:** [`crates/pcloud-cache/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/Cargo.toml)

In-memory caching primitives for pcloud-rs metadata and transfer state.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_cache` | lib | [`crates/pcloud-cache/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/lib.rs) |
| `warm_cache` | example | [`crates/pcloud-cache/examples/warm_cache.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/examples/warm_cache.rs) |

## Direct dependencies

`aes-gcm`, `getrandom`, `hmac`, `lru`, `serde`, `serde_json`, `sha2`, `thiserror`

## Cargo features

No declared package features.

## File inventory (10)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-cache/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/Cargo.toml) | Cargo manifest | T2.3.a — CacheCipher: AES-256-GCM with HKDF-SHA256 key derivation |
| [`crates/pcloud-cache/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/README.md) | documentation | pcloud-cache |
| [`crates/pcloud-cache/examples/warm_cache.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/examples/warm_cache.rs) | example | Constructs a `PageCacheGeneric&lt;String&gt;`, warms it with 16 pages, then |
| [`crates/pcloud-cache/src/checksum_cache.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/checksum_cache.rs) | Rust module | Entry-count bounded cache of local file checksums. |
| [`crates/pcloud-cache/src/cipher.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs) | Rust module | T2.3.a — Encryption-at-rest for the local cache. |
| [`crates/pcloud-cache/src/eviction.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/eviction.rs) | Rust module | Declarative eviction-policy tag attached to a \[`crate::CacheShell`\]. |
| [`crates/pcloud-cache/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/lib.rs) | library root | pcloud-cache |
| [`crates/pcloud-cache/src/page_cache_generic.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs) | Rust module | Key-typed generic LRU page cache. Canonical string-keyed page cache |
| [`crates/pcloud-cache/src/sealed_blob.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/sealed_blob.rs) | Rust module | T2.3.b — disk-shaped wrapper around \[`CacheCipher`\]. |
| [`crates/pcloud-cache/src/staging.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs) | Rust module | In-memory staging buffer for in-flight local writes. |

## Rust declaration index (133 total; 54 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `main` | `private` | fn | [`crates/pcloud-cache/examples/warm_cache.rs:14`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/examples/warm_cache.rs#L14) | Read the source/rustdoc for the exact contract. |
| `WARM_PAGES` | `private` | const | [`crates/pcloud-cache/examples/warm_cache.rs:22`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/examples/warm_cache.rs#L22) | Read the source/rustdoc for the exact contract. |
| `PAGE_BYTES` | `private` | const | [`crates/pcloud-cache/examples/warm_cache.rs:23`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/examples/warm_cache.rs#L23) | Read the source/rustdoc for the exact contract. |
| `ChecksumCache` | `pub` | struct | [`crates/pcloud-cache/src/checksum_cache.rs:31`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/checksum_cache.rs#L31) | Entry-count bound for the checksum cache. `entry_limit` is the maximum number of `(path, sha1)` pairs the enc… |
| `default` | `private` | fn | [`crates/pcloud-cache/src/checksum_cache.rs:38`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/checksum_cache.rs#L38) | Read the source/rustdoc for the exact contract. |
| `MASTER_KEY_LEN` | `pub` | const | [`crates/pcloud-cache/src/cipher.rs:47`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L47) | Length of the master key used to derive the cache cipher key. The auth-vault layer hands the daemon a 32-byte… |
| `CACHE_KEY_LEN` | `pub` | const | [`crates/pcloud-cache/src/cipher.rs:49`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L49) | Length of the derived AES-256-GCM key. |
| `NONCE_LEN` | `pub` | const | [`crates/pcloud-cache/src/cipher.rs:51`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L51) | AES-GCM nonce length (12 bytes per RFC 5116). |
| `TAG_LEN` | `pub` | const | [`crates/pcloud-cache/src/cipher.rs:53`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L53) | AES-GCM authentication tag length (16 bytes). |
| `PAGE_CACHE_DOMAIN` | `pub` | const | [`crates/pcloud-cache/src/cipher.rs:57`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L57) | Domain-separation label used as HKDF `info` for the page-cache layer. Different cache layers (page vs staging… |
| `STAGING_DOMAIN` | `pub` | const | [`crates/pcloud-cache/src/cipher.rs:59`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L59) | Domain-separation label for the staging layer. |
| `HmacSha256` | `private` | type | [`crates/pcloud-cache/src/cipher.rs:61`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L61) | Read the source/rustdoc for the exact contract. |
| `CipherError` | `pub` | enum | [`crates/pcloud-cache/src/cipher.rs:65`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L65) | Errors raised by the cipher. |
| `CacheCipher` | `pub` | struct | [`crates/pcloud-cache/src/cipher.rs:93`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L93) | AES-256-GCM cipher derived from a master key + domain string. |
| `fmt` | `private` | fn | [`crates/pcloud-cache/src/cipher.rs:98`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L98) | Read the source/rustdoc for the exact contract. |
| `derive` | `pub` | fn | [`crates/pcloud-cache/src/cipher.rs:115`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L115) | Derive a per-domain cipher from a 32-byte master key. `domain` is the HKDF `info` parameter — pass \[`PAGE_CAC… |
| `seal` | `pub` | fn | [`crates/pcloud-cache/src/cipher.rs:134`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L134) | Seal `plaintext` with a fresh random nonce. Returns the self-contained on-disk record `nonce \|\| ciphertext \|\|… |
| `open` | `pub` | fn | [`crates/pcloud-cache/src/cipher.rs:164`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L164) | Open a sealed record produced by \[`Self::seal`\]. `aad` MUST match the value passed at seal time. # Errors - \[… |
| `overhead` | `pub` | fn | [`crates/pcloud-cache/src/cipher.rs:184`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L184) | Length overhead `seal` adds to its input. Useful for callers that need to size on-disk pages. Always `NONCE_L… |
| `hkdf_sha256` | `private` | fn | [`crates/pcloud-cache/src/cipher.rs:192`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L192) | HKDF-SHA256 (RFC 5869). `salt = \[\]` collapses to the "extract-no-salt" mode which is the standard choice for… |
| `tests` | `private` | mod | [`crates/pcloud-cache/src/cipher.rs:228`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L228) | Read the source/rustdoc for the exact contract. |
| `fixed_master` | `private` | fn | [`crates/pcloud-cache/src/cipher.rs:231`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L231) | Read the source/rustdoc for the exact contract. |
| `derive_rejects_bad_master_length` | `private` | fn | [`crates/pcloud-cache/src/cipher.rs:240`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L240) | Read the source/rustdoc for the exact contract. |
| `derive_is_deterministic` | `private` | fn | [`crates/pcloud-cache/src/cipher.rs:249`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L249) | Read the source/rustdoc for the exact contract. |
| `different_domains_produce_different_keys` | `private` | fn | [`crates/pcloud-cache/src/cipher.rs:257`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L257) | Read the source/rustdoc for the exact contract. |
| `seal_open_round_trip` | `private` | fn | [`crates/pcloud-cache/src/cipher.rs:265`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L265) | Read the source/rustdoc for the exact contract. |
| `seal_output_is_not_plaintext_on_disk` | `private` | fn | [`crates/pcloud-cache/src/cipher.rs:275`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L275) | Read the source/rustdoc for the exact contract. |
| `nonce_is_fresh_per_seal` | `private` | fn | [`crates/pcloud-cache/src/cipher.rs:292`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L292) | Read the source/rustdoc for the exact contract. |
| `open_with_wrong_aad_fails` | `private` | fn | [`crates/pcloud-cache/src/cipher.rs:303`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L303) | Read the source/rustdoc for the exact contract. |
| `open_with_wrong_key_fails` | `private` | fn | [`crates/pcloud-cache/src/cipher.rs:311`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L311) | Read the source/rustdoc for the exact contract. |
| `open_rejects_tampered_ciphertext` | `private` | fn | [`crates/pcloud-cache/src/cipher.rs:322`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L322) | Read the source/rustdoc for the exact contract. |
| `open_rejects_truncated_record` | `private` | fn | [`crates/pcloud-cache/src/cipher.rs:333`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L333) | Read the source/rustdoc for the exact contract. |
| `debug_does_not_leak_key` | `private` | fn | [`crates/pcloud-cache/src/cipher.rs:346`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L346) | Read the source/rustdoc for the exact contract. |
| `hkdf_sha256_matches_rfc_5869_test_vector_1` | `private` | fn | [`crates/pcloud-cache/src/cipher.rs:355`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L355) | Read the source/rustdoc for the exact contract. |
| `hex_decode` | `private` | fn | [`crates/pcloud-cache/src/cipher.rs:370`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/cipher.rs#L370) | Read the source/rustdoc for the exact contract. |
| `EvictionPolicy` | `pub` | enum | [`crates/pcloud-cache/src/eviction.rs:22`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/eviction.rs#L22) | Advisory eviction policy selector. # Example ``` use pcloud_cache::eviction::EvictionPolicy; // SizeBound is… |
| `checksum_cache` | `pub` | mod | [`crates/pcloud-cache/src/lib.rs:42`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/lib.rs#L42) | Read the source/rustdoc for the exact contract. |
| `cipher` | `pub` | mod | [`crates/pcloud-cache/src/lib.rs:43`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/lib.rs#L43) | Read the source/rustdoc for the exact contract. |
| `eviction` | `pub` | mod | [`crates/pcloud-cache/src/lib.rs:44`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/lib.rs#L44) | Read the source/rustdoc for the exact contract. |
| `page_cache_generic` | `pub` | mod | [`crates/pcloud-cache/src/lib.rs:52`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/lib.rs#L52) | Key-typed generic LRU page cache. Canonical string-keyed page-cache primitive for this crate. `pcloud-fs::pag… |
| `sealed_blob` | `pub` | mod | [`crates/pcloud-cache/src/lib.rs:53`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/lib.rs#L53) | Read the source/rustdoc for the exact contract. |
| `staging` | `pub` | mod | [`crates/pcloud-cache/src/lib.rs:54`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/lib.rs#L54) | Read the source/rustdoc for the exact contract. |
| `CRATE_NAME` | `pub` | const | [`crates/pcloud-cache/src/lib.rs:64`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/lib.rs#L64) | Human-readable crate name. Used by telemetry / logging so the originating crate can be identified without pul… |
| `CacheShell` | `pub` | struct | [`crates/pcloud-cache/src/lib.rs:73`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/lib.rs#L73) | Aggregate holder for every cache primitive used by the daemon. `CacheShell` composes the individual caches (p… |
| `default` | `private` | fn | [`crates/pcloud-cache/src/lib.rs:90`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/lib.rs#L90) | Read the source/rustdoc for the exact contract. |
| `cache_page` | `pub` | fn | [`crates/pcloud-cache/src/lib.rs:117`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/lib.rs#L117) | Insert a page into the shared `PageCacheGeneric&lt;String&gt;`. Accepts anything convertible into `String` for the… |
| `stage_file` | `pub` | fn | [`crates/pcloud-cache/src/lib.rs:133`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/lib.rs#L133) | Stage an in-flight write buffer for `path`. See \[`staging::StagingCache::stage`\] for the eviction contract. #… |
| `summary` | `pub` | fn | [`crates/pcloud-cache/src/lib.rs:149`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/lib.rs#L149) | One-line human-readable summary of current cache state. Intended for logs and diagnostics only — the exact fo… |
| `tests` | `private` | mod | [`crates/pcloud-cache/src/lib.rs:164`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/lib.rs#L164) | Read the source/rustdoc for the exact contract. |
| `summary_reflects_cached_and_staged_state` | `private` | fn | [`crates/pcloud-cache/src/lib.rs:168`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/lib.rs#L168) | Read the source/rustdoc for the exact contract. |
| `DEFAULT_PAGE_SIZE` | `pub` | const | [`crates/pcloud-cache/src/page_cache_generic.rs:43`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L43) | Default FUSE-aligned page size. Matches the 64 KiB page size used by the reference C client's block cache. Re… |
| `DEFAULT_MAX_BYTES` | `pub` | const | [`crates/pcloud-cache/src/page_cache_generic.rs:47`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L47) | Default cache cap: 128 MiB. Re-exported by `pcloud_fs::page_cache::DEFAULT_MAX_BYTES`. |
| `PageCacheConfig` | `pub` | struct | [`crates/pcloud-cache/src/page_cache_generic.rs:52`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L52) | Runtime configuration shared by every `PageCacheGeneric&lt;K&gt;` and by `pcloud_fs::page_cache::PageCache` (which… |
| `default` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:62`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L62) | Read the source/rustdoc for the exact contract. |
| `PageCacheStats` | `pub` | struct | [`crates/pcloud-cache/src/page_cache_generic.rs:73`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L73) | Observed cache statistics. Re-exported by `pcloud_fs::page_cache::PageCacheStats`. |
| `hit_ratio` | `pub` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:91`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L91) | Lifetime hit ratio computed from `hits / (hits + misses)`. Returns `0.0` when no reads have been observed. |
| `Slot` | `private` | struct | [`crates/pcloud-cache/src/page_cache_generic.rs:103`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L103) | Cached page entry. Holds the page bytes behind an \[`Arc`\] so a `get` returns a cheap refcount bump instead of… |
| `CacheKey` | `pub` | trait | [`crates/pcloud-cache/src/page_cache_generic.rs:130`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L130) | Optional grouping discriminant for cache keys. Implementing types declare a `Group` associated type and a `gr… |
| `Group` | `private` | type | [`crates/pcloud-cache/src/page_cache_generic.rs:133`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L133) | Discriminant used to bucket entries for O(k) per-group invalidation. `()` is the conventional "no grouping" c… |
| `group` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:138`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L138) | Return the group this key participates in, or `None` if the key is ungrouped. The returned value is stored in… |
| `Group` | `private` | type | [`crates/pcloud-cache/src/page_cache_generic.rs:145`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L145) | Read the source/rustdoc for the exact contract. |
| `group` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:146`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L146) | Read the source/rustdoc for the exact contract. |
| `InnerGeneric` | `private` | struct | [`crates/pcloud-cache/src/page_cache_generic.rs:152`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L152) | Read the source/rustdoc for the exact contract. |
| `new` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:175`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L175) | Read the source/rustdoc for the exact contract. |
| `evict_until_fits` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:187`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L187) | Read the source/rustdoc for the exact contract. |
| `PageCacheGeneric` | `pub` | struct | [`crates/pcloud-cache/src/page_cache_generic.rs:213`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L213) | LRU page cache parameterised on the key type. |
| `default` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:224`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L224) | Read the source/rustdoc for the exact contract. |
| `clone` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:253`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L253) | Read the source/rustdoc for the exact contract. |
| `eq` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:298`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L298) | Read the source/rustdoc for the exact contract. |
| `PageCacheGenericWire` | `private` | struct | [`crates/pcloud-cache/src/page_cache_generic.rs:362`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L362) | Read the source/rustdoc for the exact contract. |
| `serialize` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:377`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L377) | Read the source/rustdoc for the exact contract. |
| `deserialize` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:405`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L405) | Read the source/rustdoc for the exact contract. |
| `new` | `pub` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:436`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L436) | Construct a cache. Zero-valued `page_size` is replaced with \[`DEFAULT_PAGE_SIZE`\]; `max_bytes` is floored so… |
| `config` | `pub` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:450`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L450) | Active configuration (after normalisation by \[`Self::new`\]). |
| `get` | `pub` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:462`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L462) | Lookup. On hit promotes to MRU; returns the page bytes via a cheap \[`Arc`\] clone. Accepts any `&amp;Q` where `K:… |
| `put` | `pub` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:479`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L479) | Insert or replace a page. Pages larger than `max_bytes` are silently dropped and counted in `bytes_rejected_o… |
| `invalidate_group` | `pub` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:520`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L520) | Drop every page belonging to `group`. Returns the number of pages evicted. O(k) where k is the resident page… |
| `clear` | `pub` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:538`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L538) | Clear the entire cache. Does not reset hit/miss counters. |
| `stats` | `pub` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:549`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L549) | Snapshot the current cache statistics. |
| `hit_ratio` | `pub` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:564`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L564) | Lifetime hit ratio. `0.0` when no reads have been observed. |
| `len` | `pub` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:570`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L570) | Number of pages currently resident. |
| `is_empty` | `pub` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:576`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L576) | Whether no pages are resident. |
| `tests` | `private` | mod | [`crates/pcloud-cache/src/page_cache_generic.rs:582`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L582) | Read the source/rustdoc for the exact contract. |
| `cfg` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:585`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L585) | Read the source/rustdoc for the exact contract. |
| `round_trips_value` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:593`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L593) | Read the source/rustdoc for the exact contract. |
| `evicts_under_byte_quota` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:602`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L602) | Read the source/rustdoc for the exact contract. |
| `records_oversized_rejection` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:612`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L612) | Read the source/rustdoc for the exact contract. |
| `clone_produces_independent_content_equal_cache` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:621`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L621) | Read the source/rustdoc for the exact contract. |
| `equality_excludes_stats_counters` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:634`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L634) | Read the source/rustdoc for the exact contract. |
| `serde_round_trip_preserves_entries_and_stats` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:651`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L651) | Read the source/rustdoc for the exact contract. |
| `serde_round_trip_preserves_mru_ordering` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:680`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L680) | Read the source/rustdoc for the exact contract. |
| `typed_key_struct_works_too` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:704`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L704) | Read the source/rustdoc for the exact contract. |
| `TestKey` | `private` | struct | [`crates/pcloud-cache/src/page_cache_generic.rs:709`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L709) | Read the source/rustdoc for the exact contract. |
| `Group` | `private` | type | [`crates/pcloud-cache/src/page_cache_generic.rs:714`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L714) | Read the source/rustdoc for the exact contract. |
| `group` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:715`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L715) | Read the source/rustdoc for the exact contract. |
| `invalidate_group_drops_only_matching_entries` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:728`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L728) | Read the source/rustdoc for the exact contract. |
| `GKey` | `private` | struct | [`crates/pcloud-cache/src/page_cache_generic.rs:730`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L730) | Read the source/rustdoc for the exact contract. |
| `Group` | `private` | type | [`crates/pcloud-cache/src/page_cache_generic.rs:735`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L735) | Read the source/rustdoc for the exact contract. |
| `group` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:736`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L736) | Read the source/rustdoc for the exact contract. |
| `invalidate_group_is_noop_for_ungrouped_keys` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:786`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L786) | Read the source/rustdoc for the exact contract. |
| `invalidate_group_after_eviction_is_consistent` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:798`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L798) | Read the source/rustdoc for the exact contract. |
| `GKey` | `private` | struct | [`crates/pcloud-cache/src/page_cache_generic.rs:802`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L802) | Read the source/rustdoc for the exact contract. |
| `Group` | `private` | type | [`crates/pcloud-cache/src/page_cache_generic.rs:804`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L804) | Read the source/rustdoc for the exact contract. |
| `group` | `private` | fn | [`crates/pcloud-cache/src/page_cache_generic.rs:805`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/page_cache_generic.rs#L805) | Read the source/rustdoc for the exact contract. |
| `seal_blob_for_disk` | `pub` | fn | [`crates/pcloud-cache/src/sealed_blob.rs:38`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/sealed_blob.rs#L38) | Seal `plaintext` for on-disk storage under `blob_name`. # Errors See \[`CipherError`\]. |
| `open_blob_from_disk` | `pub` | fn | [`crates/pcloud-cache/src/sealed_blob.rs:54`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/sealed_blob.rs#L54) | Open an on-disk record produced by \[`seal_blob_for_disk`\]. `blob_name` MUST match the value passed at seal ti… |
| `sealed_blob_overhead` | `pub` | fn | [`crates/pcloud-cache/src/sealed_blob.rs:65`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/sealed_blob.rs#L65) | Bytes the wrapper adds to the input. Callers can pre-size on- disk buffers as `plaintext.len() + sealed_blob_… |
| `tests` | `private` | mod | [`crates/pcloud-cache/src/sealed_blob.rs:70`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/sealed_blob.rs#L70) | Read the source/rustdoc for the exact contract. |
| `fixed_master` | `private` | fn | [`crates/pcloud-cache/src/sealed_blob.rs:74`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/sealed_blob.rs#L74) | Read the source/rustdoc for the exact contract. |
| `round_trip_preserves_plaintext` | `private` | fn | [`crates/pcloud-cache/src/sealed_blob.rs:83`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/sealed_blob.rs#L83) | Read the source/rustdoc for the exact contract. |
| `rename_attack_fails_aead_check` | `private` | fn | [`crates/pcloud-cache/src/sealed_blob.rs:92`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/sealed_blob.rs#L92) | Read the source/rustdoc for the exact contract. |
| `cross_domain_decrypt_fails` | `private` | fn | [`crates/pcloud-cache/src/sealed_blob.rs:103`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/sealed_blob.rs#L103) | Read the source/rustdoc for the exact contract. |
| `sealed_record_does_not_contain_plaintext` | `private` | fn | [`crates/pcloud-cache/src/sealed_blob.rs:115`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/sealed_blob.rs#L115) | Read the source/rustdoc for the exact contract. |
| `sealed_blob_overhead_matches_cipher` | `private` | fn | [`crates/pcloud-cache/src/sealed_blob.rs:128`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/sealed_blob.rs#L128) | Read the source/rustdoc for the exact contract. |
| `empty_plaintext_round_trips` | `private` | fn | [`crates/pcloud-cache/src/sealed_blob.rs:134`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/sealed_blob.rs#L134) | Read the source/rustdoc for the exact contract. |
| `corrupt_sealed_record_fails_open` | `private` | fn | [`crates/pcloud-cache/src/sealed_blob.rs:144`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/sealed_blob.rs#L144) | Read the source/rustdoc for the exact contract. |
| `DEFAULT_MAX_OPEN_FILES` | `pub` | const | [`crates/pcloud-cache/src/staging.rs:28`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L28) | Default maximum number of distinct staged files. |
| `DEFAULT_MAX_BYTES` | `pub` | const | [`crates/pcloud-cache/src/staging.rs:31`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L31) | Default byte budget: 32 MiB — large enough for typical interactive edits while preventing a single large stag… |
| `StagingResult` | `pub` | enum | [`crates/pcloud-cache/src/staging.rs:35`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L35) | Outcome of a \[`StagingCache::stage`\] call. |
| `StagingCache` | `pub` | struct | [`crates/pcloud-cache/src/staging.rs:51`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L51) | Bounded staging buffer keyed by remote-relative path. |
| `default` | `private` | fn | [`crates/pcloud-cache/src/staging.rs:69`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L69) | Read the source/rustdoc for the exact contract. |
| `stage` | `pub` | fn | [`crates/pcloud-cache/src/staging.rs:102`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L102) | Stage `bytes` under `path`. Returns \[`StagingResult::Accepted`\] when the entry was admitted and \[`StagingResu… |
| `seed_unchecked` | `pub` | fn | [`crates/pcloud-cache/src/staging.rs:132`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L132) | Seed `bytes` under `path`, bypassing the byte-budget guard. Intended for tests and deterministic fixtures whe… |
| `get` | `pub` | fn | [`crates/pcloud-cache/src/staging.rs:156`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L156) | Return the staged buffer for `path`, or `None` if absent / evicted. # Example ``` use pcloud_cache::staging::… |
| `staged_count` | `pub` | fn | [`crates/pcloud-cache/src/staging.rs:173`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L173) | Number of staged files currently resident. # Example ``` use pcloud_cache::staging::StagingCache; let mut cac… |
| `resident_bytes` | `pub` | fn | [`crates/pcloud-cache/src/staging.rs:179`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L179) | Total bytes of resident staged data. |
| `evict_if_needed` | `private` | fn | [`crates/pcloud-cache/src/staging.rs:183`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L183) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-cache/src/staging.rs:196`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L196) | Read the source/rustdoc for the exact contract. |
| `stages_and_reads_file_buffers` | `private` | fn | [`crates/pcloud-cache/src/staging.rs:200`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L200) | Read the source/rustdoc for the exact contract. |
| `evicts_oldest_staged_file_when_limit_is_exceeded` | `private` | fn | [`crates/pcloud-cache/src/staging.rs:211`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L211) | Read the source/rustdoc for the exact contract. |
| `rejects_payload_exceeding_byte_budget` | `private` | fn | [`crates/pcloud-cache/src/staging.rs:224`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L224) | Read the source/rustdoc for the exact contract. |
| `byte_budget_evicts_oldest_when_exceeded_by_accumulation` | `private` | fn | [`crates/pcloud-cache/src/staging.rs:246`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L246) | Read the source/rustdoc for the exact contract. |
| `replace_updates_byte_tracking` | `private` | fn | [`crates/pcloud-cache/src/staging.rs:263`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-cache/src/staging.rs#L263) | Read the source/rustdoc for the exact contract. |

## Usage guidance

This is product code but not a frozen external library contract. Check current status and native qualification before deployment claims.
