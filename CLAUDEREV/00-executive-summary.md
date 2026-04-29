# pcloud-rs Enterprise Readiness Audit — Executive Summary

**Date:** 2026-04-29
**Auditor:** Claude Agent (12 parallel general-purpose audit subagents, one per dimension)
**Master prompt:** `pcloud_rev.md`
**Scope:** All `crates/**`, root `.md` documentation, `RUST-PLANS/`, parity truth files, CI workflows, packaging.
**Method:** Read-only audit. No source files modified. Each dimension produced a standalone report under `CLAUDEREV/01-…12-…md`; this summary consolidates them.

---

## Executive Summary

pcloud-rs has reached a level of engineering quality that **substantially exceeds** what is implied by a "rewrite of a legacy C client". The Rust path is genuinely stricter than the upstream C client on every security axis the master prompt enumerated: `SecretString`/`SecretBytes` zeroization is pervasive, the auth vault has correct 0600/0700 hygiene with atomic write, owner-only IPC with peer-credential checks is enforced on all live platforms, and the production transport policy rejects plaintext at three independent gates. The crypto subsystem implements both PclsyncCompat (PBKDF2-HMAC-SHA512 + RSA-4096-OAEP + sector AEAD, byte-compatible with pCloud apps) and an opt-in Enhanced backend (AES-256-GCM + Argon2id) with a `--acknowledge-not-interop` gate and a `BackendMismatch` error that prevents silent fallback. Parity row reachability is genuinely good — a 25-row spot-check against the active `crates/` tree matched the matrix for every entry checked.

The audit nevertheless surfaces **1 CRITICAL, 41 HIGH, 68 MEDIUM, 53 LOW** findings across the 12 dimensions. The single CRITICAL is a Windows-only mount-reaper registry that is fully implemented but never wired up in production: `mount_with_winfsp_dyn` (the only WinFSP entry point used by the daemon) does not call `reaper::register_mount`, so on a Windows process crash a stale WinFSP volume can be left behind. The HIGH findings cluster around three themes: (a) **resilience plumbing in production paths** — the `ResilientTransport` (circuit breaker / global retry budget / token bucket) is implemented but unreachable from production HTTP backends, and the same shape recurs on the BSD/Windows reaper paths; (b) **cross-platform CI gaps** — macOS, Windows, and FreeBSD jobs all exclude `pcloud-fs`, so Tier-1 claims have no enforcement; live-E2E CI is `continue-on-error: true`; (c) **doc-vs-code drift** — STATUS.md headline counts contradict its own internal tables (154/2/0/30 vs 153/3/0/30), `install.md` references binary names and a Rust MSRV that no longer match the workspace, and CLAUDE.md references the `bd-1du.*` bead family that does not exist as live tracker IDs (the beads visible under `.beads/` use the `pcloud-rs-ncx.*` naming and are all closed).

**Bottom line:** the code is much closer to enterprise-deployable than the documentation suggests. Closing the parity-proof epic (whatever its actual bead ID) is gated overwhelmingly by **work that is properly out of AI scope** — hardware-attached macOS/Windows mount verification, signed package distribution, and human reviewer sign-off — rather than missing implementation. With the 1 CRITICAL and the top ~10 HIGHs addressed (estimated 2–3 engineering weeks), pcloud-rs would be defensible as a production deployment for a single Linux operator. Tier-1 claims for macOS and Windows would require an additional ~4–6 weeks of platform-specific live verification and CI build-out.

---

## Findings by Severity (consolidated)

| Severity | Count | Notes |
|----------|------:|-------|
| **CRITICAL** | 1 | Windows mount path never registers with its own reaper (FUSE dim 5) |
| **HIGH** | 41 | Concentrated in resilience wiring, cross-platform CI, and doc accuracy |
| **MEDIUM** | 68 | Quality / completeness / polish issues with concrete remediation |
| **LOW** | 53 | Enhancements and stylistic items |

Per-dimension breakdown:

| Dim | Title | C | H | M | L |
|----:|-------|--:|--:|--:|--:|
| 1 | C-to-Rust Feature Parity & API Coverage | 0 | 2 | 2 | 3 |
| 2 | Security | 0 | 4 | 7 | 4 |
| 3 | Crypto Subsystem | 0 | 3 | 5 | 4 |
| 4 | Sync Engine & Runtime | 0 | 6 | 8 | 5 |
| 5 | Mounted-drive / FUSE Parity | 1 | 5 | 8 | 7 |
| 6 | Transport & Network Resilience | 0 | 1 | 3 | 3 |
| 7 | IPC & Daemon | 0 | 2 | 5 | 4 |
| 8 | CLI & SDK Surface | 0 | 1 | 5 | 4 |
| 9 | Code Quality & Robustness | 0 | 2 | 3 | 3 |
| 10 | Testing & QA | 0 | 7 | 9 | 6 |
| 11 | Deployment & Operations | 0 | 4 | 7 | 6 |
| 12 | Documentation Quality | 0 | 4 | 6 | 4 |
| **Total** | | **1** | **41** | **68** | **53** |

---

## The 1 CRITICAL

**[FUSE-C-1] Windows production mount entry never registers with its own reaper.**
`crates/pcloud-fs/src/platform/windows.rs:1931-2138` defines a complete `ACTIVE_MOUNTS` registry plus a `StopDispatcher` closure type and `reaper::register_mount` API. `mount_with_winfsp_dyn` — the only production WinFSP mount path called from `pcloud-daemon` — does not call `register_mount`, so on a daemon crash or an unexpected `SIGTERM`-equivalent the `FspFileSystemStopDispatcher` callback is never invoked and a stale WinFSP volume can leak. **Remediation:** call `reaper::register_mount` from `mount_with_winfsp_dyn` before returning the handle, with an `unregister_mount` in the handle's `Drop`. ~50 lines of plumbing; pattern is fully implemented and proven on Linux. See `CLAUDEREV/05-fuse-mount.md`.

---

## Top HIGH Findings (selected — full lists in per-dimension reports)

### Resilience and runtime correctness
- **[TRANSPORT-H-1]** Production HTTP backends bypass `ResilientTransport`. Circuit breaker, rate limit, and global retry budget are implemented but unreachable from the production hot path. (`CLAUDEREV/06-transport.md`)
- **[FUSE-H-2]** BSD reaper never wired — same shape as the Windows CRITICAL.
- **[FUSE-H-3]** Linux `Drop` settle-window blocks for up to 7 s, can stall daemon shutdown beyond `WatchdogSec=`.
- **[SYNC-H-04-1]** `fs_watcher` silently drops kernel events on overflow with no telemetry and no recovery scan — sync state can quietly diverge.
- **[SYNC-H-04-2]** Hand-rolled debouncer stalls on continuous churn instead of using `notify-debouncer-full`.
- **[SYNC-H-04-3]** `pause_on_battery` is a silent no-op on macOS and Windows; field claim violates the strict-truthfulness rule from CLAUDE.md.
- **[SYNC-H-04-4]** Case-insensitive filename collision blindness on macOS/Windows despite an existing unused `probe_case_insensitive_fs` helper.

### Security
- **[SEC-H-1..H-3]** Bearer-credential `String` fields in `pcloud-proto::auth_api`, `pcloud-proto::account_api`, `pcloud-ipc::methods`, and `pcloud-web::WebConfig` should be `SecretString`. Wrappers exist; sites missed.
- **[SEC-H-4]** TLS revocation is **off by default** in production — already tracked under `pcloud-rs-t9o`, called out for visibility.

