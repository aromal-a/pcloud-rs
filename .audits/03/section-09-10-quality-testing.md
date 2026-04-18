# Sections 9 & 10: Code Quality & Testing
## Date: 2026-04-17
## Auditor: Claude Opus (Agent 9/10)

Scope: read-only audit of `crates/**/src/`, `crates/**/tests/`, `crates/**/benches/`,
`crates/**/fuzz/`, and `.github/workflows/`. Counts below are produced with a
stripper that removes `#[cfg(..test..)]` mod blocks and `#[test]` free
functions before counting production-path occurrences. Doc comments (`///`)
are dropped for the "prod" numbers so doctests do not inflate them.

## Summary Counts

- `.unwrap()` / `.expect(` total (all files, tests included): **2,622 across 176 files**
- `.unwrap()` / `.expect(` **production** (after stripping `#[cfg(..test..)]`
  mods, `#[test]` fns, and `///` doc examples): **152 across 55 files**
  - Top 5: `pcloud-fs` (28), `pcloud-daemon` (27), `pcloud-sdk` (16),
    `pcloud-crypto` (14), `pcloud-backends` (14).
- `TODO / FIXME / HACK / STUB` (excluding `XXX` which is not used as a marker):
  **36 across 21 files**. Of those, **27 carry an explicit `bd-` tracker
  reference** (`bd-xplat`, `bd-1du.4`, `bd-1du.4.6`, `bd-1du.10`, `bd-fuse`,
  `bd-xplat-bsd`, `bd-xplat-windows`, `H-3`, `P0.3`, `spec §...`). **9 do
  not** (see MEDIUM findings below).
- `unsafe` blocks / fns / impls / traits: **358 total**. **313** carry a
  `// SAFETY:` comment within the eight preceding lines; **45** do not.
  Of those 45, **~26 are trivial `std::env::{set,remove}_var` calls in
  test helpers**, **~10 are benign libc wrappers** (`libc::kill`,
  `libc::statvfs64`, `libc::sigaction`, `libc::setsockopt`,
  `CString::new(literal)`), and **9 are in `pcloud-config/tests/` or
  `pcloud-cli/src/prompt.rs` / `globals.rs` / `commands.rs`** — all
  short, locally obvious, and preceded by a `// Safety:`-equivalent
  prose comment explaining the single-threaded test invariant.
- `impl Drop`: **21 sites** (mount handles, listeners, handles, guards,
  lease holders, span buffers, ticket wrappers, shells).
- `panic!(`, `unreachable!(`: **106 total**; **0 reachable from production
  dispatch** after stripping test modules and test fns. The remaining
  occurrences are test assertions ("expected X, got Y") or `unwrap_or_else`
  inside proptest match arms.
- `todo!(` / `unimplemented!(`: **0 in executable code**. The single hit
  in `crates/pcloud-daemon/src/vault/mod.rs:9` is inside a `//!` module
  doc comment that reassures the reader: *"All four backends are real
  implementations — no `unimplemented!()`"*.
- Sensitive-value log leaks (`(info|warn|error|debug|trace)!\s*\([^)]*(password|secret|token)`):
  **1 hit**, `crates/pcloud-daemon/src/serve.rs:309`, text is
  `log::info!("pcloud-session-refresh: token refreshed successfully");` —
  message mentions the *word* "token" without logging a token value. Safe.

## Findings

### CRITICAL [0]

No CRITICAL findings. No `.unwrap()` / `.expect()` on an IPC-handler
code path reachable from an untrusted client; no `panic!` / `unreachable!`
is reachable from daemon request dispatch; no secret is emitted through
a log macro; no production transport downgrade is permitted.

### HIGH [3]

**HIGH-Q1 — proptest IPC exhaustiveness guard still defeated (not fixed since audit 02)**

`crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:95` still ends
with `_ => 0,` (verified line 95 on 2026-04-17). The enclosing
`must_match_every_method_variant` advertises itself as a compile-time
exhaustiveness guard, yet the wildcard arm plus `#[non_exhaustive]` on
`Method` renders it a no-op.

- `Method` variant count (source): **45** (enum body at `methods.rs:37..`).
- `every_method()` enumerates: **31** (line 94 in the guard; final `_ => 0`
  silently accepts the remaining 14).
- `Request` variant count (source): **81**.
- `arb_request()` strategies: **~24 variants**.

Ratio: ~30% of `Request` variants receive proptest fuzzing; ~69% of
`Method` variants appear in the exhaustiveness guard's explicit arm.
The CSV parity matrix claim that "IPC surface is proptest-verified" is
still not true.

