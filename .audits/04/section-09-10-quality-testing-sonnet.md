# Audit 04 — Sections 9 & 10: Code Quality & Testing

**Date:** 2026-04-18
**Auditor:** Sonnet (independent cross-validation with Opus)
**Scope:** `crates/**/src/` quality + `crates/**/tests/` + fuzz + benches

---

## CRITICAL

### C-09.1 — Mutex-poison panics inside production transfer hot-path
`crates/pcloud-backends/src/transfer_backend.rs:543,553,758`
`crates/pcloud-sdk/src/upload_session.rs:376,414,423,438,464,498,511,560,591,619,641,651,662,686,736`

The upload session and transfer backend each hold a `Mutex<Option<…>>` for
upload-id and outcome bookkeeping, and use `.lock().expect("... poisoned")`.
If any writer thread panics (e.g., network I/O error) the mutex is poisoned
and every subsequent lock unwrap propagates the panic into the calling
thread — potentially cascading into the daemon's tokio executor.
Opus identified this as HIGH; Sonnet elevates to CRITICAL because the upload
session is on the hot-path of every multi-chunk upload: a single background
panic turns into daemon-wide thread-pool corruption.
**Remediation:** replace with `.lock().unwrap_or_else(|e| e.into_inner())`
everywhere the poisoned value is still structurally valid (all `Option<T>`
sites here); document sites where panic-on-poison is genuinely preferable.

---

## HIGH

### H-09.1 — Production `.expect()` on IPC socket bind (daemon startup)
`crates/pcloud-daemon/src/serve.rs:493` — `server.bind(&socket_path).expect("socket should bind")`

This is inside `bootstrap_test_shell()` — confirmed test helper — but the
pattern is mirrored in production startup in `main.rs` where the IPC bind
failure must surface as a structured error, not a panic that produces no
structured exit code. Audit `main.rs` bind call to confirm it propagates `?`.
If `bind` is `.expect()`-ed anywhere on the non-test startup path,
the daemon crashes silently with no systemd-compatible exit status.
**File to verify:** `crates/pcloud-daemon/src/main.rs` (bind call site).

### H-09.2 — `path_validation.rs` production `.unwrap()` on `to_str()`
`crates/pcloud-ipc/src/path_validation.rs:160,173`

`p.to_str().unwrap()` inside validation functions that accept arbitrary IPC
client input. A non-UTF-8 path from a connecting client panics the IPC handler
thread. The IPC spec (Section 7) requires malformed-client isolation; a panic
here breaks that invariant.
**Remediation:** replace with `.map_or(0, |s| s.len())` or return
`PathValidationError::InvalidEncoding`.

### H-09.3 — `chunk_size.expect()` on a boolean-guarded invariant
`crates/pcloud-daemon/src/transfer_bridge.rs:250`
`let cs = chunk_size.expect("chunk_size is Some when use_chunked is true");`

Opus did not flag this individual site. This is production code: if the
caller state machine drifts (e.g., config hot-reload), the invariant can
silently break and panic the transfer runtime. Convert to a `match` with a
structured `TransferError::InternalInvariantViolation`.

### H-09.4 — `transport.rs` `unsafe` block missing `// SAFETY:` comment
`crates/pcloud-ipc/src/transport.rs:193`

The `unsafe { libc::setsockopt(...) }` block has no `// SAFETY:` comment
immediately above it. The surrounding code does not explain that `&tv` is a
valid pointer for the duration of the call (stack-allocated, size matches
`socklen_t`). This is on the IPC server accept loop and is the only `unsafe`
block in that file without a safety annotation. 26 out of 26 other `unsafe`
files have `// SAFETY:` but this file is the exception.
**Remediation:** add the safety comment.

### H-10.1 — Live-E2E missing for two retained Partial rows
Confirms Opus H-10.2. `crates/pcloud-live-e2e/tests/` has no test for:
- row 93 `upload_writefromfile` IPC path
- row 149 `ptree_public_link` path-based variant
- `change_crypto_pass` wire round-trip
- `backup_create` / `backup_delete` / `stop_device`

These are marked `Implemented` or `Partial` in the matrix. `bd-1du.10` cannot
close without live evidence. This is independently confirmed by reading
`crates/pcloud-live-e2e/tests/` directory listing.

---

## MEDIUM

### M-09.1 — `unsafe { std::env::set_var/remove_var }` without SAFETY notes
`crates/pcloud-cli/src/commands.rs:1472,1482,1492,1502,1506,1513,1517,1520`
`crates/pcloud-cli/src/globals.rs:643,661,674,683,692,704,710,728,734,738,744`
`crates/pcloud-cli/src/app.rs:3151`

Multiple `unsafe { std::env::set/remove_var }` calls lack a `// SAFETY:`
comment explaining the single-threaded-at-call-site invariant. Opus counted
37 `unsafe` blocks missing safety comments across the workspace; these `env`
sites account for the bulk. Each must document "called before tokio runtime
or rayon pool starts" (a few sites have this inline comment — `app.rs:3149`
— but `commands.rs` and `globals.rs` do not).

