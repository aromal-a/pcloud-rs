# pcloud-rs Enterprise Readiness Audit Report

**Date:** 2026-04-17
**Auditor:** Claude Agent (multi-agent parallel audit — 10 Opus 4.7 specialists)
**Scope:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/` including `crates/`
**Audit prompt:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/pcloud_rev.md`
**Methodology:** 10 parallel specialist auditors, each owning 1–2 of the 12 audit dimensions, writing per-section findings with file:line references. Findings synthesized into this unified report. No source files were modified by the audit.

---

## Executive Summary

**Overall readiness:** pcloud-rs is a *substantively implemented* clean-room Rust rewrite of the pCloud client. The gating discipline set out in `CLAUDE.md` — no false "parity" / "production ready" / "drop-in" claims, stricter-than-C security posture, evidence-before-closure on `bd-1du.10` — is **visibly enforced throughout the code and docs**. All core workspace gates pass: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo deny --locked check` are green. Secret wrappers (`SecretString`/`SecretBytes`) zeroize correctly with constant-time compares, the auth vault is opt-in with `0600` file + `0700` parent dir + atomic tmp+rename writes, production transport refuses plaintext, `danger_accept_invalid_certs` appears nowhere in `src/`, the parity matrix / STATUS / review files are internally consistent (186 / 158 / 0 / 0 / 28), and every `Rejected` row has a matching rationale in `REJECTED-RATIONALES-14042026.md`.

**It is not, however, deployment-ready today.** The blockers cluster into seven groups, all of which must close before `bd-1du.10` can honestly be satisfied:

1. **The sync engine is structurally non-functional under realistic load** — the scheduler's `next_batch` is a pure peek that never dequeues (`scheduler.rs:122-127`), so every cycle re-emits the same operations; the default `rename_both` conflict policy does not rename and the `newest_wins` policy ignores timestamps (`conflict_resolver.rs:170-191`), silently destroying local edits; queues are unbounded; stall detection is absent; engine state is entirely non-durable across restart; `crates/pcloud-engine/tests/` does not exist.
2. **FUSE (`bd-1du.4`) is scaffolding, not production** — the fuser shim has no `statfs` (`df` ENOSYSes on the mount), the write journal `commit()` fsyncs the file but not the parent directory (data-loss window that contradicts its own doc contract), `ProtoUploadBackend::upload_file` slurps entire staging blobs into memory (OOM on large files), journal `replay_path` exists but the daemon never calls it on startup, all kernel-mounted FUSE integration tests are `#[ignore]`+env-gated (CI runs zero), and `MountService::mount` has no Windows arm.
3. **There is no continuous integration at all** — `.github/workflows/` does not exist; `fuzz/README.md` and `codecov.yml` reference pipelines that aren't there; the codecov hard-flip date `2026-04-29` is 12 days from now; **every tier-1 platform claim (Linux / FreeBSD / macOS / Windows) is currently unsubstantiated**.
4. **No per-request IPC capability scoping** — any local process running as the daemon's user reaches `Shutdown`, `CryptoReset`, `Logout`, `CryptoChangePassword*` and every other privileged method. The only gate at accept is uid-match.
5. **One orphan IPC handler and several cross-document path drifts** — `Request::VerifyPath` is constructed by the CLI (`commands.rs:1102`) but `runtime.rs` has zero handler for it (it falls through to the "unsupported ipc request" arm); 41+ rows in `C_FEATURE_PARITY_MATRIX.csv` cite `crates/pcloud-daemon/src/*_backend.rs` paths that no longer exist (all moved to `crates/pcloud-backends/src/`), and the same stale paths carry into `ARCHITECTURE.md`, `API-REFERENCE.md`, `SECURITY.md`, and `CLAUDE.md` itself.
6. **Crypto byte-compatibility with the legacy C client is unverified** — the Rust `pcloud-crypto` crate uses AES-256-GCM + HMAC-SHA256 primitives and there is no known-answer test (KAT) against `pclsync/pcryptofolder.c` output. Files encrypted by the legacy C client may not round-trip through the Rust client. Additionally, password rotation silently invalidates all existing sector ciphertext because per-file keys are `HMAC(master, …)` — a data-loss trap with no warning to the user.
7. **Packaging/release will wedge** — `packaging/windows/wix/pcloud-rs.wxs:14` ships a placeholder `UpgradeCode="PUT-A-STABLE-GUID-HERE-0000-000000000000"`; any MSI shipped with this GUID cannot be upgraded.