**Fix (unchanged from audit 02):** remove the `_ => 0` arm; extend
`every_method()` and `arb_request()` to every variant; make `Method`
non-`#[non_exhaustive]` for in-crate tests or re-export an enumeration
macro from `pcloud-ipc`.

---

**HIGH-Q2 — `cargo fmt --all --check` is RED on this tree**

`cargo fmt --all --check` exits with status **1** (run 2026-04-17 on a
clean checkout): **59 `Diff in` records** across `pcloud-backends`,
`pcloud-cli`, `pcloud-daemon`, `pcloud-fs`, `pcloud-ipc`,
`pcloud-proto`, and `pcloud-sdk` — long lines with redacted-wrapper
calls (`RedactedProtoString::from(auth_token.expose_secret().to_owned())`)
and a handful of multi-armed match-blocks spilling over the column
limit.

The CI workflow (`ci.yml:26`) runs this exact check with
`-- -D warnings` clippy after it, so either (a) CI is broken, (b) CI has
not run on this branch since the drift was introduced, or (c) CI is
allowed to skip on feature branches. Either way, the "fmt gate" is
currently not enforcing anything on the working tree.

**Fix:** run `cargo fmt --all` and commit. Keep the CI gate and add a
`pre-commit` hook or a branch-protection rule to prevent regression.

---

**HIGH-Q3 — `pcloud-daemon/src/integrity_sweeper_service.rs` has 17
production `Mutex::lock().expect("poisoned")` sites in a long-running
background service**

`integrity_sweeper_service.rs:760, 801, 808, 814, 920, 929, 955, 1015,
1039, 1067, 1163, 1202, 1203, 1206, 1288, 1295, 1344`.

Each is a `self.<mutex>.lock().expect("... poisoned")`. Mutex poisoning
is not a "cannot happen" — any prior panic inside a scope that held the
lock flips it. The sweeper is a long-running scheduler thread, so one
unrelated panic (e.g. inside `audit_sink`, inside `checksum_fetcher`,
inside a glob-matcher) silently leaves the mutex poisoned; the *next*
tick then propagates a second panic that aborts the scheduler thread
and silently degrades the integrity guarantee the sweeper was designed
to provide. The audit-drop-count atomic elsewhere in this file is
explicit evidence that the authors care about silent-failure avoidance;
the poisoned-mutex path contradicts that stance.

**Fix:** replace the 17 `.expect("... poisoned")` calls with
`PoisonError::into_inner()` recovery (preferred for read-only snapshots
like `progress_snapshot`), or with a structured degradation path that
logs once and returns the previous known-good value.

---

### MEDIUM [8]

**MEDIUM-Q4 — `#![allow(dead_code)]` at file scope in three
first-class source files**

`crates/pcloud-cli/src/migrate.rs:43`,
`crates/pcloud-cli/src/main.rs:10`,
`crates/pcloud-fs/src/platform/macos_ffi.rs:22` all carry a
**file-wide** `#![allow(dead_code)]`. The macos_ffi file is a bindings
surface where unused C-ABI constants are expected; the two CLI files
are not. A file-level allow disables clippy's `dead_code` lint for the
entire translation unit and hides genuinely unused items. The sibling
pattern (*item-level* `#[allow(dead_code)]` with a prose justification,
e.g. `pcloud-auth/src/orchestrator.rs:899-908`) is the correct one and
is already used elsewhere in the tree.

**Fix:** replace file-wide allows with item-level allows and a rationale
comment per occurrence.

---

**MEDIUM-Q5 — 9 `TODO` markers without a `bd-` tracker ID**

Of 36 `TODO|FIXME|HACK|STUB` markers, 27 cite a bd tracker or spec
section. The remaining 9 do not:

- `crates/pcloud-sdk/src/upload_session.rs:693` — `// TODO: thread once
  the wire supports ifhash.` (no tracker)
- `crates/pcloud-sdk/src/lib.rs:1351` — doc comment mentions "`TODO(stub)`
  markers" narratively.
- `crates/pcloud-daemon/src/metrics_server.rs:184` — `TODO(P0.3 follow-up):
  wire slo.incr_upload_started()` (P0.3 is a phase label, not a
  persistent bd-id).
- `crates/pcloud-proto/src/transfer_api.rs:414` — `TODO(spec §9.5):
  live-API verification required` (spec-section ref, no bd-id).
- `crates/pcloud-proto/src/resilient_transport.rs:291, 335, 341` — all
  three `TODO(H-3)` (H-3 is not a bd-id).
