# pcloud-rs Enterprise Deployment Readiness Audit

## Mission

You are a senior Rust systems engineer and cloud-sync protocol specialist conducting a comprehensive enterprise readiness audit of the **pcloud-rs** project — a clean-room Rust rewrite of the pCloud client (daemon, CLI, SDK, FUSE mounted drive, crypto, shares, public links, backup/device, sync engine). Your audit must be thorough, unforgiving, and actionable. Every finding must include severity, affected files/lines, and a concrete remediation path.

**Scope**: Everything under `/home/ezechiel203/Projects/FORKS/pcloud-rs/` **INCLUDING** the `crates/` directory (that is where the primary code lives). Specifically:

- `crates/**/src/` — all workspace crates (daemon, CLI, SDK, fs, crypto, ipc, proto, engine, cache, store, config, auth, resilience, secret, model, observability, session, web, policy, etc.)
- `crates/**/tests/` — unit, integration, and live-e2e test suites
- `crates/pcloud-live-e2e/` — real-account verification suite
- `docs/book/` and root-level `.md` files (parity matrix, review, CLAUDE.md, plans, status)
- `RUST-PLANS/` — execution plans (especially `30-C-FEATURE-PARITY-EXECUTION-PLAN.md`)
- `C_FEATURE_PARITY_MATRIX.csv` and `C_FEATURE_PARITY_REVIEW.md` — the parity source of truth
- `STATUS.md` — authoritative counts source
- `REJECTED-RATIONALES-*.md` — per-row rejection rationale
- `Cargo.toml` (workspace root) and per-crate manifests, `rust-toolchain.toml` if present
- Top-level `README.md`, `CONTRIBUTING.md`, `LICENSE`, release artifacts

**Explicitly excluded** (do NOT audit):
- Build artifacts (`target/`, `*.rmeta`, `*.o`)
- Vendored dependency sources (`vendor/`)
- `.beads/` state
- Auto-generated tracker output

**Historical note**: citations to `pclsync/*.c`, `main.cpp`, `control_tools.cpp`, `pclsync_lib.cpp` in parity documents refer to the upstream C tree at `github.com/pcloudcom/pcloud-rs`; the C sources have been removed from this fork. Treat those references as parity provenance only.

## Strategic Goals

pcloud-rs must become a production-grade, enterprise-deployable pCloud client that:

1. **Achieves verified C-to-Rust capability parity** per `C_FEATURE_PARITY_MATRIX.csv`. Every retained row must be `Implemented` and live-verified where relevant; every `Partial` row must have a tracking bead and concrete remediation; every `Rejected` row must have documented security/design rationale in `REJECTED-RATIONALES-*.md`. Close `bd-1du.10` only when all of this is true.
2. **Delivers a working mounted-drive on all tier-1 platforms** — Linux (libfuse3 via `fuser`), FreeBSD (libfuse2 via `fuser`), macOS (fuse-t + macFUSE via direct FFI), Windows (WinFSP via FFI). Full read path, write path with crash-safe writeback journal, mount/unmount, orphan detection, signal-handled teardown.
3. **Enforces a stricter security posture than the legacy C client** — no plaintext password persistence, auth tokens vaulted with owner-only 0600 files and 0700 parent dirs, owner-only local IPC, TLS-only in production (no downgrade), `SecretString`/`SecretBytes` wrappers throughout, redacted Debug impls, zeroize on Drop, no secrets in logs or error messages.
4. **Passes all live verification gates against a real pCloud account** — auth, TFA, crypto lock/unlock, public-link CRUD, share accept/decline, backup create/stop, upload/download, sync-add with backend validation. Results committed to the repo under `crates/pcloud-live-e2e/`.
5. **Provides honest, stable, production-ready packaging** — systemd unit on Linux, launchd plist on macOS, Windows service on Windows; validated configuration schema; Prometheus metrics via `pcloud-observability`; structured logging with log rotation; upgrade path that preserves session/vault/journal state.
6. **Maintains documentation that matches reality** — no "full parity" / "production ready" claims ahead of matrix evidence; deployment guide that a senior sysadmin can follow end-to-end; SDK API reference; compliance matrix linked from `STATUS.md`.

## Audit Dimensions

