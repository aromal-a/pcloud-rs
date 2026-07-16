#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(clippy::pedantic)]
//! T2.1 — block signatures + rolling-hash primitives for
//! differential sync.
//!
//! # Why a separate crate
//!
//! The differential-sync pipeline has three concerns:
//!
//! 1. **Signature**: hash every block of the (remote) baseline file.
//! 2. **Delta**: walk the (local) candidate file with a rolling
//!    hash, look up each window in the signature, emit `CopyServer`
//!    or `NewBytes` operations.
//! 3. **Apply**: server-side, materialise the new file from the
//!    `CopyServer` ranges + the `NewBytes` payload.
//!
//! All three pieces are pure compute: no I/O, no async, no
//! pCloud-specific glue. Putting them in their own crate keeps
//! the `pcloud-engine` planner focused on scheduling and lets the
//! delta encoder be tested in isolation against synthetic edits.
//!
//! # Algorithm shape
//!
//! Mirrors librsync's choice of two hashes per block:
//!
//! - **Weak hash** — Adler-32-style 32-bit rolling sum. Cheap to
//!   advance one byte at a time, cheap to compare against a hash
//!   table; collisions are common.
//! - **Strong hash** — SHA-256 truncated to 16 bytes (the high
//!   bits — librsync uses MD4, but the workspace already pulls
//!   `sha2` and we trade a small extra cost for a far stronger
//!   second-layer match). Distinguishes weak-hash false positives.
//!
//! A 16-byte truncated SHA-256 has a collision probability of
//! ~1.5e-19 over a single comparison, which is many orders of
//! magnitude below the per-file unrecoverable error rate of
//! consumer SSDs. Truncation halves the signature's on-wire size.
//!
//! # Block size
//!
//! Default 4 KiB ([`DEFAULT_BLOCK_SIZE`]). librsync's heuristic is
//! `sqrt(file_size)`; for streaming uploads we prefer a fixed
//! size since it lets the server-side `upload_writefromfile` API
//! address blocks by `(offset, length)` without negotiating
//! variable-size segments first.

pub mod delta;
pub mod rolling;
pub mod signature;

pub use delta::{DeltaOp, apply_delta, compute_delta};
pub use rolling::RollingHash;
pub use signature::{
    BlockSignature, DEFAULT_BLOCK_SIZE, Signature, SignatureError, compute_signature,
};