- `crates/pcloud-proto/src/methods/upload.rs:69, 602` — `TODO(spec §9.3...)`,
  `TODO(spec §9.2): live-API verification` (spec refs, no bd-id).

Per the repo convention (`CLAUDE.md` §"Documentation Discipline"), every
TODO should be paired with a tracker bead. The three `TODO(H-3)` in
`resilient_transport.rs` are the clearest offenders because the linked
work (Prometheus histogram emission) is observable infrastructure that
is shipped half-wired.

**Fix:** open a `bd-XXX` bead for each, and rewrite the marker.

---

**MEDIUM-Q6 — Proptest coverage for the IPC surface is ~30%**

Separate from HIGH-Q1, the breadth of proptest strategies is the
underlying issue. `arb_request()` exercises ~24 variants; the enum
defines 81. A crate-internal exhaustiveness test
(`every_method_variant_round_trips`, line 100) does round-trip
*Plain* method envelopes but does not exercise the parameterized
variants (`SyncRootAdd`, `CreateFilePublicLink`, `CryptoUnlock`,
`CryptoSetup`, etc.). The missing strategies include security-sensitive
destructive operations: `Shutdown`, `Unmount`, `MountForceUnmount`,
`BackupSnapshot`, `BackupDelete`, `CryptoReset`, `AccountChangePassword`,
`AccountRegister`, `SetApiServer`.

**Fix:** at minimum, add strategies for every destructive-or-
sensitive-request variant; aim for 100% `Request` coverage.

---

**MEDIUM-Q7 — `pcloud-fs/src/inode.rs:147` "inode number space exhausted"
panic**

`crates/pcloud-fs/src/inode.rs:147`: `.expect("inode number space
exhausted");`. On a long-running mount this is reachable: after 2^63 =
9.22×10^18 inode allocations the id generator wraps. That threshold is
astronomical in normal use, but: (a) the FUSE layer does not reset the
counter on unmount/remount of the same mount, and (b) a misbehaving
scanner that rapid-allocates then releases inodes without reusing can
pressure the counter faster than anticipated. The file-system-facing
layer is a long-lived daemon process; panicking on inode exhaustion
aborts the mount.

**Fix:** return `libc::ENOSPC` (or the FUSE equivalent) from the open
path; log a single `error!` with context.

---

**MEDIUM-Q8 — `sync_loop_runtime.rs:577` panics on store-open failure**

`sync_loop_runtime.rs:577`: `.expect("failed to open sync loop store
connection");`. This is on the bootstrap path; failure here aborts
the daemon at startup. Startup panic is acceptable in principle but
produces a raw Rust backtrace rather than a structured log line, so
operators debugging a SQLite-permission / disk-full / schema-mismatch
fault get the wrong diagnostic. The rest of daemon bootstrap already
uses structured `anyhow::Error` (`bootstrap_with_config` returns
`Result`).

**Fix:** propagate via `?` and emit a single structured error.

---

**MEDIUM-Q9 — `pcloud-fs/src/platform/macos.rs` and
`platform/linux.rs` `statvfs64` paths lack a `// SAFETY:` comment**

`pcloud-fs/src/platform/linux.rs:169-170, 762-763`,
`platform/fuser_shim.rs:669-670`. The `std::mem::zeroed::<libc::statvfs64>()`
idiom is safe for POD C structs, but the invariant
("the target type contains no `NonNull`/`Box`/enum-discriminant niche")
is not obvious to a reviewer who has not memorized libc. The sibling
FFI sites in the same files **do** carry `// SAFETY:` comments; the
statvfs sites do not.

**Fix:** add a one-liner `// SAFETY: statvfs64 is a POD C struct with
no padding invariants.`

---

**MEDIUM-Q10 — `pcloud-fs/src/platform/macos.rs:1625-1637` eight
`CString::new("literal").expect("literal has no NUL")` sites**

This is a common pattern and technically infallible (string literals
never contain NUL), but eight copies in one function inflate the
expect count and obscure genuine error-path expects in the same file.
A `macro_rules!` helper like `cstr!("pcloud-rs")` (or a single `const`
`&CStr` using `c"pcloud-rs"` edition-2024 literals) would eliminate all
eight.

**Fix:** introduce `c"..."` string literals (edition 2024) or a
`const CSTR_PCLOUD_RS: &CStr = c"pcloud-rs";` block.

---

**MEDIUM-Q11 — No CI job for FreeBSD despite `pcloud-fs/src/platform/bsd.rs`**

