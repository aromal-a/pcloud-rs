# Section 7 — IPC & Daemon Audit (Opus)

**Scope:** `crates/pcloud-ipc/`, `crates/pcloud-daemon/` — framing, peer
credential checks, socket perms, rate limiting, dispatch safety,
privileged logging, sd_notify, connection limits, write timeouts,
shutdown, health endpoints, proptest coverage, plus new
`Request::UploadWriteFromFile` and `Request::CreateTreePublicLinkFromPaths`.

---

## CRITICAL

### C1. `UploadWriteFromFile` reads arbitrary local files with no path validation and no size cap
`crates/pcloud-daemon/src/runtime.rs:2493-2530`

`upload_write_from_file_ipc` accepts `local_path: String` from the IPC
peer and calls `std::fs::read(path)` directly. There is:

- No `pcloud_ipc::path_validation::validate_local_sync_path` check
  (NUL, `..`, symlink, UTF-8 length, `MAX_SYNC_PATH_LEN`).
- No size cap — the whole file is slurped into `Vec<u8>` in memory.
  The 1 MiB IPC frame cap (`MAX_REQUEST_BYTES`,
  `crates/pcloud-ipc/src/server.rs:42`) is bypassed, enabling an
  owner-uid process to OOM the daemon by pointing at a large file or
  `/dev/zero`-style device node.
- No symlink / special-file rejection — a symlink or FIFO could cause
  blocking reads or dereference into sensitive state.
- The buffer is then passed to `transfer_runtime.upload_bytes` without
  streaming; chunked `upload_write` pipelining (`TODO(bd-1du.4.6)` in
  write_path.rs) is not used.

Fix: validate `local_path` via `path_validation` (extended with
absolute-path and symlink-follow policy), `stat` then reject symlinks /
devices / FIFOs, enforce a configurable max file size, stream via
`std::fs::File` + chunked reads into sequential `upload_write` calls.

### C2. Rate-limit categorization does not cover the two new variants correctly
`crates/pcloud-daemon/src/rate_limit.rs:181-205`

- `Request::UploadWriteFromFile { .. }` falls through the `_ =>
  RateCategory::Medium` default. This is a heavy disk + network
  operation and should sit in `Expensive` alongside other bulk
  transfers.
- `Request::CreateTreePublicLinkFromPaths { .. }` also falls through to
  `Medium`, yet its sibling `Request::CreateTreePublicLink { .. }` is
  explicitly classified `Expensive` at line 194. The path-based variant
  performs N daemon-side `get_folder_id_by_path` round-trips *plus* the
  tree-link creation — strictly heavier than the id-based sibling. The
  asymmetry lets a caller bypass the Expensive bucket by choosing the
  path-based IPC variant.

Fix: add explicit arms for both variants in `categorize()` mapping to
`RateCategory::Expensive`.

---

## HIGH

### H1. Neither new privileged-mutating variant is audit-logged
`crates/pcloud-daemon/src/serve.rs:68-107`

`is_privileged_request` and `request_kind_name` do not list
`UploadWriteFromFile` or `CreateTreePublicLinkFromPaths`. Both mutate
remote state and the former reads arbitrary local files; they deserve
the same pre-dispatch audit line as `SyncRootRemove` / `DeleteBackup`.

### H2. sd_notify is incomplete for `Type=notify` supervision
`crates/pcloud-daemon/src/serve.rs:41-48, 314, 384`

Only `READY=1` and `WATCHDOG=1` are emitted. For production
`Type=notify` with `WatchdogSec=`, systemd expects `STOPPING=1` at the
start of drain and (ideally) `RELOADING=1` on SIGHUP hot-reload. Neither
is emitted; `RELOADING=1` absence means ordering against reload-dependent
units is silently wrong. `sd_notify` also silently swallows send
failures with no log.

Fix: emit `STOPPING=1\nSTATUS=draining` at `signals::begin_drain()` path
(serve.rs:245) and `RELOADING=1` around the `try_reload` block
(serve.rs:276-292), followed by `READY=1` when reload completes.

### H3. No health endpoint exposed by the daemon process itself
`crates/pcloud-daemon/src/metrics_server.rs:7` (comment only);
`crates/pcloud-web/src/routes.rs:76-114`

`livez`/`readyz` exist only in `pcloud-web` (a separate optional HTTP
surface). The daemon itself exposes `Method::GetHealth` / `Method::Health`
over IPC, but an enterprise deployment using systemd / k8s expects an
HTTP probe on the daemon. Marked HIGH because the bootstrap flow above
(`serve_with_shutdown`) does not spawn the metrics/health HTTP server,
so liveness probes cannot see the daemon unless pcloud-web is explicitly
deployed alongside.

### H4. `accept_and_spawn` exists but `serve_until_shutdown` still uses single-threaded `serve_once`
`crates/pcloud-ipc/src/transport.rs:272-318`; `crates/pcloud-daemon/src/serve.rs:294`