For each dimension produce a dedicated section with findings organized by severity: **CRITICAL** (blocks deployment / data loss / security vulnerability), **HIGH** (significant gap / compliance risk), **MEDIUM** (quality issue / missing feature), **LOW** (enhancement / polish).

---

### 1. C-to-Rust Feature Parity & API Coverage

Audit the parity truth files (`C_FEATURE_PARITY_MATRIX.csv`, `C_FEATURE_PARITY_REVIEW.md`, `STATUS.md`, `REJECTED-RATIONALES-*.md`) against actual crate implementations:

- **Auth** (`crates/pcloud-proto/src/auth_api.rs`, `crates/pcloud-daemon/src/auth_backend.rs`): password auth, token auth, TFA code, recovery code, TFA SMS resend, TFA device-notification resend, `userinfo`, `verify_email`, `verify_email_restricted`, `lost_password`, `change_password`, `register`, `get_promo`, `get_api_servers`, `set_language`, `set_api_server`. Verify every row is wired through the daemon and reachable from CLI/SDK.
- **Transfers** (`crates/pcloud-proto/src/transfer_api.rs`, `crates/pcloud-daemon/src/transfer_backend.rs`, `crates/pcloud-sdk/src/lib.rs`): `getfilelink`, `upload_create`, `upload_write`, `upload_save`, signed-HTTP download execution, upload-byte execution, `upload_data`, `upload_data_as`, `upload_file`, `upload_file_as`. Verify chunked upload, resumability, and idempotency on retry.
- **Public links** (`crates/pcloud-proto/src/public_links_api.rs`, `crates/pcloud-daemon/src/public_link_backend.rs`): file/folder pub-link create/list/show/delete, `changepublink` expire/password/upload policy, upload-link CRUD, tree-link, upload-access, bookmarks/pins, screenshot, folder up/down link. Spot-check against C symbols (`psync_change_publink_*`, `psync_publink_*`).
- **Shares, business, teams** (`crates/pcloud-proto/src/shares_api.rs`, `crates/pcloud-daemon/src/shares_backend.rs`): share request list, share list, add/remove/modify, accept/decline/cancel, contacts, my-teams, account team-share, crypto-aware variants.
- **Crypto** (`crates/pcloud-crypto/`, `crates/pcloud-daemon/src/runtime.rs`): setup/start/stop/reset, lock/unlock, crypto-folder create, AES-256-GCM sector encryption, deterministic metadata filename encoding, password rotation, fingerprint verification, `change_crypto_pass` family, `send_change_user_private`, `priv_key_flags`, crypto-aware share/team-share temppass flow. Flag every missing item.
- **Sync root management** (`crates/pcloud-daemon/src/sync_backend.rs`, `crates/pcloud-engine/src/lib.rs`): persisted add/list/remove, `sync-add` backend validation, path canonicalization, duplicate/nested-root rejection, queued-work eviction on remove, suggestions, syncability classification.
- **Sync engine runtime** (`crates/pcloud-engine/`): queue model, conflict resolution, stall detection, bandwidth scheduling, pause/resume, back-pressure. Compare depth against C `psync_syncer.c` responsibilities.
- **Backup / device / account utility** (`crates/pcloud-proto/src/backup_api.rs`, `crates/pcloud-daemon/src/backup_backend.rs`, `crates/pcloud-daemon/src/account_backend.rs`): backup create/delete, stop device, delete backup-device local cleanup, `psync_send_publink`. Note any ghost surfaces (C declarations that never shipped in the active C client) and confirm they're marked `Rejected` with rationale.
- **CLI coverage** (`crates/pcloud-cli/src/commands.rs`, `crates/pcloud-cli/src/app.rs`): every C `ctrl_tools` command should be present or explicitly rejected. Verify argument shapes match the daemon IPC `Request` enum (`crates/pcloud-ipc/src/lib.rs`).
- **SDK breadth** (`crates/pcloud-sdk/src/lib.rs`, `crates/pcloud-sdk/examples/`): public API completeness, doc coverage, examples that compile. Flag `pub` items without `#[doc]` on the public surface.

For each audited row cite file:line. Anything `Partial` without a linked bead = HIGH. Anything claimed `Implemented` but not reachable from a live caller = HIGH. Anything `Rejected` without a rationale in `REJECTED-RATIONALES-*.md` = MEDIUM.