`.github/workflows/ci.yml:56-67` contains commented-out FreeBSD
scaffolding and a `TODO(bd-xplat)` note. `pcloud-fs/src/platform/bsd.rs:29`
has a `TODO(bd-xplat-bsd)` for wiring fuser on FreeBSD. The tree still
claims BSD as a target at compile time; the CI matrix still does not
exercise it.

**Fix:** either land the commented-out `cirrus-ci` / cross-platform-
actions FreeBSD job, or mark `platform/bsd.rs` as explicitly
experimental/untested in its module doc.

---

### LOW [6]

**LOW-Q12 — `pcloud-fs/src/write_path.rs` has 23 mutex-lock expects in
mock/test support code inside the production file**

All 23 sites are under `impl MockUploadSink` (lines ~1332-1472). Runs
under `cfg(test)` through mock dependency injection, but the impl is
*not* inside a `#[cfg(test)]` module; it is a `pub(crate)` helper used
both by tests and by fuzz harnesses. The risk is minimal (mutex
poisoning here only matters if a test already failed), but it
contaminates the production-path count.

**Fix:** move the mock impl behind a `#[cfg(any(test, feature =
"test-support"))]` gate, or use `PoisonError::into_inner`.

---

**LOW-Q13 — `sdk/upload_session.rs` has 15 `Mutex::lock().expect("...
poisoned")` sites**

Same pattern as HIGH-Q3 but inside a single upload session lifecycle
object. The session is short-lived; a poisoned mutex on one session
would affect only that upload, not the long-running daemon. Lower
severity than the sweeper.

**Fix:** recover via `PoisonError::into_inner` for read-only snapshots
(`outcome`, `chunked` peeks) and surface a structured `UploadError`
for mutating paths.

---

**LOW-Q14 — `pcloud-daemon/src/audit_verifier_service.rs:454` spawn
expects and `570, 577` poisoned-wake expects**

Same root cause as HIGH-Q3 (scheduler thread). Fewer sites; classifying
lower.

---

**LOW-Q15 — Benchmarks exist for every critical hot path**

At audit 02 this was flagged as a gap. Present as of 2026-04-17:
`crates/pcloud-crypto/benches/aead_sector.rs`,
`crates/pcloud-fs/benches/chunked_flush.rs`,
`crates/pcloud-fs/benches/page_cache.rs`,
`crates/pcloud-ipc/benches/ipc_codec.rs`,
`crates/pcloud-daemon/benches/dispatch_end_to_end.rs`,
`crates/pcloud-daemon/benches/sync_root_canonicalize.rs`,
`crates/pcloud-engine/benches/engine.rs`,
`crates/pcloud-proto/benches/proto_dispatch.rs`,
`crates/pcloud-sdk/benches/upload_session.rs`,
`crates/pcloud-secret/benches/secret_ct_eq.rs`,
`crates/pcloud-store/benches/store_kv.rs`.
All of the audit-02 required benches are now present. No new finding;
noting for record.

---

**LOW-Q16 — No benchmark for `pcloud-fs` write-journal replay**

Given the crash-recovery focus of the mount path (`bd-1du.4`), the
write-journal replay loop (`write_journal.rs:275-305`) lacks a
throughput benchmark. The adjacent `chunked_flush.rs` bench covers the
flush side but not replay.

**Fix:** add `crates/pcloud-fs/benches/write_journal_replay.rs`.

---

**LOW-Q17 — No fuzz target for daemon dispatch / IPC envelope mutation**

`crates/pcloud-proto/fuzz/fuzz_targets/` has 7 targets and
`crates/pcloud-ipc/fuzz/fuzz_targets/fuzz_ipc_frame.rs` covers the frame
codec. Nothing fuzzes the `dispatch.rs` decode-then-execute pipeline
with an attacker-controlled envelope body. Given that IPC is
authenticated by UID only (per audit 07, `Shutdown`/`Unmount` etc.),
an attacker with the owner UID can feed any mutated body.

**Fix:** add `fuzz_dispatch_envelope.rs` under `pcloud-daemon/fuzz/`.

---

## Section 9: Code Quality — Detailed

### Top 20 production `.unwrap()` / `.expect(` sites (by count)