**Documentation drift correction.** Both the §1 parity auditor and the §3 crypto auditor independently confirm that `CLAUDE.md` is wrong about four symbols it lists as "missing": `change_crypto_pass`/`change_crypto_pass_unlocked`, `send_change_user_private`, `priv_key_flags`, and `psync_send_publink` are **all implemented** with live code, daemon dispatch, SDK helpers, and tests. The parity matrix already reflects this; `CLAUDE.md` must be reconciled.

**Top strengths.** Workspace discipline is strong: `#![forbid(unsafe_code)]` holds crate-wide in `pcloud-crypto`, all nonces/IVs come from `OsRng`, the master key is never serialised, policy gates reject `persist_master_key=true`, temppass signatures are verified before AEAD unwrap, every tested FFI `unsafe` block in `pcloud-fs/platform/*` that was spot-checked carries a `SAFETY:` comment, graceful-drain has a real state machine, the circuit breaker is panic-safe via `ProbeGuard`, Windows named-pipe peer checks do a real SID comparison, the systemd unit at `packaging/systemd/pcloudd.service` is unusually hardened for a first-party service, and CLAUDE.md's no-false-claims rule is visibly enforced across 10+ docs.

---

## Findings by Severity

| Severity | Approx count | Meaning |
|---|---|---|
| **CRITICAL** | ~23 | blocks deployment / data loss / security vulnerability |
| **HIGH** | ~89 | significant gap / compliance risk / correctness bug |
| **MEDIUM** | ~120 | quality issue / missing feature / doc drift |
| **LOW** | ~95 | enhancement / polish |