### Crypto
- **[CRYPTO-H-1]** No cross-interop KAT (Known Answer Test) against the C client for the Enhanced backend. Status of round-trip is asserted, not proven against canonical ciphertext.
- **[CRYPTO-H-2]** PclsyncCompat share-invitation path is gated off; `share_rsa::wrap_share_invitation_b64` exists but is unwired. STATUS.md rows 124/142 honestly remain **Partial**. **Independently corroborated** by re-ran `cargo doc --workspace --no-deps` (2026-04-29): three of the 46 rustdoc unresolved-link warnings reference `pcloud_crypto::share_rsa::wrap_share_invitation_b64` from production sites at `crates/pcloud-proto/src/methods/shares.rs:107`, `crates/pcloud-proto/src/methods/shares.rs:343`, `crates/pcloud-proto/src/shares_api.rs:512` — the symbol is doc-referenced from three call sites but not resolvable as a public item. (See `CLAUDEREV/12-documentation.md` MEDIUM-1.)
- **[CRYPTO-H-3]** Merkle auth-tree parent tags miss the AES-ECB step of the pclsync spec → multi-sector files cannot round-trip the master tag against the C client.

### Daemon & IPC
- **[IPC-H-7.1]** No per-request capability tier — `is_privileged_request` is audit-only `log::info!`. Any authenticated peer can call `Shutdown`, `CryptoReset`, `AccountChangePassword`, etc.
- **[IPC-H-7.2]** Connection-cap counters are process-global statics, fragile for multi-listener embedders.

### Testing & CI
- **[TEST-H-1]** Live-E2E CI job is `continue-on-error: true` — regressions never fail the gate.
- **[TEST-H-2]** Retained parity rows for TFA flow, account utility, `upload_writefromfile`, and crypto team-share temppass have **zero live coverage** despite CLAUDE.md citing them as live-verified.
- **[TEST-H-3]** `change_crypto_pass` live test body is `todo!()`.
- **[TEST-H-5/6/7]** macOS, Windows, and FreeBSD CI jobs all **exclude `pcloud-fs`** — Tier-1 mount claims are not enforced by CI.

### Deployment
- **[DEPLOY-H-11.1]** Windows MSI installs a Service that compiles but is a no-op (`serve_with_shutdown` is `Unsupported` on Windows). Confirms the daemon-Windows blocker tracked under `bd-xplat-windows`.
- **[DEPLOY-H-11.2]** `.deb`/`.rpm` build (nfpm) not wired into CI; no signed/reproducible package pipeline.
- **[DEPLOY-H-11.3]** Shipped systemd unit's `IPAddressDeny=any` blocks the pCloud API by default; postinst does not auto-install the override drop-in. **A vanilla `dnf install`/`apt install` of the package will not reach the daemon.**
- **[DEPLOY-H-11.4]** No `CryptoPolicy::fips_mode` runtime gate; any FIPS claim would currently be unsupported.

### Code quality
- **[CQ-H-1]** `cargo fmt --all --check` dirty in 35 files.
- **[CQ-H-2]** clippy is not gated `-D warnings`; 3 latent warnings present.

### Documentation
- **[DOC-H-1]** STATUS.md headline (154/2/0/30) contradicts its own summary tables (153/3/0/30) at lines 669–672 / 692–695.
- **[DOC-H-2]** API-REFERENCE.md mis-states rows 23/24/93 statuses vs the CSV.
- **[DOC-H-3]** `install.md` references `target/release/pcloud-daemon` (actual binary: `pcloudd`), pins `Rust 1.80+` (workspace MSRV: `1.85`), references `pcloud-daemon.1` (actual man page: `pcloudd.1`).
- **[DOC-H-4]** Book ADR TOC stops at 0010 then jumps to 0019; ADRs 0011–0018 unreachable from the mdBook nav.

### Parity tracker
- **[PARITY-H-1]** CLAUDE.md and STATUS.md reference `bd-1du`, `bd-1du.4`, `bd-1du.5`, `bd-1du.10` throughout. The live `.beads/issues.jsonl` has **no such IDs**; real beads use the `pcloud-rs-ncx.*` family and are all closed. Tracker references in handoff docs are stale.
- **[PARITY-H-2]** CLAUDE.md "open epics (3 beads)" section is contradicted by an empty open-bead list. The text was correct at one point and has not been updated since the underlying epics closed.