| Count | File | Classification |
|-------|------|----------------|
| 17 | `pcloud-daemon/src/integrity_sweeper_service.rs` | HIGH-Q3 |
| 15 | `pcloud-sdk/src/upload_session.rs` | LOW-Q13 |
| 10 | `pcloud-fs/src/backend.rs` | test-support mocks inside prod file |
|  8 | `pcloud-fs/src/platform/macos.rs` | 8× `CString::new(literal)` — MEDIUM-Q10 |
|  7 | `pcloud-backends/src/mock.rs` | mock recorder mutex poisoned |
|  4 | `pcloud-resilience/src/pacing.rs` | mutex poisoned |
|  4 | `pcloud-idp/src/jwks.rs` | IDP sync primitives |
|  4 | `pcloud-fs/src/inode.rs` | MEDIUM-Q7 — includes exhaustion panic |
|  4 | `pcloud-fleet/src/lib.rs` | fleet agent mutex poisoned |
|  4 | `pcloud-daemon/src/mount_runtime.rs` | `shim/adapter already consumed` (state-machine invariant) |
|  3 | `pcloud-resilience/src/rate_limit.rs` | rate-limiter mutex |
|  3 | `pcloud-plugin-api/src/lib.rs` | plugin boundary |
|  3 | `pcloud-kms/src/lib.rs` | KMS invariants |
|  3 | `pcloud-fs/src/write_journal.rs` | `TryInto<[u8;4]>` — infallible |
|  3 | `pcloud-fs/src/mount_service.rs` | mount supervisor mutex |
|  3 | `pcloud-daemon/src/audit_verifier_service.rs` | LOW-Q14 |
|  3 | `pcloud-crypto/src/lib.rs` | spawn-time invariants |
|  3 | `pcloud-crypto/src/keys.rs` | key-import slice sizes (infallible) |
|  3 | `pcloud-config/src/integrity_sweeper.rs` | config invariants |
|  3 | `pcloud-backends/src/transfer_backend.rs` | mock/test sink |

### `unsafe` block audit

**Total: 358. Missing SAFETY comment: 45 (13%).**

The 45 missing-SAFETY sites fall into three buckets:

1. **`std::env::{set,remove}_var` in tests (~27)** —
   `pcloud-cli/src/globals.rs`, `pcloud-cli/src/commands.rs`,
   `pcloud-config/tests/config_validation.rs`. These are `unsafe` in
   2024 edition because env-var mutation is not thread-safe; the
   tests run single-threaded and this is idiomatic. Fix: add a single
   `// SAFETY: single-threaded test process.` comment per site.
2. **libc syscall wrappers (`sigaction`, `setsockopt`, `statvfs64`,
   `kill`, `zeroed`) (~13)** — `pcloud-daemon/src/signals.rs`,
   `pcloud-ipc/src/transport.rs`, `pcloud-fs/src/platform/linux.rs`,
   `pcloud-fs/src/platform/fuser_shim.rs`, `pcloud-cli/src/main.rs`,
   `pcloud-cli/src/prompt.rs`. These follow established idioms; fix
   as MEDIUM-Q9 (annotate the `statvfs64` sites).
3. **FFI adapter fn / zeroed POD struct (~5)** —
   `pcloud-fs/src/platform/windows.rs:351` (`unsafe fn adapter_from_fs`),
   `pcloud-daemon/src/metrics_server.rs:245` (test env-var mutation),
   `pcloud-daemon/src/mount_runtime.rs:1183` (test context),
   `pcloud-fs/src/platform/macos.rs:235` (macOS SIGTERM trampoline
   stub — already tagged `TODO(bd-1du.4)`).

No CRITICAL `unsafe` findings. Recommended to annotate all 45 in a
dedicated follow-up sweep.

### `Drop` impl discipline

21 `Drop` sites, covering:

- mount handles (`MountHandle`, `MountControl`)
- listener sockets (`BoundIpcServer`, `WindowsListener`, `WindowsStream`)
- mock server handles (`MockHandle`)
- shared memory segments (`ShmSegment`)
- auth/refresh tickets (`RefreshTicket`)
- lease holders (`LeaseHolder`)
- shell handles (`IntegritySweeperShell`, `AuditVerifierShell`,
  `SyncLoopHandle`, `P2pShell`)
- span buffers / exporter handles (`TracingHandle`, `ExporterHandle`)
- DPAPI guard (`LocalFreeGuard`, Windows)
- security-descriptor guards (`SecurityDescriptor`, `HandleGuard`)
- in-flight request guard (`InFlightGuard`)
- test-only `TempDir` / `Restore`.

Every mount handle, listener, and FFI guard has a `Drop`. No obvious
resource-leak surfaces found.

### Config validation

`crates/pcloud-config/src/schema.rs` and `loader.rs` validate at load
time (23 test functions over 30 external integration tests). `cargo
fmt` drift does affect `loader.rs` but no validation bypass was
observed. No finding.

