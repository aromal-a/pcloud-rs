// Wave 1 / Primitive E — pclsync-compatible 128-ary Merkle authentication
// tree over per-sector 32-byte tags.
//
// GATING: feature = "pclsync-v2"
//
// PROVENANCE (C reference, upstream pcloud-rs tree):
//   - `pclsync/pfscrypto.h:41` defines
//     `PSYNC_CRYPTO_HASH_TREE_SECTORS = PSYNC_CRYPTO_SECTOR_SIZE / PSYNC_CRYPTO_AUTH_SIZE`
//     i.e. `4096 / 32 = 128` — the tree fanout.
//   - `pclsync/pcrypto.h:37` defines `PSYNC_CRYPTO_AUTH_SIZE = PSYNC_AES256_BLOCK_SIZE * 2 = 32`
//     (two AES blocks — the 32-byte tag shape).
//   - `pclsync/pcrypto.h:39` defines `PSYNC_CRYPTO_MAX_HASH_TREE_LEVEL = 6`
//     — the maximum depth (128^6 ≈ 4.4 * 10^12 leaves).
//   - `pclsync/pfscrypto.c:132-167` (`pfs_crpt_offset_by_size`) computes the
//     serialized layout: bottom-up, 128-child chunking, tracks the last
//     (possibly short) auth sector's length per level in
//     `offsets->lastauthsectorlen[level]`, and stores the root offset in
//     `offsets->masterauthoff`. Loop condition is `while (size > 1)` so the
//     loop stops once a single auth sector covers the level, and that final
//     one-sector level is the *root* stored at `masterauthoff`.
//   - `pclsync/pfscrypto.c:139` sets
//     `offsets->needmasterauth = size > PSYNC_CRYPTO_SECTOR_SIZE`, i.e. plain
//     size must exceed 4096 bytes (≥1 leaf) to require a master auth. Files of
//     size 0 are short-circuited (line 140-141, `if (!size) return`) and files
//     of size ≤4096 carry just the single leaf tag out-of-band.
//   - `pclsync/pcrypto.c:644-654` (`pcrypto_sign_sec`) is the C sign routine
//     used to build parent tags in `pfs_crpt_flush` (`pfscrypto.c:695`). It
//     computes:
//         tmp = HMAC-SHA512(hmac_key, data)[0..32]
//         tag = AES-256-ECB-encrypt-2-consecutive-blocks(aes_key, tmp)
//     where `enc->iv` is the 128-byte `sym_key_ver1::hmackey`
//     (`pcryptofolder.c:85-90`, `PSYNC_CRYPTO_HMAC_SHA512_KEY_LEN = 128` at
//     `psettings.h:170`) and `enc->encoder` is an AES-256 encoder keyed on
//     `sym_key_ver1::aeskey`.
//
// DIVERGENCE NOTE:
//   The deliverable signature for this module (per
//   `CLAUDE.md` "Wave 1 / Primitive E" brief) only threads `hmac_key: &[u8; 128]`
//   through parent-tag construction. The full byte-exact C tag construction
//   also requires the AES-256 encoder (`sym_key_ver1::aeskey`). This module
//   therefore implements the *HMAC-SHA512(hmac_key, concat_of_children)[0..32]*
//   half of the parent construction — the pure-HMAC half of the C routine —
//   and documents the missing AES step. A future integration patch
//   (`pclsync_sector::SectorEncoder::sign_sec` — Primitive D) will wrap this
//   module and apply the AES step to produce byte-exact C tags. Consumers
//   building on this module MUST NOT assume the returned tags are byte-for-byte
//   compatible with on-disk pclsync auth sectors until that wrapper lands.
//
//   The serialized layout produced here (`AuthTree::levels`) is a
//   level-separated representation convenient for test vectors and parent-path
//   recomputation. The C on-disk layout interleaves level-0 auth sectors after
//   every 128 data sectors, level-1 auth sectors after every 128 level-0
//   sectors, etc. (`pfscrypto.c:122-130` `pfs_crypto_auth_offset`). That
//   interleaving is a storage concern and does not affect tag contents; it
//   will be recomputed in the integration layer, not this primitive.

#![forbid(unsafe_code)]

use aes::Aes256;
use hmac::{Hmac, Mac};
use sha2::Sha512;
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

/// Branching factor of the auth tree: `PSYNC_CRYPTO_SECTOR_SIZE / PSYNC_CRYPTO_AUTH_SIZE`
/// (`pclsync/pfscrypto.h:41`).
pub const PCLSYNC_TREE_FANOUT: usize = 128;

