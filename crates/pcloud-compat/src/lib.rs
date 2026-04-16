#![warn(unsafe_op_in_unsafe_fn)]
// Compat crate requires targeted unsafe for repr(C) binary layout
// serialisation matching the legacy C IPC wire format.
//! `pcloud-compat` — C-to-Rust IPC compatibility primitives.
//!
//! # Why this crate exists (R8 interop finding)
//!
//! The `pcloud-rs` C CLI and the Rust daemon must coexist during the Phase-1
//! dual-boot window. Finding **R8** in the Rust-rewrite audit
//! (`RUST-PLANS/` and `CLAUDE.md`) recorded that:
//!
//! 1. operators upgrading from the legacy client still rely on the SysV
//!    shared-memory status pipe and the 512-byte binary-RPC framing used by
//!    `control_tools.cpp` — any silent divergence from that wire format
//!    breaks `pcloud-rs --status` and the `crypto/`sync` opcodes the overlay
//!    tool issues over the `poverlay` socket,
//! 2. the SysV IPC key is derived via `ftok("$HOME/.pcloud/data.db", 'A')`
//!    at the exact path the legacy C client uses for its `SQLite` state.
//!    If a Rust daemon used a different anchor path (or a different project
//!    id), a stale C binary and a new Rust binary would compute **different
//!    keys** and silently fail to see each other's segment. Worse, dual-boot
//!    detection (the logic that refuses to start a second client when one is
//!    already running) would stop firing — two processes would each believe
//!    they were alone and scribble on the same `data.db` from opposite
//!    sides.
//!
//! Accordingly this crate exists purely as a **compat shim** that preserves
//! byte-for-byte compatibility with:
//!
//! * `pclsync/prpc.h` and `pclsync/pcommands.h` — the 16-byte rpc header and
//!   opcode enum,
//! * `pclsync/pshm.h` — the 4 KiB SysV segment header layout,
//! * `pclsync/pfoldersync.h` — the `psync_folder_list_t` payload format,
//! * `pclsync/ppath.c` — the `$HOME/.pcloud/data.db` anchor path used by
//!   `ftok()` so dual-boot detection keeps working without surprising users
//!   who run both binaries in sequence.
//!
//! The goal is that a Rust-based producer can publish to a C-based consumer
//! (and vice versa) for as long as the legacy binary is shipped. Once the C
//! client is retired this crate can be deleted wholesale; nothing in the
//! secure Rust path depends on it.
//!
//! Nothing in this crate is load-bearing for the secure native-Rust IPC
//! path. The native path uses owner-only Unix domain sockets with a
//! structured envelope (see `pcloud-daemon`). This crate is only compiled
//! into the daemon when the C-interop bridge is explicitly enabled.
//!
//! # Scope
//!
//! * [`rpc_codec`] — binary `rpc_message_t` framing with opcode enum
//!   (portable; always compiled).
//! * [`folder_list`] — ABI-exact mirror of `psync_folder_list_t`
//!   (portable; always compiled).
//! * `shm_producer` — SysV shared-memory segment producer matching the
//!   exact `psync_shm` layout. Only available on Linux and FreeBSD, and
//!   only when the `legacy-shm` Cargo feature is enabled.
//!
//! ## Why `#[cfg(any(target_os = "linux", target_os = "freebsd"))]`?
//!
//! The legacy C client was only ever shipped for Linux and FreeBSD — those
//! are the only platforms where SysV IPC is both reliably available and
//! actually consumed by an existing pcloud-rs installation:
//!
//! * **Linux** ships SysV IPC via `sys/shm.h` on every mainstream distro,
//!   and the C client's build system targets it as the primary platform.
//! * **FreeBSD** ships the same `sys/shm.h` interface with compatible
//!   `shmget`/`shmat`/`shmctl` semantics (`ftok` project-id, `IPC_RMID`
//!   on-last-detach).
//! * **macOS** has SysV shm but with hard kernel caps
//!   (`kern.sysv.shmmax=4194304` by default) and practical flakiness; the
//!   legacy C client was never officially built for macOS.
//! * **Windows / WASM / other** have no SysV IPC at all.
//!
//! Compiling `shm_producer` on those unsupported targets would either fail
//! to link (`libc::shmget` absent) or produce a module that can never be
//! exercised, so we gate it away entirely. The `folder_list` and
//! `rpc_codec` modules are pure serialization and stay portable.
//!
//! **Not implemented here** (separate sub-tasks): the opcode dispatcher,
//! pause/resume aggregation, and the daemon bridge itself.
//!
//! # Security notes
//!
//! The C codebase uses a world-writable (`0666`) SysV shm segment. This is
//! a legacy-compatibility quirk. In this crate the shm producer is gated
//! behind the `legacy-shm` Cargo feature and must be opt-in; no secrets
//! should ever be written to the shm surface — only status strings,
//! pending counters, and sync-folder descriptors (the same information the
//! C client surfaces). `ShmSegment::create` additionally
//! refuses to attach to any segment not owned by the current UID, closing
//! a drive-by-hijack window that the C client does not enforce.

#![deny(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]
#![allow(clippy::pedantic)]

// **PLATFORM:** folder_list + rpc_codec are portable (all targets).
// **GATING:** shm_producer is `#[cfg(any(target_os = "linux", target_os =
// "freebsd"))]` and also guarded by the `legacy-shm` Cargo feature.

pub mod folder_list;
pub mod rpc_codec;

#[cfg(all(
    any(target_os = "linux", target_os = "freebsd"),
    feature = "legacy-shm"
))]
pub mod shm_producer;