### Logging discipline

Grep of `(info|warn|error|debug|trace)!\s*\([^)]*(password|secret|token)`
returned **1 hit**, at `pcloud-daemon/src/serve.rs:309`, which logs the
textual word *"token"* ("token refreshed successfully") — not a token
value. The `SecretString`/`SecretBytes` types redact `Debug` and
`Display` impls; no leaks observed.

### Newtype discipline

`pcloud-model/src/ids.rs` defines `UserId`, `SyncId`, `FileId`,
`FolderId`, `LinkId`, `AuditId`, `TransferId` via a `define_id!` macro
with `const` constructors, `Ord`, `Hash`, `Serialize`, `Deserialize`.
Used through the workspace; no bare `u64` IDs observed on the request
API surface.

### `panic! / unreachable! / todo! / unimplemented!`

0 `todo!(` / `unimplemented!(` in executable code. All `panic!` uses
are in `#[test]` or `#[cfg(test)]` mod blocks with "expected X, got Y"
assertion patterns. Zero reachable from daemon dispatch.

### `cargo fmt`

Fails (exit 1): **59 diffs** on 2026-04-17. See HIGH-Q2.

### Dead code

3 file-level `#![allow(dead_code)]` (see MEDIUM-Q4). 24 item-level
allows with prose justifications — acceptable pattern.

---

## Section 10: Testing & QA — Detailed

### Per-crate coverage inventory

| crate | src loc | tests loc | bench loc | fuzz loc | inline #[test] | external #[test] |
|-------|--------:|----------:|----------:|---------:|---------------:|-----------------:|
| pcloud-auth | 2567 | 293 | 0 | 0 | 26 | 15 |
| pcloud-backends | 16215 | 152 | 0 | 0 | 167 | 10 |
| pcloud-cache | 864 | 0 | 0 | 0 | 10 | 0 |
| pcloud-chaos | 171 | 574 | 0 | 0 | 0 | 5 |
| pcloud-cli | 14412 | 342 | 0 | 0 | 206 | 11 |
| pcloud-compat | 1489 | 47 | 0 | 0 | 23 | 1 |
| pcloud-config | 6120 | 391 | 0 | 0 | 86 | 30 |
| pcloud-crypto | 3921 | 760 | 67 | 28 | 59 | 20 |
| pcloud-daemon | 21527 | 3834 | 152 | 0 | 206 | 95 |
| pcloud-daemon-win | 294 | 0 | 0 | 0 | 0 | 0 |
| pcloud-engine | 5145 | 442 | 105 | 0 | 80 | 12 |
| pcloud-fleet | 941 | 562 | 0 | 0 | 10 | 3 |
| pcloud-fs | 19660 | 2781 | 339 | 0 | 175 | 19 |
| pcloud-idp | 1632 | 0 | 0 | 0 | 17 | 0 |
| pcloud-ipc | 4280 | 1430 | 135 | 21 | 25 | 48 |
| pcloud-kms | 1331 | 0 | 0 | 0 | 12 | 0 |
| pcloud-live-e2e | 84 | 2965 | 0 | 0 | 0 | 20 |
| pcloud-mockserver | 1013 | 238 | 0 | 0 | 9 | 7 |
| pcloud-model | 1679 | 0 | 0 | 0 | 22 | 0 |
| pcloud-observability | 3327 | 331 | 0 | 0 | 38 | 1 |
| pcloud-p2p | 544 | 0 | 0 | 0 | 16 | 0 |
| pcloud-plugin-api | 1795 | 0 | 0 | 0 | 23 | 0 |
| pcloud-plugin-autoheal | 397 | 223 | 0 | 0 | 0 | 5 |
| pcloud-plugin-backup-schedule | 931 | 0 | 0 | 0 | 5 | 0 |
| pcloud-plugin-dlp | 476 | 0 | 0 | 0 | 9 | 0 |
| pcloud-plugin-publink-expiry | 746 | 0 | 0 | 0 | 8 | 0 |
| pcloud-policy | 634 | 0 | 0 | 0 | 7 | 0 |
| pcloud-proto | 17224 | 1152 | 100 | 21251 | 170 | 35 |
| pcloud-resilience | 2845 | 114 | 0 | 0 | 54 | 1 |
| pcloud-sdk | 5284 | 344 | 119 | 0 | 49 | 4 |
| pcloud-secret | 402 | 315 | 69 | 0 | 0 | 22 |
| pcloud-session | 673 | 0 | 0 | 0 | 9 | 0 |
| pcloud-store | 4016 | 251 | 183 | 0 | 34 | 16 |
| pcloud-web | 1401 | 336 | 0 | 0 | 11 | 8 |

