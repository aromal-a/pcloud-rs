//! Async transfer protocol support: typed envelopes and helpers for
//! upload/download that run outside the synchronous command channel.
//! Consumed by `pcloud-backends::transfer_backend` and the
//! long-running transfer runtime.
//!
//! ## Role in the request pipeline
//!
//! The synchronous binary channel ([`crate::transport`]) is used for
//! short control-plane commands. Large / long-running transfers run
//! on dedicated sockets and are demultiplexed by a `stream_id`, so
//! multiple uploads or downloads can share an authenticated session
//! without head-of-line blocking. This module provides the typed
//! envelope used to represent a single frame on that multiplexed
//! channel; higher layers (`transfer_api`, the transfer runtime)
//! assemble frames into complete transfers.
//!
//! ## Security considerations
//!
//! Frames are otherwise-untrusted input. Consumers must validate
//! `payload_len` against a connection-level cap before allocating a
//! receive buffer, and must refuse to allow one `stream_id` to
//! impersonate another by checking stream ownership.
//!
//! Portable; no platform gating.

/// Single frame on a multiplexed pCloud async-transfer stream.
///
/// ## Wire layout
///
/// Each frame carries a `u32` `stream_id` followed by a
/// length-prefixed payload (`payload_len` bytes). The daemon
/// aggregates frames sharing a `stream_id` into a single logical
/// transfer; see [`crate::transfer_api`] for the higher-level
/// helpers that drive upload / download state machines.
///
/// ## Design choices
///
/// Owned, `Copy`-sized scalar fields (no `String` / `Vec`) so that
/// frame metadata can be cheaply logged and passed by value between
/// the transport reader thread and the transfer runtime without
/// heap traffic. The payload bytes themselves are handled separately
/// by the reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamFrame {
    /// Multiplexing identifier.
    ///
    /// Assigned by the transfer runtime when a new transfer is
    /// initiated and tagged into every frame so the receiver can
    /// route payload bytes to the correct in-flight transfer.
    pub stream_id: u32,
    /// Length, in bytes, of the payload attached to this frame.
    ///
    /// Callers must check this against their per-connection limit
    /// before allocating a buffer.
    pub payload_len: usize,
}