/// Size (in bytes) of one fully-populated auth sector: `128 * 32 = 4096`
/// (matches `PSYNC_CRYPTO_SECTOR_SIZE` at `pclsync/pcrypto.h:46`).
pub const PCLSYNC_AUTH_SECTOR_SIZE: usize = PCLSYNC_TREE_FANOUT * PCLSYNC_AUTH_TAG_LEN;

/// Maximum tree depth (`pclsync/pcrypto.h:39`: `PSYNC_CRYPTO_MAX_HASH_TREE_LEVEL = 6`).
/// `128^6` ≈ 4.4 trillion leaves — enough for a 17-petabyte plaintext.
pub const PCLSYNC_MAX_TREE_LEVELS: usize = 6;

/// Size (in bytes) of one auth tag: `PSYNC_CRYPTO_AUTH_SIZE = AES256_BLOCK * 2`
/// (`pclsync/pcrypto.h:37`). 32 bytes.
pub const PCLSYNC_AUTH_TAG_LEN: usize = 32;

/// Length of the HMAC-SHA512 key (`PSYNC_CRYPTO_HMAC_SHA512_KEY_LEN = 128`,
/// `pclsync/psettings.h:170`). This is the `hmackey` field of `sym_key_ver1`
/// (`pcryptofolder.c:85-90`).
pub const PCLSYNC_HMAC_KEY_LEN: usize = 128;

type HmacSha512 = Hmac<Sha512>;

/// Serialized Merkle authentication tree.
///
/// `levels[0]` holds the leaf auth sectors (concatenated sector tags, packed
/// 32 bytes each). `levels[1]` holds the parent sectors, and so on. For a
/// non-empty tree, the top entry `levels[treelevels]` is exactly one 32-byte
/// tag: the master/root tag, whose serialized byte offset in the C on-disk
/// layout is `masterauthoff`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthTree {
    /// Level-separated serialization. `levels.len() == treelevels + 1` for a
    /// non-empty tree; `levels.len() == 0` for the empty case.
    pub levels: Vec<Vec<u8>>,
    /// Number of interior levels (0-based depth). Matches the C
    /// `offsets->treelevels` value at `pfscrypto.c:166`.
    pub treelevels: usize,
    /// Byte offset (in the C serialized on-disk layout) where the root auth
    /// tag is written. Computed per `pfs_crpt_offset_by_size` at
    /// `pfscrypto.c:163-165`: the sum of all encrypted data bytes (rounded
    /// up to a full last sector) plus all non-root auth-sector bytes.
    pub masterauthoff: u64,
    /// Mirrors the C `offsets->needmasterauth` flag (`pfscrypto.c:139`).
    /// `true` iff the plaintext is strictly greater than one sector
    /// (`> 4096 bytes`) — i.e. more than a single leaf tag is required.
    pub needmasterauth: bool,
}

