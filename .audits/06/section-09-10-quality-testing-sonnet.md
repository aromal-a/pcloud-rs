# Audit 06 — Sections 9 & 10: Code Quality & Testing
**Auditor:** Sonnet (independent cross-validation of Opus audit-05)
**Date:** 2026-04-18
**Scope:** Post audit-05 claims: Mutex sweep, 52 SAFETY comments, FUSE expect fix, offline KAT in CI, typed transport, placeholders removed.

---

## §9 Code Quality

### HIGH

**H1 — 3,018 `.unwrap()` / `.expect()` calls in non-test `src/`**
The raw count from `grep` across all `crates/**/src/` (excluding `tests/`, `unwrap_or*`) is **3,018**. This is far above any threshold compatible with an enterprise daemon claim. The majority appear in helper and bench code, but a meaningful portion is in production paths (see specific instances below). Audit-05's claim of a "Mutex sweep" cannot have addressed this volume.
- *Concrete instances in production daemon code:*
  - `crates/pcloud-daemon/src/vault/mod.rs:434,444,452` — three `.expect()` calls in non-test vault selection logic; these panic if the auto-selection invariant is violated at runtime.
  - `crates/pcloud-cli/src/progress.rs:305,311,317` — three `lock().unwrap()` on a Mutex in a production progress renderer; a poisoned mutex causes a CLI panic.
- *Remediation:* Replace with `?`-propagated errors or `lock().unwrap_or_else(|p| p.into_inner())` for non-critical paths.

**H2 — Mutex poison panics in production CLI renderer**
`crates/pcloud-cli/src/progress.rs:305,311,317` uses `lock().unwrap()` on a shared `Mutex`. If a thread panics while holding the lock, the next caller panics unconditionally. The audit-05 "Mutex sweep" did not cover this file.
- *Remediation:* Use `lock().unwrap_or_else(|p| p.into_inner())`.

**H3 — Windows IPC is documented as STUB**
`crates/pcloud-ipc/src/platform/mod.rs:8` explicitly marks the Windows IPC path as `— STUB`. Windows is claimed as a tier-1 platform target. A stub IPC on a tier-1 platform is a deployment blocker.
- *Remediation:* Promote to HIGH, open a bead, or downgrade Windows to tier-2 in all docs.

### MEDIUM

**M1 — `unsafe` blocks without `// SAFETY:` in `pcloud-cli/src/`**
Multiple `unsafe` blocks in `crates/pcloud-cli/src/doctor.rs:733-734`, `prompt.rs:165,183,187,194,208`, `commands.rs:1510,1522,1533,1544,1549,1557,1562,1566`, `globals.rs:643-747` have no preceding `// SAFETY:` comment. Audit-05 claimed 52 SAFETY comments were added; these CLI sites were not covered.
- *Remediation:* Add `// SAFETY:` comment to every `unsafe {}` block. The `prompt.rs` sites (tcgetattr/tcsetattr, isatty) are straightforward to justify.

**M2 — `unsafe { std::env::set_var / remove_var }` called without SAFETY in multi-threaded context**
`crates/pcloud-cli/src/commands.rs:1510-1566` and `globals.rs:643-754` call `std::env::set_var` / `remove_var` inside `unsafe` blocks with no SAFETY comment explaining thread-safety guarantee. `set_var` is not thread-safe if other threads read the environment concurrently (Rust 1.80 added `unsafe` requirement for exactly this reason).
- *Remediation:* Add SAFETY comment documenting that these calls occur only on the single CLI thread before any concurrent tokio tasks are spawned, or use an alternative mechanism.

**M3 — Three `TODO(bd-follow-up)` without bead IDs**
- `crates/pcloud-daemon/src/audit_verifier_service.rs:456`
- `crates/pcloud-daemon/src/integrity_sweeper_service.rs:806,1071`
All three are `// is the intended behaviour. TODO(bd-follow-up): surface as Err.` — silent error swallowing on audit/integrity paths with no tracking bead. Per CLAUDE.md rule: silently swallowing persistence or audit failures on active control paths is prohibited.
- *Remediation:* Open beads under `bd-1du` epic and tag comments with bead IDs, or surface the errors immediately.

**M4 — Five `TODO(pcloud-rs-8mb.*)` without `bd-*` bead IDs**
- `pclsync_sector.rs:495` — return type zeroization gap (security-adjacent)
- `serve.rs:68,83` — launchd/rc.d signalling
- `sync_loop.rs:557` — daemon startup Err surface
- `sync_loop_runtime.rs:181` — AuditRepository load failure silent drop
None are linked to a `bd-` bead. The `8mb.*` reference namespace is opaque.
- *Remediation:* Resolve or open beads; the `pclsync_sector.rs` item is security-adjacent (zeroization) and should be promoted to HIGH.

**M5 — `pclsync_sector.rs:495` — sector decrypt return not zeroized**
`TODO(pcloud-rs-8mb.28/L-4)` notes that the return type should be `Zeroizing<Vec<u8>>` but is not yet. This is not cosmetic: sector-decrypted plaintext surviving in heap after use is a security weakness. Audit-05 did not mark this as resolved.
- *Remediation:* Upgrade return type before marking crypto complete.

**M6 — `sync_loop_runtime.rs:181` — AuditRepository load failure silently dropped**
`TODO(pcloud-rs-8mb.29/L-3)` documents that an `AuditRepository` load failure is silently dropped. CLAUDE.md mandates: "do not silently swallow persistence or audit failures on active control paths."
- *Remediation:* Surface the error as a daemon startup warning/error.