Crates with **zero** tests on disk:
- `pcloud-idp` (17 inline tests but no `tests/` dir — acceptable)
- `pcloud-kms` (12 inline tests, no external tests — but `#[ignore]d`
  AWS+Vault integration tests present at `lib.rs:1289-1311`)
- `pcloud-model` (22 inline, no external — small-surface data types)
- `pcloud-p2p` (16 inline, no external)
- `pcloud-plugin-api` (23 inline, no external)
- `pcloud-plugin-backup-schedule` / `dlp` / `publink-expiry` (zero
  external — these are plugin crates; integration exercised via
  `pcloud-plugin-api` and daemon integration)
- `pcloud-policy` (7 inline only)
- `pcloud-session` (9 inline only — but session lifecycle goes through
  `pcloud-daemon` tests)
- `pcloud-cache` (10 inline only)

None of these is a critical untested path; all ship through daemon
integration tests. No finding.

### Live-e2e suites present

`crates/pcloud-live-e2e/tests/`:

- `auth_lifecycle.rs` ✓ (4 `#[ignore]`d variants gated on
  `PCLOUD_LIVE_E2E=1` + credentials)
- `crypto.rs` ✓ (setup/unlock/status/mkdir/lock; rotation explicitly
  out of scope — documented)
- `drain.rs` ✓ (2 variants — state machine + in-flight guard)
- `field_selectors.rs` ✓
- `fleet_mtls.rs` ✓ (gated on controller URL + CA bundle)
- `integrity_sweeper.rs` ✓
- `mount_linux.rs` ✓ (double-gated on `PCLOUD_FUSE_TEST=1`; Linux
  only — **bd-1du.4 proof surface**)
- `public_links.rs` ✓
- `rate_limit.rs` ✓
- `shares.rs` ✓ (gated on `PCLOUD_TEST_PEER_USER`)
- `snapshot_pipeline.rs` ✓ (default + GPG variants)
- `snapshot_prune.rs` ✓ (GFS semantics)
- `sync_loop_live.rs` — **present** per `ls` output
- `sync_roots.rs` ✓ (all sync-type flavors)
- `transfers.rs` ✓ (upload/download round-trip)

Missing live-e2e for parity rows (MEDIUM-noted, not blocking):

- No live suite for **backup device lifecycle** (`BackupCreate`,
  `BackupDelete`, `StopDevice`) — tracked under retained parity rows.
- No live suite for **business / team share** beyond the basic
  `shares.rs` folder-invite flow.
- No live suite for **crypto password rotation** — explicitly
  out-of-scope per docstring (email-confirmation delivery not
  programmatic). Acceptable.
- No live suite for **mount write-path crash replay** — covered
  indirectly by `crates/pcloud-fs/tests/write_path_replay.rs` (gated).

### Proptest inventory

17 files reference proptest (source: Grep of `proptest!|proptest::`).

**IPC roundtrip** (`pcloud-ipc/tests/proptest_methods_roundtrip.rs`):
~24 of 81 `Request` variants, 31 of 45 `Method` variants in explicit
arms with a `_ => 0` fallthrough. **See HIGH-Q1 and MEDIUM-Q6.**

**Other proptest files:**
- `pcloud-ipc/tests/peer_and_protocol.rs` — peer-cred + framer.
- `pcloud-resilience/tests/circuit_breaker_proptest.rs` — state
  transitions.
- `pcloud-secret/tests/proptest_zeroize_invariants.rs`,
  `redaction_and_zeroize.rs` — secret zeroization.
- `pcloud-daemon/tests/proptest_sync_and_resolver.rs` — sync path
  canonicalization + conflict resolver.
- `pcloud-crypto/tests/proptest_seal.rs` — sector seal round-trip.
- `pcloud-proto/tests/proptest_framer.rs`,
  `proptest_response_and_frames.rs` — framer + response parser.

Config parser proptest: **absent**. Path validation proptest: partial
(covered via `pcloud-daemon/tests/proptest_sync_and_resolver.rs` for
sync paths, but not for public-link / backup / snapshot paths).

### Fuzz targets

Total: **9 fuzz targets** across 3 crates.

- `crates/pcloud-proto/fuzz/fuzz_targets/`:
  - `fuzz_auth_flow_state.rs` ✓
  - `fuzz_binary_request_roundtrip.rs` ✓
  - `fuzz_ipc_method_decode.rs` ✓
  - `fuzz_json_response.rs` ✓
  - `fuzz_listfolder_response.rs` ✓
  - `fuzz_path_canonicalize.rs` ✓
  - `fuzz_response_parser.rs` ✓
