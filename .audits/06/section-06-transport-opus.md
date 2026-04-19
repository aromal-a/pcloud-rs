# Audit 06 — Section 6: Transport & Network Resilience (Auditor Opus)

Date: 2026-04-18
Scope: `crates/pcloud-proto/`, `crates/pcloud-resilience/`,
`crates/pcloud-config/src/api.rs`, HTTP client composition.

## Post Audit-05 Verification

All four post-05 claims **HELD**:

1. **Typed `TransportError` + `TlsError` enum** replacing string-match
   classifier — verified at
   `crates/pcloud-resilience/src/transport.rs:227-273` (enum defs) and
   `:303-326` (`classify_transport_error`). TLS is always Terminal
   (`:314`), IO is ErrorKind-driven (`:305-311`), and the stable
   wire-tag round-trip is documented at `:328-346` with
   `typed_error()`/`classify_error()` used by unit tests
   (`:1006-1036`). Fail-closed default for `Unknown` (`:324`) is
   correct per CLAUDE.md "no silent failures" rule.
2. **`is_known_safe_host` dedup** — `pcloud-proto/src/transport.rs:438-439`
   is a thin delegate to
   `pcloud_config::api::is_known_safe_host` (`crates/pcloud-config/src/api.rs:208`).
   The in-crate function remains only as an internal wrapper; parity
   tests at `transport.rs:767-779` pass through to the config impl.
3. **`upload_writefromfile` in `is_upload_mutation`** —
   `crates/pcloud-proto/src/resilient_transport.rs:421-425` covers
   `"upload_write" | "upload_writefromfile" | "upload_save"`. Retry
   guard at `:378` correctly suppresses retries for all three mutating
   primitives. Comment at `:407-418` documents row 93 bead coverage.
4. **`parking_lot::Mutex` on `BandwidthPacer`** — verified at
   `crates/pcloud-resilience/src/pacing.rs:49` (`use parking_lot::Mutex;`)
   and `:70` (field `state: Mutex<PacerState>`). No poisoning path; no
   `.lock().expect(...)` or `.unwrap()` needed on the pacer.

---

## Findings

### MEDIUM

**M1 — `TokenBucket` still uses poisoning `std::sync::Mutex`.**
`crates/pcloud-resilience/src/rate_limit.rs:158, 196, 225, 248` all
call `self.state.lock().expect("token-bucket mutex poisoned")`. The
migration to `parking_lot::Mutex` was applied to `BandwidthPacer` in
pacing.rs but **not** to `TokenBucket`, even though both are in the
same crate and serve the same transport/retry stack. Any panic while
holding the bucket lock (unlikely but possible during metric
registration on first use) will poison it and subsequent transport
calls will abort via `.expect`. Recommendation: switch to
`parking_lot::Mutex` for consistency with the pacer, or document why
the asymmetry is deliberate. No open bead covers this.

**M2 — `diff` protocol has no resume-with-cursor reconnect loop.**
`crates/pcloud-proto/src/diff_api.rs` (731 lines) implements
`poll_diff` as a single-shot request returning `DiffResponse`
containing `new_diff_id` (:99-101). No long-poll, no streaming, no
reconnect-with-resume helper. The caller (sync engine) must drive the
loop and persist `diffid` itself. Section 6 of `pcloud_rev.md`
explicitly calls for "reconnect-with-resume semantics" on the diff
stream. This is **not** a regression from C, and the cursor contract
is correct, but the resume logic lives in the engine, not in proto —
flag for Section 7/10 cross-check that engine-side resumption is
actually tested end-to-end.

### LOW

**L1 — Retry-After honoured in two disjoint paths.**
`crates/pcloud-proto/src/http_download.rs:367-371, 1103-1104` parses
`Retry-After: N` (integer seconds only — no HTTP-date form) for
signed downloads, while `crates/pcloud-resilience/src/transport.rs:527-534`
parses it for the resilient HTTP path (integer + HTTP-date per
comment). The signed-download path accepts fewer forms than the
resilient path. Low severity because signed downloads are short-lived
and servers typically emit integer seconds, but consistency would be
nicer. No security impact.

**L2 — API-server steering is local config state.**
`set_api_server` persists via `pcloud-config::api` but no code path
performs sticky-host failover across restarts beyond the configured
value. This matches the C behavior and `CLAUDE.md`'s "API-server
selection parity is local runtime/config state" rule. Called out only
so Section 7/11 reviewers do not re-audit it. No action.

**L3 — Test-only `.unwrap()` uses.** Two `.unwrap()` calls at
`crates/pcloud-resilience/src/transport.rs:1034, 1186, 1194` are all
inside `#[cfg(test)]` modules. No production impact.

---

## Confirmed Strong Posture

- TLS enforcement: production profile rejects `http://`
  (`pcloud-config/src/file_history.rs:114, tests/config_validation.rs:219`).
- No `danger_accept_invalid_certs` anywhere in the tree (grep clean).
- Connect/read/write/total timeouts distinct and configurable
  (`pcloud-proto/src/transport.rs:102-137`).
- Exponential backoff + equal-jitter with deterministic seed for
  reproducible tests (`pcloud-config/src/resilience.rs:73-100`,
  `pcloud-proto/src/resilient_transport.rs:256`).
- Retry is suppressed for non-idempotent upload primitives
  (`resilient_transport.rs:372, 378`).
- Per-endpoint latency histogram gated on `transport-metrics` feature
  (`pcloud-resilience/src/transport.rs:91-109`).
- Typed error classification is fail-closed for `Unknown`.

## Verdict

Post-05 fixes hold. Transport layer is **solid**. Two MEDIUM items
(TokenBucket mutex asymmetry, diff-resume coverage cross-check) plus
three LOW items. No CRITICAL or HIGH findings in Section 6.