Thread-per-connection is implemented (`accept_and_spawn`) but the
production serve loop calls `bound.serve_once(|request| ...)` which
serializes all dispatch on the accept thread. A slow backend call
blocks the accept loop from delivering `DrainStatus` / `GetHealth`
during degraded operation, contradicting the docstring on
`serve_until_shutdown` (lines 132-137) which claims per-connection
threading. Either the docs or the wiring is wrong.

---

## MEDIUM

### M1. Privileged-request log uses `current_effective_uid()` instead of peer uid
`crates/pcloud-daemon/src/serve.rs:182-187`

The log line claims to record peer uid but hard-codes
`current_effective_uid()` (the daemon's own uid). The comment justifies
this by noting only owner-uid peers reach the handler, but the audit
line then conveys no useful information. Log the actual
`PeerIdentity { uid, pid }` recovered by the transport — that pid is
forensically useful even when uid is constant.

### M2. `retry_after_for` reserves a token by calling `bucket.acquire(1)`
`crates/pcloud-daemon/src/rate_limit.rs:241-257`

The comment acknowledges this deducts a token and relies on the
"reservation" contract. A client polling a rejected request therefore
has its retry hint computed against a permanently pre-reserved token,
which skews subsequent rejections toward longer waits than documented.
Prefer exposing a `TokenBucket::time_to_next_token()` non-consuming
method in `pcloud-resilience`.

### M3. Connection cap is process-global, not per-peer
`crates/pcloud-ipc/src/transport.rs:41-74`

`MAX_IPC_CONNECTIONS = 128` is a single `AtomicUsize`. Per-UID caps are
unnecessary (owner-only), but a buggy CLI fork-bomb can monopolize the
cap and lock out legitimate clients. Consider per-pid or short
accept-time limits.

### M4. Read timeout (5 s) is applied only after peer auth succeeds
`crates/pcloud-ipc/src/transport.rs:329, 466`

Peer credential recovery (`peer_identity`) happens *before*
`set_read_timeout`. A peer that connects and never sends bytes can't
block forever because `getsockopt(SO_PEERCRED)` is non-blocking, but
`read_framed_request` is the first call with timeout protection — fine
here, but `read_framed_request` is also used to drain the request on
unauthorized paths (lines 334, 345, 470, 481), which do have the
timeout set. OK; noted for completeness.

### M5. Proptest covers wire round-trip only; no dispatch proptest
`crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:588-606`

Both new variants have encode/decode round-trip coverage. There is no
proptest that fuzzes the `runtime::handle_request` dispatch arms with
arbitrary variants to prove the `catch_unwind` boundary and rate-limit
integration survive random input.

---

## LOW

### L1. `backend_label` in `dispatch.rs` contains a fallback `_ => "other"` for two tracked variants
`crates/pcloud-daemon/src/dispatch.rs:205-210`

Both new variants are mapped explicitly (good). However, the preceding
match still has a non-exhaustive `_ => "other"` catch-all
(line 210) which silently buckets any future `Request::*` addition
under "other", undermining observability. Consider removing the
catch-all and letting the compiler enforce exhaustiveness via
`#[non_exhaustive]`-aware match (current match is not exhaustive
because `Request` is marked `#[non_exhaustive]`, so a wildcard is
required — but a `log::warn!` on the wildcard path would surface drift).

### L2. `write_response` swallows IO errors on stale clients
`crates/pcloud-ipc/src/transport.rs:376`

`let _ = write_response(...)` suppresses `BrokenPipe` / `ConnectionReset`
silently. Harmless but violates the CLAUDE.md "less tolerant of silent
failures" rule; a trace-level log would close the gap.

### L3. `is_privileged_request` omits `LostPassword` and `VerifyEmailRestricted`
`crates/pcloud-daemon/src/serve.rs:68-84`

These trigger authenticated email dispatch to attacker-supplied
addresses in some configurations and deserve audit trails.

### L4. No explicit `MainPID=` notification on fork model
`crates/pcloud-daemon/src/serve.rs:384`

Not a blocker (Rust daemon is single-process), but if a future embedder
forks after bind, systemd would need `MAINPID=` in the notify path.
Leave as a docstring note.

---

## Summary

New `UploadWriteFromFile` and `CreateTreePublicLinkFromPaths` variants
are **wired** for dispatch and proptest round-trip, and
`CreateTreePublicLinkFromPaths` gates on authentication correctly
(runtime.rs:2600-2615 returns `Unauthorized` without a session). But
two security-relevant gaps ship with them:

1. `UploadWriteFromFile` reads arbitrary FS paths without validation or
   size limits (**CRITICAL**).
2. Neither is classified in the rate-limit categorizer; both default to
   `Medium` despite being `Expensive`-class work and despite the
   sibling id-based tree-link variant being explicitly `Expensive`
   (**CRITICAL** category-asymmetry bypass).

Secondary issues: missing privileged-audit entries for both variants,
incomplete sd_notify protocol (`STOPPING`/`RELOADING` absent), and a
doc/implementation mismatch on per-connection threading in
`serve_until_shutdown`.