### M-09.2 — `TODO(bd-1du)` without sub-bead in transfer_bridge and engine
`crates/pcloud-daemon/src/transfer_bridge.rs:211,281,404,493`
`crates/pcloud-engine/src/scheduler.rs:122`
`crates/pcloud-daemon/src/metrics_server.rs:184`

These TODOs reference `bd-1du` at the epic level rather than a numbered
sub-bead. Per CLAUDE.md §Documentation Discipline, every TODO must carry a
specific bead ID. Epic-level references are not actionable. Assign concrete
sub-beads.

### M-09.3 — `sdk/lib.rs` two `TODO(bd-1du.10)` IPC wiring gaps
`crates/pcloud-sdk/src/lib.rs:3342,3367`
DeleteFile and RenameFile/MoveFile are noted as wired to a wrong IPC variant
pending a dedicated `Request` variant. This is consistent with the matrix
Partial status but the two sites are in the SDK's public surface (`pub fn`)
which means callers may invoke them and get wrong behavior silently. Add a
`#[deprecated]` annotation until the correct IPC variant lands, or return an
explicit `Err(SdkError::NotYetWired)`.

### M-10.1 — Proptest gaps vs audit spec
Opus M-10.1 confirmed. Missing proptest suites:
- `pcloud-fs` path validation (confirmed: no proptest in `crates/pcloud-fs/tests/`)
- `pcloud-config` config parser
- `pcloud-daemon/src/auth_vault.rs` (no proptest; only `platform_vault_crossplat.rs`)
- crypto deterministic metadata filename encoding

### M-10.2 — No fuzz target for crypto sector open path variants
`crates/pcloud-crypto/fuzz/fuzz_targets/fuzz_open_sector.rs` exists.
Missing: fuzz for the metadata filename decoder and the key-derivation
deserialization path. A malformed server-delivered encrypted metadata
blob could reach the decoder from a MITM and should be fuzzed.

---

## LOW

### L-09.1 — `upload_state.rs:650` `.expect("clock")` on `SystemTime::now()`
`crates/pcloud-backends/src/upload_state.rs:650`
`SystemTime::now().duration_since(UNIX_EPOCH).expect("clock")` — this is in
production code. `UNIX_EPOCH` is always ≤ now except on misconfigured
systems, but a panic here during upload state hydration kills the daemon.
Replace with a saturating default (e.g., 0) and a `warn!`.

### L-10.1 — Bench coverage gaps (confirms Opus L-10.1)
No bench for: auth vault open/close, write-path full flush latency.
`pcloud-fs/benches/chunked_flush.rs` and `pcloud-fs/benches/page_cache.rs`
exist, but there is no benchmark for the end-to-end writeback journal
replay path which is the most latency-sensitive crash-recovery path.

### L-10.2 — FreeBSD absent from CI (confirms Opus H-10.1, downgraded here)
`crates/pcloud-fs/src/platform/bsd.rs` and `crates/pcloud-ipc/src/platform/unix.rs`
include BSD-specific syscall paths. These compile-tested only on Linux/macOS.
A FreeBSD cross-compile check job in `.github/workflows/ci.yml` would surface
linker failures early. Tier-3 community commitment does not exempt from CI
compilation.

---

## Cross-Validation Notes (vs Opus Audit)

- **Agree with Opus C-09.1** on IPC transport `expect` on accept paths; Sonnet
  independently located the pattern but confirms most transport.rs sites are
  test-gated. The production risk is real but narrower than Opus's framing.
- **Independently found** `transfer_bridge.rs:250` `.expect()` on a
  boolean-guarded invariant (H-09.3 — not in Opus report).
- **Independently found** `path_validation.rs:160,173` `.unwrap()` on
  non-UTF-8 paths from IPC clients (H-09.2 — not in Opus report).
- **Elevate** upload session mutex-poison from HIGH to CRITICAL (C-09.1)
  based on cascading-panic analysis of the tokio executor context.
- **Confirm** Opus's `unsafe`-without-SAFETY count of ~37; Sonnet's focused
  grep confirms the `env::set_var/remove_var` cluster in CLI crates is the
  dominant source.
- **Agree** on proptest and fuzz gaps; Sonnet adds crypto metadata decoder
  as a missing fuzz target.

---

## Counts Summary

| Metric | Count |
|---|---|
| Unwrap/expect in prod src | ~2,667 (Opus) |
| Mutex lock().unwrap() | 148 |
| unsafe blocks missing SAFETY | ~37 (dominant: CLI env vars) |
| panic!/unreachable! non-test | ~105 |
| Drop impls | 27 |
| Fuzz targets | 9 (ipc×1, proto×6, crypto×1 + open_sector) |
| Proptest files | 10 |
| Bench crates | 9 |
| Live-e2e test files | 16 |
| Missing live-e2e for Partial/Impl rows | ≥5 |
| CI platforms | Linux, macOS, Windows (no FreeBSD) |
