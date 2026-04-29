# Stream G10a — Code Quality Cross-Cutting Report

Date: 2026-04-26
Agent: Claude (Sonnet 4.6)
Scope: ≤30 targeted fixes across crates/*/src/**/*.rs

## Sites Changed

### 1. Silent error drop → log::trace (HIGH)
**File:** `crates/pcloud-ipc/src/transport.rs`
- **Before (line ~907):** `let _ = write_response(&mut stream, server, response.status, response.message);`
- **After:** `if let Err(err) = write_response(...) { log::trace!("…client disconnected?…") }`
- **Kind:** silent-error-drop (IPC response write failure invisible)
- **Justification:** `serve_stream_standalone_with_peer` was silently swallowing write failures to clients; the parallel `serve_once_with_peer` path already had log::trace. Aligned to same pattern.

### 2. Missing SAFETY comment (MEDIUM)
**File:** `crates/pcloud-ipc/src/transport.rs`
- **Before (line ~360):** `let ret = unsafe { libc::setsockopt(fd, ...) };` — no SAFETY comment
- **After:** Added `// SAFETY:` comment explaining fd validity, stack-allocated timeval lifetime, and setsockopt read-only contract
- **Kind:** unsafe block missing SAFETY rationale
- **Justification:** `set_accept_timeout` is called from daemon startup; missing safety documentation made FFI invariants opaque to reviewers.

### 3. Missing SAFETY comment (MEDIUM)
**File:** `crates/pcloud-fs/src/fuse_adapter.rs`
- **Before (line ~761):** `let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };` — comment describes *what* but not SAFETY invariants
- **After:** Added `// SAFETY:` comment explaining POSIX no-precondition contract and Linux/macOS availability
- **Kind:** unsafe block missing SAFETY rationale
- **Justification:** FUSE adapter initialization path; invariant documentation needed for auditing.

### 4. Missing SAFETY comment (MEDIUM)
**File:** `crates/pcloud-cli/src/main.rs`
- **Before (line ~1252):** `let send_rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };` — no SAFETY comment
- **After:** Added `// SAFETY:` explaining kill(2) POSIX safety, PID validity, and error handling
- **Kind:** unsafe block missing SAFETY rationale
- **Justification:** Daemon-stop path; signal delivery to user-supplied PID requires documented invariants.

### 5. Missing SAFETY comment (MEDIUM)
**File:** `crates/pcloud-cli/src/main.rs`
- **Before (line ~1403):** `let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGHUP) };` — no SAFETY comment
- **After:** Added `// SAFETY:` cross-referencing the SIGTERM pattern
- **Kind:** unsafe block missing SAFETY rationale

### 6. Missing SAFETY comment (MEDIUM)
**File:** `crates/pcloud-cli/src/main.rs`
- **Before (line ~1766):** `unsafe { cmd.pre_exec(|| { let _ = libc::setsid(); ... }) }` — no SAFETY comment
- **After:** Added `// SAFETY:` explaining post-fork pre-exec async-signal-safe constraint and setsid(2) POSIX guarantee
- **Kind:** unsafe block missing SAFETY rationale
- **Justification:** Second `pre_exec` site (background daemon spawn path) was missing documentation that the first site (daemonize path) already had.

### 7. assert! in constructor → Result-propagating try_new (MEDIUM)
**File:** `crates/pcloud-resilience/src/circuit_breaker.rs`
- **Before:** `CircuitBreakerConfig::new()` panicked on `failure_threshold == 0` with no `try_new` alternative
- **After:** Added `# Panics` doc to `new()` plus `try_new()` returning `Option<Self>`
- **Kind:** assert!/panic in public constructor reachable from user-config path
- **Justification:** `resilient_transport::build()` feeds user-config directly into this constructor; a zero threshold from a bad config file would abort the daemon.

