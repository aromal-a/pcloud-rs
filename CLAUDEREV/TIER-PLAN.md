# CLAUDEREV Tier-Implementation Plan

Date: 2026-04-30
Source: the four-tier "best in the world" survey response after the
deferred-set campaign closed (`DEFERRED-COMPLETE.md`).
Driver: cron `*/3 * * * *` (every 3 min, session-scoped).
Scope explicitly excluded by the user: **GUI client**, **mobile clients**.

This plan picks up the remaining 22 tier-items. Each fire reads this
file plus `CLAUDEREV/TIER-PROGRESS.md`, picks the next unfinished
in-scope item top-down (T1 → T4), decomposes if scope > 30-min budget,
executes, and updates the progress log.

Items are ordered **(blast-radius × dependency-chain)** ascending —
smaller, contained items first so the loop accumulates wins and
unblocks later items.

---

## Tier 1 — UX / surface area (excl. GUI & mobile)

### T1.1 — Selective sync (per-path include/exclude globs)

- Files: `crates/pcloud-config/src/sync_loop.rs` (add `[sync_root.<id>.exclude_globs]` schema), `crates/pcloud-engine/src/scheduler.rs`, `crates/pcloud-cli/src/commands.rs`.
- Fix: per-sync-root glob exclusion list; engine scheduler skips matches; CLI `pcloudc sync exclude add/remove/list <root> <pattern>`.
- Acceptance: a file matching the pattern stops syncing on next planner pass; unit + live test.

### T1.2 — File-version listing + restore

- Files: `crates/pcloud-proto/src/methods/file.rs` (add `listrevisions` if missing), `crates/pcloud-ipc/src/methods.rs`, daemon dispatch, CLI `pcloudc file revisions <path>` + `pcloudc file restore --rev <revid> <path>`.
- Acceptance: list shows pCloud server-tracked file versions; restore creates a new revision from a prior one.

### T1.3 — Conflict resolution UX

- Files: `crates/pcloud-cli/src/commands.rs`, `crates/pcloud-engine/src/conflicts.rs` (or wherever ConflictEntry surfaces).
- Fix: `pcloudc conflicts list` (already present? confirm) + `pcloudc conflicts resolve --keep-local|--keep-remote|--keep-both <id>` interactive picker.
- Acceptance: each conflict can be cleared from the CLI without daemon-internal poking.

### T1.4 — Bandwidth scheduling (time-of-day + metered network)

- Files: `crates/pcloud-config/src/sync_loop.rs` (`[bandwidth.schedule]` table), `crates/pcloud-resilience::BandwidthPacer`, `crates/pcloud-engine/src/scheduler.rs`.
- Fix: schedule `cron-style` rules + a metered-network detector (NetworkManager dbus on Linux; `MeteredNetwork` API on Win; macOS `nw_path_monitor`).
- Acceptance: pacer cap rises/falls per schedule; metered-network triggers a tighter cap.

### T1.5 — Internationalisation (i18n) of CLI output

- Files: new `crates/pcloud-cli/i18n/` directory; `crates/pcloud-cli/Cargo.toml` (add `fluent` or `gettext-rs`).
- Fix: extract user-facing strings to `.ftl`/`.po`; `LANG` / `LC_MESSAGES` env-driven runtime selection; English baseline; one example translation (e.g. French) to prove the path.
- Acceptance: `LANG=fr_FR.UTF-8 pcloudc status` shows translated output; fallback to English when locale absent.

### T1.6 — Network-drive integration: WebDAV gateway

- Files: new crate `crates/pcloud-webdav/`.
- Fix: thin WebDAV-PROPFIND/GET/PUT proxy in front of the daemon's existing IPC. Listens on a Unix socket or local-only TCP. Lets non-FUSE clients (file managers, Photos.app) browse pCloud.
- Acceptance: `cadaver` connects + lists + uploads; smoke test via `curl -X PROPFIND`.

---

## Tier 2 — Engineering depth

### T2.1 — Differential / block-level sync (rsync-style rolling-hash)

- Files: new `crates/pcloud-rsync/` (rolling-hash + delta-encoder), `crates/pcloud-engine/src/diff_planner.rs`, `crates/pcloud-backends/src/transfer_backend.rs` (chunked PUT path).
- Fix: when local-modified file is large + already on server, compute rolling-hash signature locally, fetch server's signature via `getfilemetadata` extensions, send only modified blocks via `upload_writefromfile` server-side copy + new bytes.
- Acceptance: edit-1-byte-of-1GB-file uploads only the modified block instead of the full file. Bench shows ~3 orders of magnitude transfer reduction.