/// Build an authentication tree over the caller-supplied leaf tags.
///
/// `sector_tags` is the ordered sequence of per-sector 32-byte tags produced
/// by the Primitive D sector AEAD (`pcrypto_encode_sec`). `hmac_key` is the
/// 128-byte `sym_key_ver1::hmackey`.
///
/// Parent tags are derived as:
///
/// ```text
///     parent = HMAC-SHA512(hmac_key, concat_of_128_or_fewer_child_tags)[0..32]
/// ```
///
/// This implements the pure-HMAC half of `pcrypto_sign_sec`
/// (`pclsync/pcrypto.c:644-654`). See the module-level DIVERGENCE NOTE for the
/// missing AES-256-ECB wrapping step.
///
/// # Behaviour on edge cases
/// - `sector_tags.is_empty()` → empty tree, `needmasterauth = false`,
///   `treelevels = 0`, `masterauthoff = 0`. Matches
///   `pfscrypto.c:140-141` (`if (!size) return`).
/// - `sector_tags.len() == 1` → single leaf, `needmasterauth = false`,
///   `treelevels = 0`. The one 32-byte leaf tag is held in `levels[0]`
///   and `compute_master_auth` returns it. This mirrors the C
///   short-circuit where a ≤4096-byte file stores just the single leaf tag
///   out-of-band (`offsets->needmasterauth = size > PSYNC_CRYPTO_SECTOR_SIZE`
///   — false for a one-sector file).
///
/// # Panics
/// Does not panic. Input lengths exceeding `128^6` would exceed
/// `PCLSYNC_MAX_TREE_LEVELS`; the builder caps level growth and will leave
/// `treelevels == PCLSYNC_MAX_TREE_LEVELS` for any such oversize input.
#[must_use]
pub fn build_auth_tree(
    hmac_key: &[u8; PCLSYNC_HMAC_KEY_LEN],
    sector_tags: &[[u8; PCLSYNC_AUTH_TAG_LEN]],
) -> AuthTree {
    if sector_tags.is_empty() {
        return AuthTree {
            levels: Vec::new(),
            treelevels: 0,
            masterauthoff: 0,
            needmasterauth: false,
        };
    }

    // Pack level 0 (the raw leaf-tag concatenation, possibly spanning many
    // 4096-byte auth sectors). Matches C `lastauthsectorlen[0] = nleaves % 128
    // * 32` for the last level-0 sector (pfscrypto.c:151-162).
    let mut level0 = Vec::with_capacity(sector_tags.len() * PCLSYNC_AUTH_TAG_LEN);
    for tag in sector_tags {
        level0.extend_from_slice(tag);
    }
    let mut levels: Vec<Vec<u8>> = Vec::new();
    levels.push(level0);

    // Grow multi-tag auth-sector levels until the top level holds a single
    // auth sector (<= 128 tags). This matches the C loop condition
    // `while (size > 1)` at pfscrypto.c:151-162 where `size` is the number of
    // auth sectors at the current level.
    // SAFETY: `levels` is seeded with `level0` (pushed above) and only grows
    // via `levels.push(parent)`; `levels.last()` is therefore always `Some`.
    while level_sector_count(levels.last().expect("non-empty").len()) > 1 {
        if levels.len() >= PCLSYNC_MAX_TREE_LEVELS {
            // Safety cap: C asserts `level <= PSYNC_CRYPTO_MAX_HASH_TREE_LEVEL`
            // (`pfscrypto.c:185`). We refuse to keep building rather than
            // panic. The resulting `AuthTree` is still well-formed but its
            // `compute_master_auth` no longer covers the entire leaf set.
            break;
        }
        // SAFETY: same invariant — `levels` is non-empty at every iteration.
        let parent = build_parent_level(hmac_key, levels.last().expect("non-empty"));
        levels.push(parent);
    }

    // Produce the single-tag root by HMAC-compressing the top auth sector, as
    // long as that top auth sector holds more than one tag (i.e. multi-leaf
    // input). Matches C pfscrypto.c:163-166 which stores an extra 32-byte
    // `lastauthsectorlen[level] = PSYNC_CRYPTO_AUTH_SIZE` at `masterauthoff`
    // on top of the multi-tag auth sectors. For the single-leaf case the top
    // auth sector is already 32 bytes and the "root" is that same tag
    // (`needmasterauth = false`, pfscrypto.c:139).
    let needmasterauth = sector_tags.len() > 1;
    if needmasterauth {
        // SAFETY: `levels` was seeded above and is non-empty throughout.
        let top = levels.last().expect("non-empty");
        let root_tag = hmac_sha512_trunc32(hmac_key, top);
        levels.push(root_tag.to_vec());
    }

    // `treelevels` in C is the number of multi-tag auth-sector levels (not
    // counting the single-tag root). For an empty / single-leaf file it is 0.
    // After building, `levels` = [auth-sector levels..., root] so the count is
    // `levels.len() - 1` when the root is present.
    let treelevels = if needmasterauth { levels.len() - 1 } else { 0 };

    // masterauthoff = sum of encrypted plaintext bytes (here: 4096 per leaf
    // sector, no short-sector padding done at this primitive layer) + every
    // non-root level's serialized byte length. Matches the accumulation at
    // `pfscrypto.c:158-165`.
    let data_bytes: u64 = (sector_tags.len() as u64) * (PCLSYNC_AUTH_SECTOR_SIZE as u64);
    let auth_bytes: u64 = levels
        .iter()
        .take(levels.len().saturating_sub(1))
        .map(|l| l.len() as u64)
        .sum();
    let masterauthoff = data_bytes + auth_bytes;

    AuthTree {
        levels,
        treelevels,
        masterauthoff,
        needmasterauth,
    }
}

/// Number of 4096-byte auth sectors occupied by a level of size `bytes`.
/// The final auth sector at a level may be short (see
/// `offsets->lastauthsectorlen` in `pfscrypto.c:159`).
fn level_sector_count(bytes: usize) -> usize {
    bytes.div_ceil(PCLSYNC_AUTH_SECTOR_SIZE)
}