---

## Strengths Worth Preserving

- **Secret discipline.** `SecretString`/`SecretBytes` zeroize on Drop, redacted Debug, present at every site the audit could find — except the four HIGH cases above. Logging macros never carry secrets in any sampled crate.
- **Auth vault correctness.** Owner-only file (0600), owner-only parent (0700), atomic write (tmp+rename), opt-in persistence, no plaintext password persisted. Posture is materially stricter than the C client.
- **TLS posture.** Three independent gates reject `http://` in production (config validation, private `use_tls` field, rustls TLS13-only). No `danger_accept_invalid_certs` in source.
- **API-server steering.** Hint allowlist (`*.pcloud.com`/`*.pcloud.link`), sticky across restarts via SQLite preferences row, hostname+cert validated.
- **Idempotency.** `upload_create` → `upload_write` → `upload_save` carries a stable per-driver CSPRNG-derived 128-bit key end-to-end.
- **Backoff hygiene.** `Retry-After` honored (RFC 7231 IMF-fixdate + delta-seconds, 300 s cap, doesn't burn budget tokens).
- **Crypto fundamentals.** Fresh AES-GCM nonce per encrypt, CAS-loop nonce-budget cap, constant-time everywhere relevant, brute-force lockout persisted across restarts with monotonic-time floor.
- **`unsafe` discipline.** 411 `unsafe` blocks; spot-check found 31 missing `// SAFETY:` comments — a finding, but the dominant pattern is correct documentation.
- **TODO discipline.** 48 markers, 42 carry an explicit bead/release ID, 0 unscoped after filtering.
- **Linux mount.** Genuinely production-quality and live-verified end-to-end (`tests/fuse_write_path_live.rs` against a real kernel mount).
- **Parity matrix accuracy.** 25/25 spot-checked rows reachable; all 30 Rejected rows have 1:1 rationales in `REJECTED-RATIONALES-14042026.md`.

---

## Remediation Roadmap

### Phase 1 — Critical & "embarrassing in a sysadmin's first hour" (target: 2 weeks)
1. **FUSE-C-1**: Wire `mount_with_winfsp_dyn` to the Windows reaper registry. (~50 lines.)
2. **DEPLOY-H-11.3**: Ship the systemd `IPAddressAllow=` drop-in alongside the unit, or remove `IPAddressDeny=any` and document the trade-off. **Without this, a clean install does nothing.**
3. **DOC-H-1/H-2/H-3**: Fix STATUS.md headline-vs-table contradiction; fix API-REFERENCE row 23/24/93; fix install.md binary names + MSRV.
4. **PARITY-H-1/H-2**: Either re-open the `bd-1du.*` beads or update CLAUDE.md and STATUS.md to use the actual `pcloud-rs-ncx.*` IDs that are still alive.
5. **TRANSPORT-H-1**: Route production HTTP backends through `ResilientTransport` so the implemented circuit breaker, retry budget, and rate limit actually engage.

### Phase 2 — Security hardening (target: 2 weeks)
6. **SEC-H-1..H-3**: Migrate the four bearer-credential `String` fields to `SecretString` (`pcloud-proto::auth_api`, `pcloud-proto::account_api`, `pcloud-ipc::methods`, `pcloud-web::WebConfig`).
7. **SEC-H-4**: Default-on TLS revocation in production, behind a `--insecure-no-revocation` opt-out.
8. **IPC-H-7.1**: Promote `is_privileged_request` from audit-only to a denied-by-default capability tier; require an explicit `requires_privileged: true` flag on each IPC `Request` variant via a typed match arm.
9. **CQ-H-2**: Gate clippy `-D warnings` in CI; clear the 3 latent warnings.
10. **CQ-H-1**: Run `cargo fmt --all` once across the 35 dirty files and lock with a `pre-commit` hook.

### Phase 3 — Parity epic closure (target: 4–6 weeks; partly out of AI scope)
11. **CRYPTO-H-1**: Capture KAT vectors from the C client for the Enhanced backend; round-trip tests.
12. **CRYPTO-H-2/H-3**: Wire `share_rsa::wrap_share_invitation_b64` into PclsyncCompat; add the AES-ECB step to the Merkle auth-tree parent tag.
13. **TEST-H-1**: Remove `continue-on-error: true` from live-E2E CI once a stable account is provisioned.
14. **TEST-H-2/H-3**: Add live coverage for TFA, account utility, `upload_writefromfile`, crypto team-share temppass; replace `todo!()` body in `change_crypto_pass` test.
15. **TEST-H-5/6/7**: Build out macOS/Windows/FreeBSD CI to include `pcloud-fs` (or honestly downgrade Tier-1 → Tier-2 in CLAUDE.md until built).
16. **DEPLOY-H-11.1**: Land Windows named-pipe accept loop and live-WinFSP mount. (Out of AI scope per CLAUDE.md "Windows posture".)
17. **FUSE remaining**: macOS/Windows live mount on real hardware. (Hardware — out of AI scope.)

### Phase 4 — Polish & ops excellence (target: 2 weeks, opportunistic)
18. **DEPLOY-H-11.2**: Wire `.deb`/`.rpm` (nfpm) build into CI; reproducible-build bit-identity check.
19. **DEPLOY-H-11.4**: Decide FIPS-mode posture; either implement or remove from any forward-looking docs.
20. **SYNC-H-04-1..H-04-4**: Replace hand-rolled debouncer with `notify-debouncer-full`; add overflow telemetry + recovery scan; honor battery state on macOS/Windows; activate `probe_case_insensitive_fs`.
21. **DOC-H-4**: Restore ADRs 0011–0018 to the mdBook TOC.
22. **CQ-M-1**: Add `// SAFETY:` comments to the 31 `unsafe` blocks that lack one (mostly in `pcloud-fs/src/platform/{macos,windows,bsd,linux,winfsp_ffi}.rs`).
23. **MED & LOW** items per per-dimension reports.

---

## Verdict on `bd-1du.10` (Final Parity Proof)

The audit's **independent verdict** matches the per-row finding from dimension 1: parity work is materially complete from AI scope. The matrix is accurate, the Rejected rationales are 1:1, the active code is reachable from live callers, and the active tracker is empty. The remaining gates are **human and hardware**:

- Windows named-pipe accept loop + live WinFSP mount on real hardware — `bd-xplat-windows` (or its actual current ID) — **out of AI scope.**
- macOS live mount on real Darwin hardware — **out of AI scope.**
- BSD rc.d supervision end-to-end on real FreeBSD — Tier-3 community best-effort, **out of AI scope.**
- Reproducible-build bit-identity check across two hosts — **build infrastructure work**, not in-tree code.
- Human reviewer sign-off — **out of AI scope.**

Therefore: **do not block `bd-1du.10` (or its real ID equivalent) on AI work.** Block it on the five gates above plus the Phase 1–2 items in this report (FUSE-C-1, DEPLOY-H-11.3, DOC-H-1..H-3, PARITY-H-1, TRANSPORT-H-1, the four SEC HIGHs, IPC-H-7.1) — those are real Rust work, scoped to ~4 engineering weeks.

---

## Per-dimension reports

For full evidence with file:line citations and remediation per finding:

- [01-parity.md](01-parity.md)
- [02-security.md](02-security.md)
- [03-crypto.md](03-crypto.md)
- [04-sync-engine.md](04-sync-engine.md)
- [05-fuse-mount.md](05-fuse-mount.md)
- [06-transport.md](06-transport.md)
- [07-ipc-daemon.md](07-ipc-daemon.md)
- [08-cli-sdk.md](08-cli-sdk.md)
- [09-code-quality.md](09-code-quality.md)
- [10-testing.md](10-testing.md)
- [11-deploy-ops.md](11-deploy-ops.md)
- [12-documentation.md](12-documentation.md)