### T2.2 — Parallel chunked download (multi-range HTTP)

- Files: `crates/pcloud-proto/src/transfer_api.rs` (add multi-range fetch helper), `crates/pcloud-fs/src/read_path.rs` / `fuse_adapter.rs`.
- Fix: split large reads into N parallel HTTP range fetches. Reassemble in order.
- Acceptance: 1 GiB cold-fetch on a 4-thread connection finishes in ~1/4 the time of single-thread.

### T2.3 — Encryption-at-rest for local cache

- Files: `crates/pcloud-cache/src/page_cache_generic.rs` (transparent encrypt-on-write), `crates/pcloud-fs/src/staging.rs`.
- Fix: derive a cache-encryption key from the auth vault (machine-bound); encrypt every page-cache entry + staging buffer at rest.
- Acceptance: an attacker with disk access (but no auth vault) cannot read cached page contents. Add a unit test that asserts on-disk bytes are not the plaintext.

### T2.4 — Per-folder crypto policy

- Files: `crates/pcloud-crypto/src/policy.rs`, `crates/pcloud-store/src/repositories/preferences.rs`.
- Fix: opt-in crypto per remote folder rather than the current account-wide flag. Each folder carries its own KEK in the daemon's session memory.
- Acceptance: user can enable crypto on `/Documents` while keeping `/Photos` plaintext; tests cover the per-folder unlock state machine.

### T2.5 — Plugin sandbox (Wasm runtime for `pcloud-plugin-api`)

- Files: new `crates/pcloud-plugin-host/`, integrates `wasmtime`.
- Fix: third-party plugins compiled to `wasm32-wasi` run with a strict capability allowlist (no fs, no net beyond a typed message bus to the daemon).
- Acceptance: a sample plugin runs sandboxed; an attempted `fs::write` from inside the plugin is denied.

### T2.6 — QUIC / HTTP/3 transport option

- Files: `crates/pcloud-proto/src/transport.rs` (add `Quic` variant), `crates/pcloud-config/src/api.rs` (`api.transport = "tls" | "quic"`).
- Fix: `quinn`-based QUIC alternative to TLS-over-TCP. Honours pCloud's existing TLS cert chain + the connection-pool semantics.
- Acceptance: `cargo run -- --api-transport quic` works against a QUIC-enabled test server; falls back cleanly to TLS when QUIC handshake fails.

### T2.7 — Distributed tracing (OpenTelemetry / W3C traceparent)

- Files: `crates/pcloud-daemon/src/dispatch.rs`, `crates/pcloud-backends/src/*.rs`, `crates/pcloud-proto/src/transport.rs`.
- Fix: `traceparent` is already on `RequestEnvelope`; thread it through every backend RPC and outbound HTTP header. Add an OTLP exporter behind a `--tracing` flag.
- Acceptance: a trace ID propagated from the CLI shows up in Jaeger across daemon → backend → outbound API.

### T2.8 — Multi-account supervisor

- Files: new `crates/pcloud-supervisor/`, refactors `crates/pcloud-daemon/src/bootstrap.rs`.
- Fix: one host process supervises N per-account daemons (each with its own auth vault, store, IPC socket). CLI `pcloudc account add/remove/switch`.
- Acceptance: two accounts running concurrently; each `pcloudc` invocation targets one account by env var or `--account` flag.

---

## Tier 3 — Quality ratchet

### T3.1 — Drive rustdoc warnings 41 → 0

- Files: workspace-wide rustdoc cleanup.
- Fix: tackle the warnings in clusters per-module; replace private-item intra-doc links with code spans; fix dangling refs to renamed items.
- Acceptance: `cargo doc --workspace --no-deps` reports 0 warnings.

### T3.2 — Coverage floor 40 → 60

- Files: `.github/workflows/ci.yml` `coverage` job env.
- Fix: bump `LINE_COVERAGE_FLOOR` after a green run reports actual coverage. Iterate up to 60% in 5-point increments per fire.
- Acceptance: floor at 60%, gate green.

### T3.3 — Workspace-wide unwrap audit

- Files: every `crates/*/src/**/*.rs`.
- Fix: for each `.unwrap()` / `.expect()` outside `#[cfg(test)]`, classify as: (a) Mutex-poisoning recovery (replace with `unwrap_or_else(|p| { log; p.into_inner() })`), (b) genuine panic-on-violation, (c) error-propagatable. Convert (c) to `?` + a typed error variant.
- Acceptance: `unwrap_used = "deny"` clippy lint clean across non-test code.