/// Build one parent level by HMAC-chunking the child level 128 tags at a time.
fn build_parent_level(hmac_key: &[u8; PCLSYNC_HMAC_KEY_LEN], child_level: &[u8]) -> Vec<u8> {
    let mut parent =
        Vec::with_capacity(level_sector_count(child_level.len()) * PCLSYNC_AUTH_TAG_LEN);
    for chunk in child_level.chunks(PCLSYNC_AUTH_SECTOR_SIZE) {
        let tag = hmac_sha512_trunc32(hmac_key, chunk);
        parent.extend_from_slice(&tag);
    }
    parent
}

/// HMAC-SHA512(key, data) truncated to the first 32 bytes.
/// Mirrors the first step of `pcrypto_sign_sec` (`pcrypto.c:651`).
fn hmac_sha512_trunc32(
    key: &[u8; PCLSYNC_HMAC_KEY_LEN],
    data: &[u8],
) -> [u8; PCLSYNC_AUTH_TAG_LEN] {
    // SAFETY: HMAC-SHA-512 accepts any non-zero key length (RFC 2104);
    // callers pass a fixed 64-byte `[u8; PCLSYNC_HMAC_KEY_LEN]` here.
    let mut mac = HmacSha512::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    let full = mac.finalize().into_bytes();
    let mut out = [0u8; PCLSYNC_AUTH_TAG_LEN];
    out.copy_from_slice(&full[..PCLSYNC_AUTH_TAG_LEN]);
    // `full` is a GenericArray<u8, _>, which is Copy — so there is no
    // meaningful drop side-effect; the bytes live on the stack and are
    // scrubbed only by the Zeroize-on-drop helpers used elsewhere.
    // Nothing more to do here; explicit drop was a no-op (clippy caught).
    let _ = full;
    out
}

/// Apply the AES-256-ECB step on a parent tag in place. Mirrors the
/// second half of `pcrypto_sign_sec` (`pclsync/pcrypto.c:644-654`):
///
/// ```text
///     tag = AES-256-ECB-encrypt-2-consecutive-blocks(aes_key, hmac_tag)
/// ```
///
/// `cipher` must be keyed on `sym_key_ver1::aes_key` (32 bytes, see
/// `pclsync_rsa::PCLSYNC_AES_KEY_LEN`). The 32-byte input is encrypted
/// as two consecutive 16-byte AES-256-ECB blocks, matching
/// `psync_aes256_encode_2blocks_consec` in the C client.
///
/// CLAUDEREV iter-1 CRYPTO-H-3 / fire 17 (2026-04-30): closes the
/// long-standing DIVERGENCE NOTE at the head of this module. The
/// previous Wave-1 deliverable produced HMAC-only parent tags; this
/// helper completes the C tag shape.
fn aes_ecb_two_blocks_inplace(cipher: &Aes256, tag: &mut [u8; PCLSYNC_AUTH_TAG_LEN]) {
    crate::pclsync_sector::ecb_encrypt_two_blocks(cipher, tag);
}

/// Length of the `sym_key_ver1::aes_key` that drives the AES-256 step.
pub const PCLSYNC_AES_KEY_LEN: usize = 32;