**M7 — AES-256-CTR pclsync-mode C-vector KAT missing**
`crates/pcloud-crypto/src/pclsync_modes.rs:496` contains a note: `NOTE(M-3.1 / bd-1du.10): a C-vector KAT for AES-256-CTR pclsync mode` — indicating the cross-client test vector is absent. Audit-05 claimed "offline KAT in CI", but this is only for blob parsing (fixture shape), not for the sector cipher round-trip against a known C-client ciphertext.
- *Remediation:* Add a sector-level C-derived ciphertext vector and test it in `pclsync_compat_kat_offline.rs`.

### LOW

**L1 — No `cargo llvm-cov` / tarpaulin in CI**
`ci.yml` has no coverage collection step. Coverage gaps for critical paths (IPC dispatch, auth vault, crypto lock/unlock, sync conflict resolution) are unquantified.
- *Remediation:* Add `cargo llvm-cov --workspace` to CI, upload to Codecov or similar, gate on minimum threshold.

**L2 — `pcloud-proto/src/account_api.rs:544` `.expect()` in non-test source**
`api.get_api_servers().expect("locations should parse")` is in `src/`, not `tests/`. If the server returns an unexpected shape, this panics the calling context.

---

## §10 Testing & QA

### HIGH

**H4 — Live E2E tests not wired in CI**
`ci.yml` has no `PCLOUD_LIVE_E2E=1` job. All tests in `crates/pcloud-live-e2e/tests/` are gated by `#[ignore]` and require manual invocation. Live suites covering auth, crypto, public-links, shares, transfers, and mount exist (`auth_lifecycle.rs`, `crypto.rs`, `transfers.rs`, `shares.rs`, `public_links.rs`, `mount_linux.rs`, `sync_loop_live.rs`), but zero run automatically. Parity matrix rows marked `Implemented` and `live-verified` are not automatically re-verified on each commit.
- *Remediation:* Wire a nightly CI job with a sandboxed pCloud test account; gate on `PCLOUD_LIVE_E2E` secret being set.

**H5 — FreeBSD CI job is `continue-on-error: true`**
`ci.yml:76` marks the FreeBSD job as `continue-on-error: true` with a comment "Tier-3: vmactions/freebsd-vm is flaky on GH runners." FreeBSD is documented as a tier-1 target for the FUSE path (`fuser` + libfuse2). A tier-1 platform with a non-blocking CI gate cannot be honestly called tier-1.
- *Remediation:* Either fix CI stability and remove `continue-on-error`, or explicitly downgrade FreeBSD to tier-2 in all docs.

**H6 — IPC stress test is `#[ignore]`-gated with no CI execution**
`crates/pcloud-ipc/tests/stress_concurrent_clients.rs:5` is gated behind `#[ignore]` with explicit manual run instructions. No CI job runs it. The IPC transport is a security-critical path (peer auth, connection limits, malformed-client isolation) that should be stress-tested on every push.
- *Remediation:* Run the stress test in CI with a bounded time limit (e.g., 60s) on Linux.

### MEDIUM

**M8 — Offline KAT scope is narrower than audit-05 claimed**
The offline KAT (`pclsync_compat_kat_offline.rs`) verifies fixture blob parsing and RSA public-key DER shape only. It does not exercise PBKDF2 derivation, AES-256-CTR sector decode, or a round-trip against a known C-client ciphertext (see M7 above). Audit-05's claim "offline KAT in CI" is technically correct but overstated in implied coverage.
- *Remediation:* Supplement with a fixture-based sector round-trip KAT that does not require a live password.

**M9 — FUSE lifecycle tests all `#[ignore]`-gated**
`fuse_write_path_live.rs`, `fuse_read_path_live.rs`, `fuse_dyn_shim_write.rs`, `fuse_kernel_e2e.rs`, `fuse_lifecycle_hardening.rs`, `fuse_small_write_wiring.rs` are all `#[ignore]` by default. The Linux mount path is claimed live-verified; but this is not continuously re-verified in CI. A regression introduced after audit-05 would be invisible until manual re-run.
- *Remediation:* Add a Linux-only CI job that runs FUSE tests in a privileged container (FUSE-capable) with `PCLOUD_FUSE_TEST=1`.

**M10 — No `cargo-fuzz` crypto sector target**
Fuzz targets exist for IPC frame parsing (`fuzz_ipc_frame.rs`) and JSON response parsing (`fuzz_json_response.rs`) but not for the crypto sector decoder — the highest-value target for a data-loss/corruption bug. `pcloud-crypto/fuzz/` does not exist.
- *Remediation:* Add a `fuzz_sector_decode.rs` target that fuzzes `pclsync_sector::decrypt`.

### LOW

**L3 — `pcloud-daemon/tests/macos_pcloud_live.rs` is `#[ignore]` with `#[cfg(target_os = "macos")]`**
macOS live tests will never run in CI (Linux runners). Not a regression, but confirms macOS tier-1 claims rest entirely on manual verification.

**L4 — Chaos / disk-full tests require `PCLOUD_CHAOS=1`**
`disk_full_journal.rs`, `slowloris_timeout.rs`, `sigkill_mid_flush.rs` are all `#[ignore]` + env-gated. These test critical crash-safety paths (journal replay, upload resume). No CI job runs them.

---

## Summary of audit-05 claim verification

| Claim | Verified? | Notes |
|---|---|---|
| Mutex sweep | Partial | `progress.rs` lock().unwrap() not covered |
| 52 SAFETY comments | Partial | CLI `unsafe` blocks still lack SAFETY comments |
| FUSE expect fix | Not independently verified (no specific file:line cited by audit-05) | |
| Offline KAT in CI | Confirmed in CI (default run) | Scope narrower than implied — no sector ciphertext vector |
| Typed transport | Not in scope for §9-10 | |
| Placeholders removed | Confirmed — no bare `unimplemented!()` in src/ | Windows IPC still marked STUB in doc comment |