---

### 2. Security Audit

Review security-critical code line-by-line across the workspace. Secret wrappers live in `crates/pcloud-secret/src/secret_string.rs` and `crates/pcloud-secret/src/secret_bytes.rs`.

- **Secret discipline**:
  - Every field or local that holds a password, auth token, refresh token, session key, crypto key, TFA code, or recovery code MUST use `SecretString` / `SecretBytes`. Grep for `password`, `token`, `api_key`, `secret` in `struct` / `fn` signatures; any `String` or `Vec<u8>` at those sites = HIGH.
  - Verify `Debug` impls redact. Spot-check `Request::PasswordSubmission`, `Request::AccountChangePassword`, etc.
  - Verify zeroize on Drop is not optimized away (check `zeroize::Zeroize` derives, `volatile_write`, or equivalent).
- **Auth vault** (`crates/pcloud-daemon/src/auth_vault.rs`):
  - Durable auth persistence must be opt-in.
  - Vault file mode `0600`, parent dir `0700`, ownership validated (`std::os::unix::fs::MetadataExt`).
  - Atomic write (tmp + rename). No mid-write crash window.
  - No plaintext password persistence (confirm this is intentionally NOT mirrored from C).
- **Local IPC** (`crates/pcloud-ipc/`, `crates/pcloud-daemon/src/` socket bring-up):
  - Socket mode 0600, owner-only.
  - Peer-credential check on every accepted connection (SO_PEERCRED on Linux, LOCAL_PEERCRED on *BSD/macOS).
  - Message length caps, framing sanity, version negotiation.
  - Audit-persistence failures surface (don't silently swallow).
  - Slow/malformed-client isolation (per-connection timeout, byte budget).
- **Transport policy**:
  - Production config MUST reject `http://` endpoints (TLS-only).
  - API-server override must validate hostname + cert.
  - No debug-only bypass reachable from `--release`.
- **Downgrade & replay**:
  - TFA flow: no path where TFA is skipped when enabled server-side.
  - Auth token refresh: no window where an expired token is accepted.
  - Re-auth after network partition.
- **Crypto subsystem** (`crates/pcloud-crypto/`):
  - AES-256-GCM nonce generation: verify every `encrypt` uses a fresh nonce (cryptographic RNG, never reused per key).
  - Key derivation: PBKDF2/Argon2 iteration counts match or exceed the C client.
  - Sector-level encryption: verify file-offset-based nonce/tweak scheme is collision-free.
  - Metadata filename encoding: deterministic, collision-resistant.
  - `lock`: zeroizes all in-memory keys, invalidates sessions.
  - `unlock`: constant-time password comparison, rate-limited.
- **Memory safety**:
  - Every `unsafe` block: `// SAFETY:` comment present and the invariant actually holds.
  - Audit FFI surfaces (`pcloud-fs/src/platform/macos_ffi.rs`, `platform/winfsp_ffi.rs`, `platform/bsd.rs`, `platform/linux.rs`) — buffer-length checks before `copy_from_slice`, CString round-trips, pointer lifetime.
  - No `unchecked_*`, `transmute` without justification.
- **Input validation**:
  - Every path-accepting CLI/SDK/IPC op: reject `..`, absolute paths outside sync root, NUL bytes, symlink escapes.
  - Unicode normalization: NFC vs NFD collision on macOS.
  - Max path length enforcement per-OS.
  - JSON payload size caps on API responses.
- **Denial of service**:
  - Connection limits (global + per-source).
  - Request rate limits.
  - Upload/download chunk caps; memory-map vs bounded-buffer.
  - Compression bomb protection on API response decompression.
  - Resource limits on sync-root queue depth, watcher backlog, journal size.
- **Logging**:
  - `grep -rn "password\|token\|secret\|priv_key" crates/**/src/` inside `log::info!`/`warn!`/`error!`/`debug!` macros: any hit that isn't redaction = CRITICAL.
  - Error messages returned to user/daemon must not include secrets.

---

### 3. Crypto Subsystem

Audit `crates/pcloud-crypto/` in detail:

- **Algorithm fidelity**: sector cipher matches the C `pcryptofolder.c` layout so cross-client files round-trip.
- **Key schedule**: master → per-folder → per-file → per-sector derivation chain; verify test vectors against the C implementation.
- **Fingerprints & reset**: key-check packets, reset flow, recovery.
- **Rotation**: `change_crypto_pass` family — does it re-encrypt all metadata, or does it keep a key-encryption-key indirection?
- **Team-share temppass** (`crates/pcloud-crypto/src/share_temppass.rs`): wrap/unwrap flow, expiry, revocation.
- **Zeroization**: every key material buffer is `SecretBytes` or a type that zeroes on drop.
- **Constant-time comparisons**: password/hash compare uses `subtle::ConstantTimeEq` or equivalent.
- **Test vectors**: round-trip tests against known ciphertext from the C client (if any) — CRITICAL if missing.
- **Missing pieces** already called out in `CLAUDE.md`: `change_crypto_pass` full family, `send_change_user_private`, `priv_key_flags`. Confirm status.

---

### 4. Sync Engine & Runtime

Audit `crates/pcloud-engine/` and `crates/pcloud-daemon/src/runtime.rs`:

- **Queue model**: priority, fairness, starvation resistance.
- **State persistence** (`crates/pcloud-store/`): SQLite schema, migration path, transaction safety, crash-consistency.
- **Conflict resolution**: simultaneous edits, local-vs-remote conflict, filename casing on case-insensitive filesystems.
- **Watcher** (`notify` crate integration): debouncing, dropped events on overflow, cross-platform semantics (FSEvents / inotify / USN journal).
- **Idempotency**: every upload/download operation must be safely retryable.
- **Back-pressure**: memory/disk budget enforcement, flow control to API.
- **Rate limiting & retry** (`crates/pcloud-resilience/`): exponential backoff, jitter, retry budget, circuit breaker.
- **Integrity sweeper** (if present): periodic consistency scan, quarantine policy for divergent state.
- **Power/battery awareness**: pause on battery, resume on AC (where platform exposes the signal).

---

### 5. Mounted-drive / FUSE Parity (`pcloud-fs`)

This is the largest open parity epic (`bd-1du.4`). Audit `crates/pcloud-fs/`:

- **Cross-platform architecture**:
  - `src/platform/mod.rs` — platform abstraction (`PlatformMount` trait).
  - `src/platform/linux.rs` — libfuse3 via `fuser` + `fusermount3` + `umount2(MNT_DETACH)` escalation + `/proc/self/mountinfo` orphan detection.
  - `src/platform/bsd.rs` — FreeBSD libfuse2 via `fuser` + `unmount(MNT_FORCE)` + `getmntinfo(3)` orphan detection. NetBSD/OpenBSD: validation-only tier 3.
  - `src/platform/macos.rs` + `src/platform/macos_ffi.rs` — fuse-t (default) + macFUSE (opt-in via `PCLOUD_MACOS_FUSE_BACKEND`) via direct C FFI.
  - `src/platform/windows.rs` + `src/platform/winfsp_ffi.rs` — WinFSP via hand-rolled FFI.
  - `src/platform/fuser_shim.rs` — shared `fuser::Filesystem` shim for Linux + FreeBSD.
- **Core ops**: lookup, getattr, readdir, open, read, release, create, write, flush, fsync, setattr, unlink, rename, mkdir, rmdir. For each, verify the adapter is wired through `FuseAdapter` dyn or generic path.
- **Write path & journal** (`src/write_path.rs`, `src/journal.rs`): staging blob, chunked flush threshold, journal replay after simulated crash.
- **Read path & cache** (`src/backend.rs`, page cache if present): latency budget, prefetch.
- **Mount handle RAII**: `MountHandle` + per-OS inner option; unmount on drop, settle window, escalation.
- **Signal handling**: process-wide SIGTERM/SIGINT trampoline that unmounts active mounts before re-raising.
- **Orphan detection** (`src/mount_orphan.rs`): detect leftover pcloud FUSE mounts on startup, reclaim.
- **Policy** (`src/mount_service.rs`): `MountOptions` validation, `allow_other` rejection, NoDev/NoSuid/DefaultPermissions hardening.
- **Benches** (`benches/page_cache.rs`, `benches/chunked_flush.rs`): read hit/miss, chunk flush throughput.
- **Integration tests**: real-mount smoke test per-platform, readdir/read/write round-trip, unmount cleanliness.
- **Known gaps**: check `bd-1du.4` body for the explicit remaining-work list and verify each item has tracker coverage.

---

### 6. Transport (HTTP API) & Network Resilience

Audit `crates/pcloud-proto/`, `crates/pcloud-resilience/`, and any HTTP-client composition:

- **TLS enforcement**: production profile rejects `http://` (see `pcloud-config`).
- **Cert validation**: no `danger_accept_invalid_certs` in production paths.
- **Timeouts**: connect / read / write / total; per-op budgets distinct from sync-engine retry budgets.
- **Retry policy**: exponential backoff + jitter; retry-after header respected; retry-budget cap.
- **Idempotency keys**: upload_create → upload_write → upload_save must round-trip safely on retry.
- **WebSocket / diff stream**: if present, verify reconnect-with-resume semantics.
- **API-server steering**: `set_api_server` honored; failover behavior; sticky selection across restarts.
- **Observability**: per-endpoint latency/error histogram exported via `pcloud-observability`.

---

### 7. IPC & Daemon

Audit `crates/pcloud-ipc/`, `crates/pcloud-daemon/src/`:

- **Wire format**: length-prefixed framing, message version negotiation, forward/backward compatibility story.
- **Serialization safety**: Serde roundtrip fuzz (proptest suites in `crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs`) — verify coverage is complete.
- **Auth**: peer credentials verified; no blind trust of connecting client.
- **Authorization**: per-request capability checks (e.g., `ShutdownDaemon` requires elevated permission).
- **Runtime directory hygiene**: `${XDG_RUNTIME_DIR}/pcloud-daemon/` ownership + mode + cleanup on exit.
- **Graceful shutdown**: drain in-flight, persist state, release mounts, close sockets. See `tests/graceful_drain.rs`.
- **Crash recovery**: on restart, re-adopt orphaned FUSE mounts, re-hydrate sync state, resume uploads from journal.
- **Stress**: `crates/pcloud-ipc/tests/stress_concurrent_clients.rs` — verify coverage matches expected load.
- **Web / management surface** (`crates/pcloud-web/`): bind address, auth, TLS, capability scoping.

---

### 8. CLI & SDK Surface

Audit `crates/pcloud-cli/` and `crates/pcloud-sdk/`:

- **CLI**:
  - Argument parser (clap) — every subcommand's help matches the actual behavior.
  - Error exit codes: consistent, documented.
  - No secrets on stdout or in shell history (mask interactive prompts).
  - Shell-completion generation (bash, zsh, fish, PowerShell) — present and current.
  - `pcloudc --version` reports workspace version + git SHA.
- **SDK**:
  - Public API surface is semver-disciplined (no `pub use` of internal types that would bind the caller to private crates).
  - Every public fn has a doc comment with an example (if not trivial).
  - `crates/pcloud-sdk/examples/` compiles with `cargo build --examples`.
  - SDK tests cover the happy path for each helper.
  - Feature flags (`default-features`, `tls-rustls` vs `tls-native`, etc.) — combinations all compile.

---

### 9. Code Quality & Robustness

- **Unwrap audit**: inventory every `.unwrap()` / `.expect()` in non-test code under `crates/**/src/`. For each: can it panic in production? Flag all that can = HIGH (daemon) or CRITICAL (IPC handler reachable from untrusted client).
- **TODO / STUB / FIXME audit**: inventory every marker. Every TODO without a bead ID = MEDIUM (see task #6 precedent in CLAUDE.md history). Use grep `TODO\|FIXME\|STUB\|XXX\|HACK\|todo!\|unimplemented!\|panic!`.
- **`unsafe` audit**: list every `unsafe` block with file:line and a summary of the safety invariant. Flag any missing `// SAFETY:` comment as MEDIUM.
- **Error propagation**: `?` used consistently; no `.ok()` silently dropping errors where recovery is meaningful.
- **Logging discipline**: structured logging via `log` / `tracing`; sensitive values never logged; correct levels (no production `info!` spam, no `error!` for expected network blips).
- **Panic paths**: any `panic!`, `unreachable!`, `assert!` reachable from a user request in the daemon = HIGH.
- **Resource leaks**: `Drop` impls release file descriptors, sockets, mount handles; audit with a spot-check of `impl Drop` sites.
- **Dead code**: `cargo +stable build --all-features --all-targets` with `-W dead_code`; flag unused pub items, unreachable match arms.
- **Type safety**: newtypes for `AccountId`, `FolderId`, `FileId`, `SyncRootId`, session IDs — not raw `u64` or `String`. Flag confused-unit bugs.
- **Configuration validation**: all config parameters validated at load time (`crates/pcloud-config/`) with typed errors, not late-bound panics.
- **`cargo fmt --all --check`** must be clean.
- **`cargo clippy --workspace --all-targets -- -D warnings`** must be clean.
- **`cargo deny check`** (if configured) must be clean — no banned licenses, no advisories.
- **MSRV**: verify the documented MSRV compiles.

---

### 10. Testing & QA

- **Coverage**: estimate per-crate coverage (or run `cargo llvm-cov` if configured). Flag critical untested paths: IPC dispatch, auth vault, crypto lock/unlock, FUSE write path, sync engine conflict resolution.
- **Live verification**: `crates/pcloud-live-e2e/` — which flows are actually exercised against a real account? Auth, TFA, crypto, public links, shares, transfers. Missing suites that are retained parity rows = HIGH.
- **Proptest**: coverage for IPC roundtrip, config parser, path validation. Flag obvious gaps.
- **Fuzzing**: any `cargo fuzz` targets? Highest-value: IPC frame parser, HTTP response parser, crypto sector decoder, JSON proto response parser.
- **Benchmarks**: `cargo bench` targets — at least page cache, chunked flush, IPC throughput, crypto sector ops.
- **Cross-platform CI**: does CI exercise Linux + macOS + FreeBSD + Windows? Tier-1 claims require tier-1 CI. If a platform is claimed tier-1 without CI, that = HIGH.
- **Live E2E flakiness**: any skipped or `#[ignore]`-gated tests that should be reinstated? Any network-flakiness-tolerant retry that masks real regressions?
- **Test hygiene**: tests that always pass regardless of correctness (no assertions, empty match arms), hardcoded values that should be dynamic, race conditions in async tests. Spot-check at least 10 tests.

---

### 11. Deployment & Operations

Audit packaging, service integration, and operations:

- **Linux**:
  - systemd unit for `pcloud-daemon` — `User=`, `Group=`, `ProtectSystem=`, `ReadWritePaths=`, `MemoryMax=`, `RestartSec=`, `WatchdogSec=` all set.
  - Log rotation (systemd journal vs file + logrotate).
  - SELinux / AppArmor profile shipped?
  - `.deb` / `.rpm` packaging rules or an equivalent build pipeline.
- **macOS**:
  - launchd plist (`com.pcloud.daemon.plist`) with correct `KeepAlive`, `RunAtLoad`, `ExitTimeOut`.
  - Code signing + notarization pipeline.
  - fuse-t / macFUSE dependency detection (`install_hint` in `platform/macos.rs`).
- **Windows**:
  - Windows service wrapper (SCM integration).
  - WinFSP installer detection and helpful error when absent.
  - Authenticode signing.
- **FreeBSD**:
  - rc.d script.
  - Kernel module preload check (fuse.ko).
- **Configuration**:
  - Schema (`pcloud-config` types) — every field documented, default value sensible, validation at load.
  - Example config shipped and matches the schema.
  - Environment variable overrides documented.
- **Observability**:
  - Prometheus metrics via `pcloud-observability` — which counters/histograms are exported?
  - Dashboards shipped? Alert rules? (Grafana / Prom rules files in `dashboards/`.)
  - Tracing: OpenTelemetry export, sample rate, sensitive-span redaction.
- **Upgrade path**:
  - Schema migrations for SQLite (version table, forward-only migrations).
  - Auth vault format versioning.
  - Journal format versioning.
  - In-place daemon restart preserves active sync / mount state (or documents the disruption).
- **Backup / restore**: documented state that needs to be backed up (vault, SQLite, journal, mount orphan registry).
- **Health checks**: `/healthz` / `/readyz` for container orchestration (if any web surface).
- **Resource limits**: ulimits, cgroup integration, sensible defaults for laptops vs servers.
- **FIPS**: if claimed, verify the crypto backend can switch to a FIPS-validated provider.

---

### 12. Documentation Quality

Audit `docs/`, root `.md` files, and inline rustdoc:

- **Parity docs**:
  - `C_FEATURE_PARITY_MATRIX.csv` matches code reality (spot-check 20 rows).
  - `C_FEATURE_PARITY_REVIEW.md` matches the matrix.
  - `STATUS.md` counts are current and generated from the matrix (not hand-edited).
  - `REJECTED-RATIONALES-*.md` covers every `Rejected` row.
- **Book** (`docs/book/`): every chapter builds with mdbook; claims in the book match code.
- **CLAUDE.md**: no claims of "full parity", "production ready", "drop-in replacement" unless `bd-1du.10` is satisfied with evidence.
- **Deployment guide**: could a senior sysadmin, new to the project, deploy to a production Linux box using only the docs? Run through mentally and flag every gap.
- **Troubleshooting**: common failure modes documented with resolution steps (FUSE mount refused, auth vault locked, sync queue stuck, TLS cert pinning mismatch).
- **SDK API reference**: `cargo doc --workspace --no-deps` warning-free; public items have doc comments; examples compile.
- **Security guide**: documents the secret-handling rules from CLAUDE.md in user-facing form.
- **Release notes**: matches changelog practice; follows semver.
- **README**: quickstart works (clone → build → run → auth → mount).

---

## Output Format

Produce a single, comprehensive report organized as follows:

```
# pcloud-rs Enterprise Readiness Audit Report
## Date: [date]
## Auditor: Claude Agent

## Executive Summary
[2-3 paragraph overview: overall readiness level, top blockers, key strengths]

## Findings by Severity
### CRITICAL [count]
### HIGH [count]
### MEDIUM [count]
### LOW [count]

## Detailed Findings

### 1. C-to-Rust Feature Parity
[findings with file:line references]

### 2. Security
### 3. Crypto Subsystem
### 4. Sync Engine & Runtime
### 5. Mounted-drive / FUSE Parity
### 6. Transport & Network Resilience
### 7. IPC & Daemon
### 8. CLI & SDK Surface
### 9. Code Quality
### 10. Testing
### 11. Deployment & Operations
### 12. Documentation

## Remediation Roadmap
[Prioritized list of work items grouped into phases:
 Phase 1: Critical blockers (must fix before any deployment)
 Phase 2: Security hardening (must fix before production)
 Phase 3: Feature completion (parity epic closure)
 Phase 4: Polish & optimization (production excellence)]

## Appendices
### A. Full .unwrap() / .expect() Inventory (non-test)
### B. Full TODO/STUB/FIXME Inventory with bead coverage
### C. Parity Matrix Gap Table (rows with Partial/Missing status)
### D. unsafe Block Inventory (file:line + safety invariant summary)
### E. Live-E2E Coverage Gap Analysis
### F. Cross-platform CI Coverage Matrix (Linux/macOS/FreeBSD/Windows × auth/transfers/mount/sync/crypto)
```

## Important Instructions

- **Be thorough**: read every file in scope. Do not skip or skim. This is a professional audit.
- **Be specific**: every finding must reference specific files and line numbers.
- **Be actionable**: every finding must include a concrete remediation recommendation and link to the relevant bead (or request one be opened under the `bd-1du` epic) if tracking is missing.
- **Be honest**: if something works well, say so. If a claimed feature is a stub, say that too. CLAUDE.md's "Final Rule" applies — the Rust path must be stricter than C, and claims must match reality.
- **Do NOT modify any files**. This is a read-only audit. The only write is the report itself.
- **Respect the parity truth files**. `C_FEATURE_PARITY_MATRIX.csv` is authoritative; if you disagree with a row status, call it out as a finding rather than silently re-classifying.
- **Cross-reference pCloud HTTP API conventions** when evaluating protocol code.
- **Cross-reference CLAUDE.md** for the security posture rules and the "do not claim parity" discipline.
- **Save the full report** to `/home/ezechiel203/Projects/FORKS/pcloud-rs/AUDIT_REPORT.md` when complete.
- **Time budget**: take as long as needed. Thoroughness is more important than speed.