/// Build the auth tree the **byte-exact C-compatible** way: each
/// parent tag is `AES-256-ECB-2-blocks(aes_key, HMAC-SHA512(hmac_key,
/// concat_of_children)[0..32])`. Closes CLAUDEREV iter-1 CRYPTO-H-3
/// (fire 17, 2026-04-30) — the AES-256-ECB step that the original
/// Wave-1 deliverable left for a future "Primitive D wrapper" is now
/// applied here.
///
/// `aes_key` is `sym_key_ver1::aes_key` (32 bytes); `hmac_key` is
/// `sym_key_ver1::hmackey` (128 bytes). The leaf tags in
/// `sector_tags` are NOT re-AES'd — leaves are produced by the
/// `pcrypto_encode_sec` AEAD which already incorporates the AES key.
/// Only **parent and root** tags are HMAC + AES'd here, matching the
/// C call site at `pfs_crpt_flush` (`pfscrypto.c:695`).
///
/// Same edge-case behaviour as [`build_auth_tree`]:
/// - empty input → empty tree, `needmasterauth = false`,
///   `treelevels = 0`.
/// - single leaf → leaf tag stored verbatim, `needmasterauth = false`,
///   `treelevels = 0`. (No parent — no AES step applied.)
///
/// # Cross-client interop posture
///
/// This function produces the byte-exact C wire shape *if* the
/// `aes_key` and `hmac_key` arguments match those used by the C
/// client to encode the same plaintext. Capturing a real C-client
/// fixture and asserting byte-identity is tracked separately as a
/// follow-up live-KAT (the closure criterion stated in the
/// CLAUDEREV remediation plan); the in-tree round-trip test
/// `aes_step_changes_root` regression-guards that the AES step is
/// actually applied versus the HMAC-only baseline.
#[must_use]
pub fn build_auth_tree_with_aes(
    aes_key: &[u8; PCLSYNC_AES_KEY_LEN],
    hmac_key: &[u8; PCLSYNC_HMAC_KEY_LEN],
    sector_tags: &[[u8; PCLSYNC_AUTH_TAG_LEN]],
) -> AuthTree {
    if sector_tags.is_empty() {
        return AuthTree {
            levels: Vec::new(),
            treelevels: 0,
            masterauthoff: 0,
            needmasterauth: false,
        };
    }

    // SAFETY: AES-256 accepts exactly 32-byte keys; we statically
    // require `[u8; PCLSYNC_AES_KEY_LEN]` (= 32) so this never fails.
    // Use the fully-qualified call to avoid the `new_from_slice`
    // ambiguity with `hmac::Mac::new_from_slice` already in scope.
    let cipher = <Aes256 as aes::cipher::KeyInit>::new_from_slice(aes_key)
        .expect("AES-256 accepts a 32-byte key");

    // Pack level 0 verbatim — leaves are not AES'd here (they came
    // pre-encoded from `pcrypto_encode_sec`).
    let mut level0 = Vec::with_capacity(sector_tags.len() * PCLSYNC_AUTH_TAG_LEN);
    for tag in sector_tags {
        level0.extend_from_slice(tag);
    }
    let mut levels: Vec<Vec<u8>> = Vec::new();
    levels.push(level0);

    // Build parent levels with the full HMAC + AES tag shape until the
    // top level holds a single auth sector.
    while level_sector_count(levels.last().expect("non-empty").len()) > 1 {
        if levels.len() >= PCLSYNC_MAX_TREE_LEVELS {
            break;
        }
        let parent =
            build_parent_level_with_aes(&cipher, hmac_key, levels.last().expect("non-empty"));
        levels.push(parent);
    }

    let needmasterauth = sector_tags.len() > 1;
    if needmasterauth {
        let top = levels.last().expect("non-empty");
        let mut root_tag = hmac_sha512_trunc32(hmac_key, top);
        aes_ecb_two_blocks_inplace(&cipher, &mut root_tag);
        levels.push(root_tag.to_vec());
    }

    let treelevels = if needmasterauth { levels.len() - 1 } else { 0 };
    let data_bytes: u64 = (sector_tags.len() as u64) * (PCLSYNC_AUTH_SECTOR_SIZE as u64);
    let auth_bytes: u64 = levels
        .iter()
        .take(levels.len().saturating_sub(1))
        .map(|l| l.len() as u64)
        .sum();
    let masterauthoff = data_bytes + auth_bytes;

    AuthTree {
        levels,
        treelevels,
        masterauthoff,
        needmasterauth,
    }
}

/// Build one parent level by HMAC + AES-ECB-chunking the child level
/// 128 tags at a time. Companion to [`build_parent_level`] for the
/// C-byte-exact path.
fn build_parent_level_with_aes(
    cipher: &Aes256,
    hmac_key: &[u8; PCLSYNC_HMAC_KEY_LEN],
    child_level: &[u8],
) -> Vec<u8> {
    let mut parent =
        Vec::with_capacity(level_sector_count(child_level.len()) * PCLSYNC_AUTH_TAG_LEN);
    for chunk in child_level.chunks(PCLSYNC_AUTH_SECTOR_SIZE) {
        let mut tag = hmac_sha512_trunc32(hmac_key, chunk);
        aes_ecb_two_blocks_inplace(cipher, &mut tag);
        parent.extend_from_slice(&tag);
    }
    parent
}