### T3.4 — Fuzz coverage extension

- Files: `fuzz/fuzz_targets/`. New targets: `transport_frame.rs` (binary protocol parser), `ipc_request.rs` (serde-bincode roundtrip), `public_link_uri.rs` (URL parsing in public-link backend).
- Acceptance: each new fuzzer runs ≥1M iterations in CI's weekly fuzz job without finding a crash.

### T3.5 — Reproducible build on macOS + Windows

- Files: `.github/workflows/release.yml`, `.cargo/config.toml`.
- Fix: port the existing Linux `release-repro` profile + `--remap-path-prefix` + `SOURCE_DATE_EPOCH` discipline to macOS + Windows runners. Diff two-runner builds.
- Acceptance: macOS + Windows CI jobs publish bit-identical binaries across two independent runs.

### T3.6 — Memory profiling (heaptrack / valgrind massif)

- Files: new `tools/memprofile/` directory; new `.github/workflows/memprofile.yml` (weekly).
- Fix: 24-hour daemon run under sustained sync load with `heaptrack`. Analyse top allocators, document baseline.
- Acceptance: published baseline + per-PR alert if RSS regresses ≥10%.

### T3.7 — Cold-start latency profiling

- Files: new `crates/pcloud-daemon/benches/cold_start.rs` (Criterion).
- Fix: bench daemon bootstrap from a cold cache to first-RPC-served. Compare against the C client.
- Acceptance: baseline published; CI gate that flags ≥20% regression.

---

## Tier 4 — Operational / SRE polish

### T4.1 — Prometheus alert rules in tree

- Files: new `deploy/prometheus/alerts.yml`.
- Fix: rules for hit-ratio dip, audit-drop spike, integrity-sweep mismatch, mount-orphan threshold, transport circuit-breaker open. Each rule has a severity + runbook link.
- Acceptance: `promtool check rules` clean; rules cite `OPERATIONS-RUNBOOK.md` playbooks by anchor.

### T4.2 — Disaster recovery drill automation

- Files: `tests/dr_drill/` new directory; CI workflow `dr-drill.yml` (monthly).
- Fix: scripted vault-loss + store-corruption + sync-root-mass-eviction scenarios. Asserts recovery procedures from `OPERATIONS-RUNBOOK.md` actually work.
- Acceptance: monthly green run; failing drill blocks the next release tag.

### T4.3 — Capacity planning docs

- Files: `docs/capacity-planning.md` (new).
- Fix: empirical RAM-per-sync-root, disk-per-cached-page, network-per-active-mount; sizing table by deployment scale (single-user laptop, NAS, fleet).
- Acceptance: doc lands; numbers are reproducible from `tools/memprofile` output (T3.6).

### T4.4 — Server-side dedup awareness in CLI

- Files: `crates/pcloud-cli/src/commands.rs` (extend `pcloudc storage` summary), `crates/pcloud-proto/src/methods/account.rs` (if a server endpoint exposes dedup ratio).
- Fix: surface "physical bytes vs logical bytes" so users see how much pCloud's server-side dedup saves them.
- Acceptance: `pcloudc storage` output shows both numbers; live test confirms dedup ratio matches web UI.

---

## Out-of-scope for this loop (per user instruction)

- **GUI client** (Tauri / iced / egui native shell).
- **Mobile clients** (iOS / Android).

---

## Operating model

Each cron fire:
1. Reads `CLAUDEREV/TIER-PROGRESS.md` to find the next unfinished item.
2. Picks one in-scope item; if scope > 30-min budget, decompose first.
3. Executes the fix.
4. Verifies via `cargo check --workspace --all-targets` + `cargo fmt --all --check` + `cargo deny check` plus the per-item acceptance commands.
5. Updates `CLAUDEREV/TIER-PROGRESS.md`.
6. If everything in this plan is done (T1–T4 complete or each `[OUT-OF-SCOPE-PENDING-USER-RESOURCE]`), call `CronList` → `CronDelete` → write `CLAUDEREV/TIER-COMPLETE.md`, stop.

Verification baseline (must hold across every fire):

- `cargo check --workspace --all-targets` exit 0
- `cargo fmt --all --check` exit 0
- `cargo deny check` clean
- `cargo doc --workspace --no-deps` warning count monotonically non-increasing (current floor: 41 — and T3.1 explicitly drives this floor down)

If a fire would break the baseline, **the fire reverts its own changes**
and logs the regression for analysis on the next fire.
