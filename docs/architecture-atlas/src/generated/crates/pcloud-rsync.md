# `pcloud-rsync`

**Maturity:** Experimental / bounded

**Version:** `0.8.1-beta`

**Directory:** `crates/pcloud-rsync`

**Manifest:** [`crates/pcloud-rsync/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/Cargo.toml)

Rolling-hash + strong-hash block signatures and delta encoder for differential sync. T2.1 — only the modified blocks of an edit travel over the wire.

## Feature-family profile

**Why it exists.** Avoid retransmitting unchanged blocks when a large file changes locally.

**What it is good for.** Rolling weak hashes, strong block signatures, delta planning, and differential-upload strategy inputs.

**Why it is good at that job.** Rsync-style weak/strong matching finds reusable blocks in one pass while strong hashes protect against weak-hash collisions.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_rsync` | lib | [`crates/pcloud-rsync/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/lib.rs) |

## Direct dependencies

`serde`, `serde_json`, `sha2`, `thiserror`

## Cargo features

No declared package features.

## File inventory (5)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-rsync/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-rsync/src/delta.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs) | Rust module | Delta encoder: walk a local file against a remote |
| [`crates/pcloud-rsync/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/lib.rs) | library root | T2.1 — block signatures + rolling-hash primitives for |
| [`crates/pcloud-rsync/src/rolling.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs) | Rust module | Adler-32-style rolling hash. |
| [`crates/pcloud-rsync/src/signature.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs) | Rust module | Block-signature builder for differential sync. |

## Rust declaration index (58 total; 25 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `DeltaOp` | `pub` | enum | [`crates/pcloud-rsync/src/delta.rs:57`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L57) | One delta operation. The encoder emits a sequence of these to reconstruct a target file from a remote baselin… |
| `output_len` | `pub` | fn | [`crates/pcloud-rsync/src/delta.rs:78`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L78) | Number of bytes this op contributes to the reconstructed file. Used by the encoder to verify that the delta r… |
| `wire_payload` | `pub` | fn | [`crates/pcloud-rsync/src/delta.rs:89`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L89) | Bytes the op contributes to the on-wire payload. `CopyServer` is essentially free (it's a `(u32, u32)` header… |
| `compute_delta` | `pub` | fn | [`crates/pcloud-rsync/src/delta.rs:114`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L114) | Compute the delta from a local file against a remote \[`Signature`\]. Returns operations in order. # Behaviour… |
| `strong_hash` | `private` | fn | [`crates/pcloud-rsync/src/delta.rs:220`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L220) | Read the source/rustdoc for the exact contract. |
| `apply_delta` | `pub` | fn | [`crates/pcloud-rsync/src/delta.rs:232`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L232) | Apply a delta against a baseline buffer to reconstruct the original local file. Used by tests to assert lossl… |
| `tests` | `private` | mod | [`crates/pcloud-rsync/src/delta.rs:251`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L251) | Read the source/rustdoc for the exact contract. |
| `empty_local_yields_empty_delta` | `private` | fn | [`crates/pcloud-rsync/src/delta.rs:256`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L256) | Read the source/rustdoc for the exact contract. |
| `empty_signature_yields_one_new_bytes_op` | `private` | fn | [`crates/pcloud-rsync/src/delta.rs:263`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L263) | Read the source/rustdoc for the exact contract. |
| `delta_of_self_is_single_copy_chain` | `private` | fn | [`crates/pcloud-rsync/src/delta.rs:279`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L279) | Read the source/rustdoc for the exact contract. |
| `one_byte_edit_isolates_to_one_block_payload` | `private` | fn | [`crates/pcloud-rsync/src/delta.rs:301`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L301) | One-byte edit in a multi-block file should ship only the edited block worth of `NewBytes`, surrounded by `Cop… |
| `fully_disjoint_local_is_just_new_bytes` | `private` | fn | [`crates/pcloud-rsync/src/delta.rs:335`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L335) | Delta against a wholly disjoint baseline degrades to `NewBytes(local)` — the encoder is never worse than a fu… |
| `short_local_smaller_than_block_size` | `private` | fn | [`crates/pcloud-rsync/src/delta.rs:354`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L354) | Local file shorter than `block_size` has no full-window walks; encoder emits a single `NewBytes` (or matches… |
| `tail_block_match_emits_copy_for_tail` | `private` | fn | [`crates/pcloud-rsync/src/delta.rs:371`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L371) | Tail-block match: the local file's trailing partial window equals the baseline's tail block byte-for-byte. |
| `reconstruction_preserves_arbitrary_edits` | `private` | fn | [`crates/pcloud-rsync/src/delta.rs:388`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L388) | Reconstruction round-trip on a moderately-sized file with a single-block insertion. Demonstrates the bounded… |
| `delta_op_wire_payload_accounting` | `private` | fn | [`crates/pcloud-rsync/src/delta.rs:421`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L421) | Read the source/rustdoc for the exact contract. |
| `serde_roundtrip_delta_op` | `private` | fn | [`crates/pcloud-rsync/src/delta.rs:434`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/delta.rs#L434) | Read the source/rustdoc for the exact contract. |
| `delta` | `pub` | mod | [`crates/pcloud-rsync/src/lib.rs:48`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/lib.rs#L48) | Read the source/rustdoc for the exact contract. |
| `rolling` | `pub` | mod | [`crates/pcloud-rsync/src/lib.rs:49`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/lib.rs#L49) | Read the source/rustdoc for the exact contract. |
| `signature` | `pub` | mod | [`crates/pcloud-rsync/src/lib.rs:50`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/lib.rs#L50) | Read the source/rustdoc for the exact contract. |
| `MAGIC` | `pub` | const | [`crates/pcloud-rsync/src/rolling.rs:38`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L38) | Rollsum bias constant (librsync `rollsum.c` uses 31). Folded into both `a` and `b` to spread out hashes for l… |
| `MODULUS` | `pub` | const | [`crates/pcloud-rsync/src/rolling.rs:46`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L46) | Modulus for the running sums. Adler-32 uses 65521 (largest prime below 2^16), but rollsum/librsync uses 2^16… |
| `RollingHash` | `pub` | struct | [`crates/pcloud-rsync/src/rolling.rs:52`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L52) | 32-bit Adler-32-style rolling hash. Advance one byte at a time with \[`Self::roll`\] or recompute from scratch… |
| `default` | `private` | fn | [`crates/pcloud-rsync/src/rolling.rs:62`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L62) | Read the source/rustdoc for the exact contract. |
| `new` | `pub` | fn | [`crates/pcloud-rsync/src/rolling.rs:70`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L70) | Empty hash with zero window length. |
| `compute` | `pub` | fn | [`crates/pcloud-rsync/src/rolling.rs:84`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L84) | Compute the rolling hash over `window` from scratch. O(window.len()) — use this when initialising a new windo… |
| `hash` | `pub` | fn | [`crates/pcloud-rsync/src/rolling.rs:109`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L109) | 32-bit hash value. |
| `roll` | `pub` | fn | [`crates/pcloud-rsync/src/rolling.rs:130`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L130) | Roll the window: drop `out` (the byte that just left the window's left edge) and append `inb` (the byte that… |
| `push` | `pub` | fn | [`crates/pcloud-rsync/src/rolling.rs:155`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L155) | Append `byte` to the window (window length grows by 1). Used when initialising a partial window or when growi… |
| `window_len` | `pub` | fn | [`crates/pcloud-rsync/src/rolling.rs:182`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L182) | Window length the hash currently covers. |
| `tests` | `private` | mod | [`crates/pcloud-rsync/src/rolling.rs:188`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L188) | Read the source/rustdoc for the exact contract. |
| `empty_window_has_zero_state` | `private` | fn | [`crates/pcloud-rsync/src/rolling.rs:192`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L192) | Read the source/rustdoc for the exact contract. |
| `compute_is_deterministic` | `private` | fn | [`crates/pcloud-rsync/src/rolling.rs:199`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L199) | Read the source/rustdoc for the exact contract. |
| `different_content_yields_different_hash` | `private` | fn | [`crates/pcloud-rsync/src/rolling.rs:207`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L207) | Read the source/rustdoc for the exact contract. |
| `all_zeros_does_not_collapse_to_zero` | `private` | fn | [`crates/pcloud-rsync/src/rolling.rs:214`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L214) | Read the source/rustdoc for the exact contract. |
| `rolling_matches_recompute_at_every_position` | `private` | fn | [`crates/pcloud-rsync/src/rolling.rs:224`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L224) | Roll-byte-by-byte must equal compute-from-scratch for every position in a sliding window. This is the load-be… |
| `push_grows_window_and_matches_compute` | `private` | fn | [`crates/pcloud-rsync/src/rolling.rs:243`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L243) | Read the source/rustdoc for the exact contract. |
| `hash_packs_a_and_b` | `private` | fn | [`crates/pcloud-rsync/src/rolling.rs:255`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/rolling.rs#L255) | Read the source/rustdoc for the exact contract. |
| `DEFAULT_BLOCK_SIZE` | `pub` | const | [`crates/pcloud-rsync/src/signature.rs:40`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L40) | Default block size in bytes. Picked as a tradeoff: large enough that the strong-hash cost is amortised; small… |
| `STRONG_HASH_LEN` | `pub` | const | [`crates/pcloud-rsync/src/signature.rs:45`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L45) | Length of the truncated strong hash (bytes). 16 bytes ≈ 128 bits of collision resistance, which is comfortabl… |
| `BlockSignature` | `pub` | struct | [`crates/pcloud-rsync/src/signature.rs:49`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L49) | One block's signature entry. |
| `Signature` | `pub` | struct | [`crates/pcloud-rsync/src/signature.rs:58`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L58) | Full signature of a baseline file: header + per-block entries. |
| `block_count` | `pub` | fn | [`crates/pcloud-rsync/src/signature.rs:73`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L73) | Number of blocks the signature describes. |
| `block_len` | `pub` | fn | [`crates/pcloud-rsync/src/signature.rs:82`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L82) | Length of the block at `index`. The last block may be shorter than `block_size`. Returns `0` when `index &gt;= b… |
| `SignatureError` | `pub` | enum | [`crates/pcloud-rsync/src/signature.rs:99`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L99) | Errors raised while building a signature. |
| `compute_signature` | `pub` | fn | [`crates/pcloud-rsync/src/signature.rs:120`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L120) | Build a \[`Signature`\] over `data` with the given block size. # Errors See \[`SignatureError`\]. # Example ``` u… |
| `strong_hash_bytes` | `private` | fn | [`crates/pcloud-rsync/src/signature.rs:146`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L146) | SHA-256 the input and return the high 16 bytes. |
| `tests` | `private` | mod | [`crates/pcloud-rsync/src/signature.rs:154`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L154) | Read the source/rustdoc for the exact contract. |
| `empty_input_yields_empty_signature` | `private` | fn | [`crates/pcloud-rsync/src/signature.rs:158`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L158) | Read the source/rustdoc for the exact contract. |
| `block_size_zero_rejected` | `private` | fn | [`crates/pcloud-rsync/src/signature.rs:165`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L165) | Read the source/rustdoc for the exact contract. |
| `full_block_aligned_input_count` | `private` | fn | [`crates/pcloud-rsync/src/signature.rs:171`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L171) | Read the source/rustdoc for the exact contract. |
| `tail_block_short_length_reported` | `private` | fn | [`crates/pcloud-rsync/src/signature.rs:182`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L182) | Read the source/rustdoc for the exact contract. |
| `block_len_out_of_range_is_zero` | `private` | fn | [`crates/pcloud-rsync/src/signature.rs:191`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L191) | Read the source/rustdoc for the exact contract. |
| `identical_inputs_produce_identical_signatures` | `private` | fn | [`crates/pcloud-rsync/src/signature.rs:199`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L199) | Read the source/rustdoc for the exact contract. |
| `one_byte_change_changes_only_one_block_strong_hash` | `private` | fn | [`crates/pcloud-rsync/src/signature.rs:206`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L206) | Read the source/rustdoc for the exact contract. |
| `weak_hash_matches_rolling_hash_compute` | `private` | fn | [`crates/pcloud-rsync/src/signature.rs:226`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L226) | Read the source/rustdoc for the exact contract. |
| `strong_hash_truncates_sha256` | `private` | fn | [`crates/pcloud-rsync/src/signature.rs:236`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L236) | Read the source/rustdoc for the exact contract. |
| `serde_roundtrip` | `private` | fn | [`crates/pcloud-rsync/src/signature.rs:244`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-rsync/src/signature.rs#L244) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.