/// Verify that `sector_tag` at `sector_index` (0-based, leaf ordering) is
/// consistent with the supplied tree by recomputing the parent chain from
/// the claimed leaf up to the root and constant-time-comparing against
/// `compute_master_auth(tree)`.
#[must_use]
pub fn verify_path(
    hmac_key: &[u8; PCLSYNC_HMAC_KEY_LEN],
    tree: &AuthTree,
    sector_index: usize,
    sector_tag: &[u8; PCLSYNC_AUTH_TAG_LEN],
) -> bool {
    if tree.levels.is_empty() {
        return false;
    }

    // Confirm the claimed leaf matches the tree's stored leaf.
    let leaf_bytes = &tree.levels[0];
    let offset = sector_index.saturating_mul(PCLSYNC_AUTH_TAG_LEN);
    if offset + PCLSYNC_AUTH_TAG_LEN > leaf_bytes.len() {
        return false;
    }
    let stored_leaf = &leaf_bytes[offset..offset + PCLSYNC_AUTH_TAG_LEN];
    if stored_leaf.ct_eq(sector_tag).unwrap_u8() == 0 {
        return false;
    }

    // Single-leaf tree: root == leaf, nothing to recompute.
    if tree.treelevels == 0 {
        let root = compute_master_auth(tree);
        return root.ct_eq(sector_tag).unwrap_u8() == 1;
    }

    // Walk up the levels. At each level, the "sibling chunk" is the
    // fanout-sized slab of child tags containing our current position.
    let mut current = *sector_tag;
    let mut child_index = sector_index;
    for level in 0..tree.treelevels {
        let child_level = &tree.levels[level];
        let chunk_start = (child_index / PCLSYNC_TREE_FANOUT) * PCLSYNC_AUTH_SECTOR_SIZE;
        // The last chunk may be short (`lastauthsectorlen`).
        let chunk_end = (chunk_start + PCLSYNC_AUTH_SECTOR_SIZE).min(child_level.len());
        if chunk_start >= child_level.len() {
            return false;
        }
        let chunk = &child_level[chunk_start..chunk_end];

        // Sanity-check that the stored chunk has our tag at the expected slot.
        let slot = (child_index % PCLSYNC_TREE_FANOUT) * PCLSYNC_AUTH_TAG_LEN;
        if slot + PCLSYNC_AUTH_TAG_LEN > chunk.len() {
            return false;
        }
        if chunk[slot..slot + PCLSYNC_AUTH_TAG_LEN]
            .ct_eq(&current)
            .unwrap_u8()
            == 0
        {
            return false;
        }

        current = hmac_sha512_trunc32(hmac_key, chunk);
        child_index /= PCLSYNC_TREE_FANOUT;
    }

    let root = compute_master_auth(tree);
    let ok = current.ct_eq(&root).unwrap_u8() == 1;
    // Best-effort zeroize of the running tag.
    let mut scratch = current;
    scratch.zeroize();
    ok
}