Per-dimension breakdown (taken from each specialist's findings index):

| Dimension | CRIT | HIGH | MED | LOW | Headline |
|---|---|---|---|---|---|
| 1. Parity & API Coverage | 1 | 4 | 4 | — | `Request::VerifyPath` has no daemon handler |
| 2. Security | 0 | 4 | 9 | 9 | 60+ proto structs expose `pub auth_token: String` with `#[derive(Debug)]` |
| 3. Crypto | 1 | 7 | 8 | 9 | no cross-client KAT vs C `pcryptofolder.c` |
| 4. Sync Engine | 8 | 14 | 18 | 20 | scheduler never dequeues; conflict policies broken |
| 5. FUSE Parity | 8 | ~15 | — | — | no `statfs`, journal `fsync` gap, OOM upload, no Windows arm |
| 6+7. Transport + IPC | 1 | 10 | — | — | no IPC capability scoping; retry classifies `InvalidCertificate` as transient |
| 8. CLI + SDK | 0 | 11 | 20 | 14 | hand-rolled parser + clap completion tree drifted; positional secrets |
| 9. Code Quality | 0 | 4 | — | — | ~40 `expect("poisoned")` on daemon hot paths; gates **green** |
| 10. Testing | 2 | ~8 | — | — | **no CI; `.github/workflows/` does not exist** |
| 11+12. Deploy + Docs | 2 | 12 | 26 | 22 | Windows MSI UpgradeCode placeholder; 41+ matrix rows cite dead paths |

Totals are approximate because some agents grouped findings without publishing a full MED/LOW tail; exact tables live in each detailed section below.

---

## Remediation Roadmap

### Phase 1 — Critical Blockers (must fix before ANY deployment)

1. **Sync scheduler correctness.** `crates/pcloud-engine/src/scheduler.rs:80-127` — replace the flat `Vec`-sorted-by-priority with per-root fairness (round-robin or weighted-deficit); make `next_batch` actually remove dequeued ops so completion doesn't re-emit them forever. Write an integration test that runs two sync roots and asserts the second one makes progress.
2. **Conflict resolver correctness.** `crates/pcloud-engine/src/conflict_resolver.rs:170-191` — fix `newest_wins` to compare timestamps; fix `rename_both` to actually rename (the default policy silently does nothing). Add a property test that exercises both.
3. **Sync engine persistence.** Persist queue state + retry state + in-flight transfers across restart. Today a daemon crash mid-sync drops everything.
4. **FUSE `statfs`, journal fsync, streamed upload.** `crates/pcloud-fs/src/platform/fuser_shim.rs` — implement `statfs`; `crates/pcloud-fs/src/journal.rs` `commit()` — fsync the parent directory, not just the file (MS-FSA §6.5); `crates/pcloud-fs/src/backend.rs` `upload_file` — stream from disk, don't buffer the whole staging blob.
5. **FUSE journal replay on boot.** Wire `replay_path()` into the daemon's startup bootstrap; an orphaned journal is currently dead data.
6. **IPC capability scoping.** `crates/pcloud-daemon/src/runtime.rs` dispatch path — gate every privileged method behind an explicit capability check before uid-match authorization. Today any local process running as the same user reaches `Shutdown`/`CryptoReset`/`Logout`/`CryptoChangePassword*`.
7. **`VerifyPath` handler.** Either wire a daemon handler in `runtime.rs` for `Request::VerifyPath` or remove it from the CLI (`commands.rs:1102`) and mark `Rejected` in the matrix.
8. **Windows MSI UpgradeCode.** `packaging/windows/wix/pcloud-rs.wxs:14` — replace the placeholder GUID with a real, committed, permanent GUID. Once an MSI ships with a real GUID, it cannot be changed without breaking upgrades.
9. **Stand up CI.** Create `.github/workflows/*.yml` (Linux / macOS / FreeBSD / Windows) that actually compiles and tests per-platform. Without CI every tier-1 claim is vapor.
10. **Cross-client crypto KAT.** Add `crates/pcloud-crypto/tests/kat_legacy_c.rs` that decrypts a sample file encrypted by the upstream C `pcryptofolder.c` — or, if the byte formats are intentionally divergent, document it loudly in `SECURITY-MODEL.md` and flag legacy-file migration as a user-visible workflow.
11. **Password-rotation data preservation.** Either re-encrypt all sector ciphertext on `change_crypto_pass`, or introduce a KEK indirection so rotation does not invalidate prior ciphertext. Today rotation silently locks the user out of their own data.

### Phase 2 — Security Hardening (must fix before production)

1. **Debug-redact the proto request builders.** `crates/pcloud-proto/src/methods/**` — 60+ structs carry `pub auth_token: String` / `pub password: String` with `#[derive(Debug)]`. Replace the fields with `SecretString` or implement a redacting `Debug`.
2. **Path input validation.** `Request::SyncRootAdd.local_path` is accepted into `runtime.rs:3952` without NUL / `..` / symlink-escape checks before `canonicalize`. Add a shared `validate_local_path()` helper and use it at every path-accepting IPC entry.
3. **TFA recovery-code wrapper.** `Request::TwoFactorCodeSubmission.value` is a plain `String` that may carry a long-lived recovery phrase — wrap in `SecretString`.
4. **ResilientTransport classifier.** `crates/pcloud-resilience/src/transport.rs` treats `InvalidCertificate` as `Transient` and retries it. Make cert-validation errors terminal.
5. **Wire `MethodRetryPolicy` into `ResilientTransport`.** `upload_create → upload_write → upload_save` currently has no idempotency anchor at the transport layer.
6. **`pcloud-web` authentication.** Web UI has CSRF but no auth; any sibling local process bypasses it. Add bearer-token + per-endpoint capability check.
7. **Mutex-poisoning sweep.** `crates/pcloud-*/src/` — ≈40 `Mutex::lock().expect("poisoned")` on daemon hot paths; replace with graceful degradation or `parking_lot::Mutex` that never poisons.
8. **SAFETY-comment sweep.** 35 `unsafe` blocks missing `// SAFETY:` (clustered in `signals.rs`, `pcloudc/src/main.rs`, `prompt.rs`) — add or refactor.
9. **WinFSP version probe + macFUSE/fuse-t runtime probe** — `crates/pcloud-fs/src/platform/windows.rs` and `platform/macos.rs`; currently load blindly.
10. **FreeBSD rc.d `kldload fuse`** — the script does not pre-load the module.

### Phase 3 — Feature Completion (enterprise parity)

1. **Engine stall detection + Retry-After honoring + global retry budget + idempotency keys** across `pcloud-engine` + `pcloud-resilience`.
2. **NFC / case-insensitive conflict detection** in the conflict resolver (macOS + Windows).
3. **Watcher inotify-overflow rescan** (`notify` integration) — currently silently drops events.
4. **Staging-cache disk budget** — today eviction is lossy and unbounded.
5. **Engine battery/power awareness** — exists for the integrity sweeper only, not the sync loop.
6. **FUSE `access`, `forget`, `rename-flags`, `setattr-mode`, `readlink`, xattr ops.**
7. **FUSE read-ahead/prefetch; `FileHandle::size` population.**
8. **macOS + Windows FUSE SIGTERM/CTRL-C handlers; Windows orphan detection** (currently a stub); **fuse-t `LowlevelOps` layout validation**.
9. **CLI↔IPC matrix closure.** Add CLI subcommands for `Method::Health`, `CryptoSetup`, `CryptoMkdir`, `SyncRootPause/Resume`, `ValueGet/Set/Has`.
10. **CLI unify on clap.** Replace the 4700-line hand-rolled parser with a single clap derive tree that also feeds completions.
11. **SDK examples + feature flags.** Add rustdoc examples for 80+ public helpers; introduce `[features]` to pick TLS provider (`rustls+ring` / `aws-lc-rs` / `native-tls`).
12. **SDK semver hygiene.** Remove `pub use pcloud_proto::Notification` and `pub use upload_session::UploadSessionDriver` from the SDK's public surface.
13. **Typed-ID sweep.** 13 raw `u64`/`String` ID parameters remain despite `pcloud-model::ids` newtypes — systematize.

### Phase 4 — Polish, Docs & Release Readiness

1. **Parity matrix + doc path reconciliation.** Fix the ~60–80 `rust_reference` rows in `C_FEATURE_PARITY_MATRIX.csv` that still cite `crates/pcloud-daemon/src/*_backend.rs`; these modules moved to `crates/pcloud-backends/src/`. Propagate the fix into `ARCHITECTURE.md`, `API-REFERENCE.md`, `SECURITY.md`, and `CLAUDE.md`.
2. **CLAUDE.md crypto correction.** Remove the "still missing" claims for `change_crypto_pass`/`send_change_user_private`/`priv_key_flags`/`psync_send_publink` — all are implemented.
3. **Dashboards.** `dashboards/` directory is empty; ship Grafana JSON + Prom alert rules matched to the `pcloud-observability` counter inventory.
4. **mdbook.** Run `cd docs/book && mdbook build` in CI and fail on broken links.
5. **SDK rustdoc sweep.** `cargo doc --workspace --no-deps` should be warning-free; two SDK rustdoc examples reference files that don't exist.
6. **Proptest coverage sweep.** `crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:15` enumerates ~30 of 45 `Method` variants; close the gap and use a compile-time exhaustiveness check (remove the `_ => 0` bypass).
7. **Live-e2e breadth.** Add suites for account utilities, transfers (currently 1 test for 4 variants), public links (1 test for 12 RPCs), backup/device (zero coverage).
8. **Fuzz targets.** Add `cargo fuzz` targets for the crypto sector decoder, HTTP response parser, path validator.
9. **Test suite bootstrap.** Add `tests/` directories for `pcloud-auth`, `pcloud-config`, `pcloud-engine`, `pcloud-idp`, `pcloud-kms`, `pcloud-store`.
10. **`#[ignore]`d Windows IPC tests.** Un-ignore `platform_ipc_crossplat.rs:148,194` once the WinFSP backend ships — they currently contradict the Windows tier-1 claim.
11. **`sync_loop_live.rs:36`** — add `#[ignore]` guard; silently passes without assertions when unconfigured.

---

## Detailed Findings

The following sections contain the full per-dimension findings with file:line references. Each was produced by an independent Opus specialist against the audit prompt. Section ordering follows the dimension numbering in `pcloud_rev.md`.