### 8. assert! in constructor → Result-propagating try_new (MEDIUM)
**File:** `crates/pcloud-resilience/src/global_budget.rs`
- **Before:** `GlobalRetryBudget::new()` panicked on capacity==0 with no fallible alternative
- **After:** Added `# Panics` doc to `new()` plus `try_new()` returning `Option<Self>`
- **Kind:** assert!/panic in public constructor reachable from user-config path

### 9. assert! in constructor → Result-propagating try_with_clock (MEDIUM)
**File:** `crates/pcloud-resilience/src/retry.rs`
- **Before:** `RetryPolicy::with_clock()` panicked on bad max_attempts or factor with no fallible alternative
- **After:** Added `# Panics` doc to `with_clock()` plus `try_with_clock()` returning `Option<Self>`
- **Kind:** assert!/panic in public constructor reachable from user-config path

### 10. Use try_new variants in hot config path (HIGH)
**File:** `crates/pcloud-proto/src/resilient_transport.rs`
- **Before:** `build()` called `CircuitBreakerConfig::new()` and `RetryPolicy::with_clock()` with user-supplied config values — would panic on invalid config
- **After:** Calls `CircuitBreakerConfig::try_new()` and `RetryPolicy::try_with_clock()`, mapping `None` to `RateLimitError::InvalidConfig` and returning `Err` to caller
- **Kind:** panic-on-bad-config in daemon-reachable transport construction
- **Justification:** This is the hot path where a bad config file would abort the daemon process; now returns a typed error instead.

### 11. Silent error drop → log::debug (HIGH)
**File:** `crates/pcloud-web/src/routes.rs`
- **Before (lines ~356, 366):** `let _ = call_ipc(...).await;` silently discarding expiry/password set-on-create IPC failures
- **After:** `if let Err(err) = call_ipc(...).await { log::debug!("…") }` with explanatory comment
- **Kind:** silent-error-drop (best-effort but invisible IPC failures)
- **Justification:** Failures here do not block the create response (best-effort intent preserved) but now appear in debug logs for operator visibility.

### 12. Silent lock-poison-as-miss → log::warn + recovery path (MEDIUM)
**File:** `crates/pcloud-fs/src/write_path.rs` (tick() method)
- **Before (line ~1025):** `let h = h.lock().ok()?;` silently treats poisoned per-handle mutex as "not dirty"
- **After:** `match h.lock()` with explicit `Err(_)` arm that logs a warning and includes the inode in the flush set for error surfacing
- **Kind:** silent-error-drop (poisoned lock treated as cache miss)
- **Justification:** A poisoned per-handle mutex means a prior flush panicked mid-write; treating it as clean causes undetected data loss.

### 13. Silent lock-poison-as-miss → log::warn + recovery path (MEDIUM)
**File:** `crates/pcloud-fs/src/write_path.rs` (drain_all() method)
- **Before (line ~1071):** `let h = h.lock().ok()?;` in drain_all filter silently skips poisoned handles
- **After:** `match h.lock()` with explicit `Err(_)` arm that logs a warning and includes poisoned inodes in drain for error surfacing
- **Kind:** silent-error-drop (unmount drain silently drops potentially dirty inodes)
- **Justification:** drain_all() is called during unmount; skipping poisoned handles silently could leave unacknowledged writes unflushed.

## Summary

| Category | Count |
|---|---|
| unwrap/expect fixed (silent drop → log) | 3 |
| unsafe blocks annotated with SAFETY | 5 |
| stubs completed | 0 |
| try_new constructors introduced | 3 (circuit_breaker, global_budget, retry) |
| hot-path config-assert converted to Result | 1 (resilient_transport::build) |
| Total sites | 12 |

## Verification

```
cargo check --workspace --all-targets → 0 errors, 0 warnings
cargo test --workspace --lib --no-fail-fast → see test count
```

All changes are targeted edits; no large refactors. Crypto, journal, IPC Request enum variants, and other stream areas were not touched.