/// Return the root (master) auth tag. For a single-leaf tree this is the
/// leaf tag; for an empty tree this is all-zero (caller should gate on
/// `needmasterauth` / non-empty `levels` first).
#[must_use]
pub fn compute_master_auth(tree: &AuthTree) -> [u8; PCLSYNC_AUTH_TAG_LEN] {
    let mut out = [0u8; PCLSYNC_AUTH_TAG_LEN];
    if let Some(top) = tree.levels.last() {
        if top.len() >= PCLSYNC_AUTH_TAG_LEN {
            out.copy_from_slice(&top[..PCLSYNC_AUTH_TAG_LEN]);
        }
    }
    out
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; PCLSYNC_HMAC_KEY_LEN] {
        let mut k = [0u8; PCLSYNC_HMAC_KEY_LEN];
        for (i, b) in k.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7).wrapping_add(3);
        }
        k
    }

    fn tag(seed: u8) -> [u8; PCLSYNC_AUTH_TAG_LEN] {
        let mut t = [0u8; PCLSYNC_AUTH_TAG_LEN];
        for (i, b) in t.iter_mut().enumerate() {
            *b = seed.wrapping_add(i as u8);
        }
        t
    }

    fn tags(n: usize) -> Vec<[u8; PCLSYNC_AUTH_TAG_LEN]> {
        (0..n).map(|i| tag(i as u8)).collect()
    }

    fn aes_key() -> [u8; PCLSYNC_AES_KEY_LEN] {
        let mut k = [0u8; PCLSYNC_AES_KEY_LEN];
        for (i, b) in k.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(11).wrapping_add(5);
        }
        k
    }

    /// CLAUDEREV iter-1 CRYPTO-H-3 / fire 17 (2026-04-30) regression
    /// guard. The AES-256-ECB step on parent tags must actually be
    /// applied — i.e. `build_auth_tree_with_aes` must produce a
    /// different root than the HMAC-only `build_auth_tree` when the
    /// input has at least one parent level. A single-leaf input is
    /// excluded because no parent is ever computed.
    #[test]
    fn aes_step_changes_root() {
        // Two leaves → one parent level, plus root with AES applied.
        let leaves = tags(2);
        let hmac_only = build_auth_tree(&key(), &leaves);
        let with_aes = build_auth_tree_with_aes(&aes_key(), &key(), &leaves);
        let hmac_only_root = compute_master_auth(&hmac_only);
        let with_aes_root = compute_master_auth(&with_aes);
        assert_ne!(
            hmac_only_root, with_aes_root,
            "build_auth_tree_with_aes must apply AES; root tags differ from HMAC-only baseline"
        );
        // Both trees have the same shape (treelevels, needmasterauth)
        // — only the bytes differ.
        assert_eq!(hmac_only.needmasterauth, with_aes.needmasterauth);
        assert_eq!(hmac_only.treelevels, with_aes.treelevels);
    }

    /// Edge case: a single-leaf input never builds a parent level, so
    /// `build_auth_tree_with_aes` must produce the SAME root as the
    /// HMAC-only `build_auth_tree` (the leaf tag itself, untouched).
    #[test]
    fn aes_variant_single_leaf_matches_hmac_only() {
        let leaves = tags(1);
        let hmac_only = build_auth_tree(&key(), &leaves);
        let with_aes = build_auth_tree_with_aes(&aes_key(), &key(), &leaves);
        assert_eq!(
            compute_master_auth(&hmac_only),
            compute_master_auth(&with_aes),
            "single-leaf trees: no parent level → no AES step → identical roots"
        );
        assert!(!with_aes.needmasterauth);
        assert_eq!(with_aes.treelevels, 0);
    }

    /// Edge case: empty input → both variants return the empty tree.
    #[test]
    fn aes_variant_empty_input_matches_hmac_only() {
        let hmac_only = build_auth_tree(&key(), &[]);
        let with_aes = build_auth_tree_with_aes(&aes_key(), &key(), &[]);
        assert_eq!(hmac_only.needmasterauth, with_aes.needmasterauth);
        assert_eq!(hmac_only.treelevels, with_aes.treelevels);
        assert_eq!(hmac_only.masterauthoff, with_aes.masterauthoff);
        assert!(hmac_only.levels.is_empty());
        assert!(with_aes.levels.is_empty());
    }

    #[test]
    fn empty_file_needmasterauth_false() {
        let t = build_auth_tree(&key(), &[]);
        assert!(!t.needmasterauth);
        assert_eq!(t.treelevels, 0);
        assert_eq!(t.masterauthoff, 0);
        assert!(t.levels.is_empty());
    }

    #[test]
    fn single_sector_needmasterauth_false() {
        let leaves = tags(1);
        let t = build_auth_tree(&key(), &leaves);
        assert!(
            !t.needmasterauth,
            "single-sector file must not need master auth"
        );
        assert_eq!(t.treelevels, 0);
        assert_eq!(t.levels.len(), 1);
        assert_eq!(t.levels[0].len(), PCLSYNC_AUTH_TAG_LEN);
        // Root == leaf for single-sector input.
        assert_eq!(compute_master_auth(&t), leaves[0]);
        // masterauthoff = data_bytes (no non-root auth levels accumulated).
        assert_eq!(t.masterauthoff, PCLSYNC_AUTH_SECTOR_SIZE as u64);
    }

    #[test]
    fn two_sectors_one_level() {
        let leaves = tags(2);
        let t = build_auth_tree(&key(), &leaves);
        assert!(t.needmasterauth);
        assert_eq!(t.treelevels, 1);
        assert_eq!(t.levels.len(), 2);
        assert_eq!(t.levels[0].len(), 2 * PCLSYNC_AUTH_TAG_LEN);
        assert_eq!(t.levels[1].len(), PCLSYNC_AUTH_TAG_LEN);
        // Hand-recompute the root:
        let mut buf = Vec::new();
        buf.extend_from_slice(&leaves[0]);
        buf.extend_from_slice(&leaves[1]);
        let expected = hmac_sha512_trunc32(&key(), &buf);
        assert_eq!(compute_master_auth(&t), expected);
    }

    #[test]
    fn fanout_128_one_level() {
        // Exactly 128 leaves → still one auth-sector at level 0, root is level 1.
        let leaves = tags(PCLSYNC_TREE_FANOUT);
        let t = build_auth_tree(&key(), &leaves);
        assert!(t.needmasterauth);
        assert_eq!(t.treelevels, 1);
        assert_eq!(t.levels[0].len(), PCLSYNC_AUTH_SECTOR_SIZE);
        assert_eq!(t.levels[1].len(), PCLSYNC_AUTH_TAG_LEN);
    }

    #[test]
    fn fanout_129_two_levels() {
        // 129 leaves forces two auth sectors at level 0 → parent level is
        // required → tree depth = 2.
        let leaves = tags(PCLSYNC_TREE_FANOUT + 1);
        let t = build_auth_tree(&key(), &leaves);
        assert!(t.needmasterauth);
        assert_eq!(t.treelevels, 2);
        assert_eq!(t.levels.len(), 3);
        assert_eq!(t.levels[0].len(), 129 * PCLSYNC_AUTH_TAG_LEN);
        assert_eq!(t.levels[1].len(), 2 * PCLSYNC_AUTH_TAG_LEN);
        assert_eq!(t.levels[2].len(), PCLSYNC_AUTH_TAG_LEN);
    }

    #[test]
    fn full_128_squared_two_levels() {
        // 128 * 128 leaves → level 1 fills exactly one auth sector → root at level 2.
        let leaves = tags(PCLSYNC_TREE_FANOUT * PCLSYNC_TREE_FANOUT);
        let t = build_auth_tree(&key(), &leaves);
        assert!(t.needmasterauth);
        assert_eq!(t.treelevels, 2);
        assert_eq!(
            t.levels[0].len(),
            PCLSYNC_TREE_FANOUT * PCLSYNC_AUTH_SECTOR_SIZE
        );
        assert_eq!(t.levels[1].len(), PCLSYNC_AUTH_SECTOR_SIZE);
        assert_eq!(t.levels[2].len(), PCLSYNC_AUTH_TAG_LEN);
    }

    #[test]
    fn verify_path_happy() {
        let leaves = tags(300); // forces 2+ levels
        let t = build_auth_tree(&key(), &leaves);
        for (idx, leaf) in leaves.iter().enumerate() {
            assert!(
                verify_path(&key(), &t, idx, leaf),
                "leaf {idx} must verify against root",
            );
        }
    }

    #[test]
    fn verify_path_rejects_wrong_tag() {
        let leaves = tags(300);
        let t = build_auth_tree(&key(), &leaves);
        let mut bogus = leaves[42];
        bogus[0] ^= 0x01;
        assert!(!verify_path(&key(), &t, 42, &bogus));
        // Correct tag at wrong index is also rejected.
        assert!(!verify_path(&key(), &t, 43, &leaves[42]));
        // Out-of-range index is rejected.
        assert!(!verify_path(&key(), &t, 9999, &leaves[0]));
    }

    #[test]
    fn masterauthoff_matches_expected_for_known_file_size() {
        // 10 000 sectors: level 0 has 10 000 tags (10 000*32 = 320 000 bytes,
        // requires 79 auth sectors because 10 000/128 = 78.125 -> 79 sectors).
        // level 1 has 79 tags (79*32 = 2 528 bytes, requires 1 auth sector).
        // level 2 (root) has 1 tag (32 bytes).
        //
        // data_bytes = 10_000 * 4096 = 40_960_000
        // non_root_auth_bytes = level0 + level1 = 320_000 + 2_528 = 322_528
        // masterauthoff = 40_960_000 + 322_528 = 41_282_528
        let leaves = tags(10_000);
        let t = build_auth_tree(&key(), &leaves);
        assert_eq!(t.treelevels, 2);
        assert_eq!(t.levels[0].len(), 10_000 * PCLSYNC_AUTH_TAG_LEN);
        assert_eq!(t.levels[1].len(), 79 * PCLSYNC_AUTH_TAG_LEN);
        assert_eq!(t.levels[2].len(), PCLSYNC_AUTH_TAG_LEN);
        assert_eq!(t.masterauthoff, 41_282_528);
    }

    #[test]
    fn build_idempotent() {
        let leaves = tags(257);
        let a = build_auth_tree(&key(), &leaves);
        let b = build_auth_tree(&key(), &leaves);
        assert_eq!(a, b);
    }

    #[test]
    fn treelevels_constants_match_c() {
        // pclsync/pfscrypto.h:41 and pcrypto.h:37 / :39.
        assert_eq!(PCLSYNC_TREE_FANOUT, 128);
        assert_eq!(PCLSYNC_AUTH_TAG_LEN, 32);
        assert_eq!(PCLSYNC_AUTH_SECTOR_SIZE, 4096);
        assert_eq!(PCLSYNC_MAX_TREE_LEVELS, 6);
        assert_eq!(PCLSYNC_HMAC_KEY_LEN, 128);
    }
}