- `crates/pcloud-crypto/fuzz/fuzz_targets/`:
  - `fuzz_open_sector.rs` ✓ (requested by audit brief)
- `crates/pcloud-ipc/fuzz/fuzz_targets/`:
  - `fuzz_ipc_frame.rs` ✓

**CI fuzz job present** at `.github/workflows/fuzz.yml`: nightly
02:00 UTC cron, runs `fuzz_ipc_frame` and `fuzz_open_sector` for 300 s
each, `continue-on-error: true`. The other 7 proto fuzz targets are
**not** exercised in CI — LOW.

**Missing fuzz target (LOW-Q17):** no `fuzz_dispatch_envelope` for the
daemon dispatch pipeline.

### Benchmarks

11 bench files present covering all audit-02-requested targets
(page cache, chunked flush, IPC throughput, crypto sector ops,
dispatch end-to-end, engine, upload session, secret CT equality,
store KV, proto dispatch, sync root canonicalize). **No finding on
benches — see LOW-Q15 for acknowledgement.**

Minor gap (LOW-Q16): no write-journal-replay throughput benchmark
despite the crash-recovery focus.

### Cross-platform CI matrix

`.github/workflows/ci.yml`:
- **Linux** — `ubuntu-latest`, fuse3 + libfuse3-dev installed;
  `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `cargo test --workspace`, `cargo deny check`. ✓
- **macOS** — `macos-latest`, `cargo test --workspace --exclude
  pcloud-fs`. ✓ (pcloud-fs excluded as required by audit brief)
- **Windows** — `windows-latest`, `cargo test --workspace --exclude
  pcloud-fs`. ✓
- **FreeBSD** — commented out. **MEDIUM-Q11.**
- **Release build** — Linux only, needs `test-linux` to pass.

### Test hygiene spot-check (10 tests)

Checked (random sample across crates):
1. `pcloud-ipc/tests/proptest_methods_roundtrip.rs:100` — meaningful:
   round-trips every method variant.
2. `pcloud-crypto/tests/proptest_seal.rs` — meaningful.
3. `pcloud-daemon/tests/live_auth.rs:71` — `#[ignore]`d with env-var
   requirement string.
4. `pcloud-fs/tests/fuse_kernel_e2e.rs` — `#[cfg(target_os="linux")]`
   + `#[ignore]` with `PCLOUD_FUSE_TEST=1` requirement.
5. `pcloud-backends/src/account_backend_tests.rs` — mock-backed, no
   sleep-based races observed.
6. `pcloud-secret/tests/proptest_zeroize_invariants.rs` — meaningful.
7. `pcloud-live-e2e/tests/sync_roots.rs` — `#[ignore]`d with
   `PCLOUD_LIVE_E2E=1`.
8. `pcloud-resilience/tests/circuit_breaker_proptest.rs` — meaningful.
9. `pcloud-engine/tests/...` (inline via `benches/engine.rs`) —
   benchmark, no race.
10. `pcloud-compat/tests/cross_process_shm.rs` — `#[ignore]`d
    integration test.

No "trivially-true" `assert!(true)` or sleep-based timing races found
in the sampled set.

### `#[ignore]` discipline

**72 occurrences across 38 files.** Every audited `#[ignore]` carries a
descriptive gating string:
- `#[ignore = "requires PCLOUD_LIVE_E2E=1 + credentials"]` (live-e2e)
- `#[ignore = "requires PCLOUD_FUSE_TEST=1 ..."]` (fuse integration)
- `#[ignore = "requires PCLOUD_GPG_TEST=1 and PCLOUD_GPG_RECIPIENT in
  keyring"]` (snapshot GPG)
- `#[ignore = "requires AWS creds + PCLOUD_KMS_AWS_TEST=1 +
  PCLOUD_KMS_AWS_KEY_ARN"]` (KMS integration)

No bare `#[ignore]` without a reason. ✓

---

## Relevant file paths

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/methods.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/integrity_sweeper_service.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/sync_loop_runtime.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/mount_runtime.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/inode.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/write_path.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/linux.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/macos.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/fuser_shim.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/src/upload_session.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/src/main.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/src/migrate.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/macos_ffi.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/.github/workflows/ci.yml`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/.github/workflows/fuzz.yml`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-live-e2e/tests/mount_linux.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-model/src/ids.rs`
