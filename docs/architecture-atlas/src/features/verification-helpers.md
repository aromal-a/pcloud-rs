# Verification, test, fuzz, and developer features

Verification code is part of the project architecture because it defines
which claims can be checked. It is not shipped product functionality. A mock
roundtrip, a source-compatible platform module, and a successful real-account
native package test prove different things.

The complete verification-family vocabulary is: unit tests, integration tests,
mock server, live E2E, chaos, fuzz, benchmarks, mutation testing, coverage,
disaster recovery, and reproducibility. Each family proves a different layer;
none inherits the claims of a stronger native or live layer automatically.

## Evidence ladder

| Layer | What it proves | What it cannot prove |
|---|---|---|
| Unit test | Local function/type/state invariant | Real composition, OS, network, service behavior |
| Property test | Invariant over many generated inputs | Complete input space or external compatibility |
| Compile-fail/static assertion | Forbidden API/type relationship or compile-time contract | Runtime behavior |
| Integration test | Multiple real crates/process components compose | Real pCloud, native kernel/hardware unless used |
| Mock-server test | Actual HTTP/codec path handles controlled service responses | Undocumented/current production service behavior |
| Known-answer test | Exact output matches an external/recorded oracle | Every version/platform or complete lifecycle |
| Fuzz test | Finds crashes/invariant violations over explored malformed inputs | Absence of bugs after finite time |
| Chaos/DR test | Recovery behavior under one scripted fault | All crash timings/storage/network failures |
| Benchmark/profile | Measured cost on one harness/host/corpus | Universal performance or production scale |
| Live E2E | Real account/service behavior for selected scenario | Other account entitlements, regions, platforms, package lifecycle |
| Native mount/package test | Actual OS ABI/artifact lifecycle | Other OS versions/models/signing channels |
| Clean release gate | Exact commit/artifact passed required matrix | Future releases |

## Ordinary workspace tests

Every crate owns unit and/or integration tests close to the behavior. The
generator inventories all test targets and files in [internal modules and
helpers](../generated/features/source-units.md). Broad categories include:

| Test feature | Why it exists / examples | Why it is valuable |
|---|---|---|
| Domain/serde roundtrips | Model, IPC envelope, protocol response, store and config types | Prevents silent wire/disk schema drift. |
| Error-code stability | Shared taxonomy/CLI mapping | Protects scripts and SDK consumers from accidental renumbering/reclassification. |
| Auth state transitions | command/state/event/orchestrator/session refresh | Proves invalid states and secret-free events without network timing. |
| Store migration/transaction tests | schema versions, repositories, busy retry, audit integrity | Makes crash/upgrade invariants executable. |
| RemoteFs cold-cache tests | live-like folder/transfer mocks with empty metadata cache | Prevents reintroduction of cache-as-authority bugs. |
| Transfer/journal replay tests | partial upload/download, changed source, crash replay, conflict | Proves resume only under matching identity and ordered state. |
| Engine planner/property tests | scanning, paths, conflicts, scheduler, recovery, resolver | Exercises deterministic reconciliation and path corner cases. |
| Filesystem adapter tests | inode, normalization, read/write, staging, journal, writeback, mount lifecycle | Proves portable core without requiring a kernel mount on every run. |
| IPC security tests | frame caps, malformed methods, peer rules, secret redaction, concurrent clients | Hardens the credential-holding local boundary. |
| Crypto roundtrip/property/KAT tests | Enhanced AEAD, pclsync primitives/profile/filename, rotation, lockout, KMS mocks | Separates internal consistency from byte-level compatibility evidence. |
| Secret compile/behavior tests | no serde/clone, redacted Debug, zeroize, constant-time equality | Guards type-system security posture from convenience regressions. |
| Plugin capability tests | manifest signatures, grant checks, panic containment, built-in behavior | Prevents extension operations from bypassing declared authority. |
| Web/WebDAV tests | bind/host/routes/templates, parser/PROPFIND/dispatch/backend | Exercises local HTTP behavior while retaining experimental labels. |
| Platform selector tests | synthetic host family for IPC/vault/mount/config | Covers selection logic, but native ABI still requires real hosts. |

## Mock pCloud service (`pcloud-mockserver`)

| Feature | Why it exists | Good for, and why | Limit |
|---|---|---|---|
| In-process HTTP server | Runs real client socket/HTTP parsing against canned outcomes without internet. | Protocol/backend integration in ordinary CI. | It implements only scripted behavior, not the full pCloud service. |
| Endpoint builders | Configure common login/userinfo/listfolder/etc. responses. | Readable tests focused on a scenario rather than HTTP boilerplate. | Builders can become stale unless compared with live/spec evidence. |
| Request matching/assertion | Keys responses on method/query and records calls. | Proving exact command/parameter reachability. | Does not prove the server accepts it today. |
| Error/timing injection | Returns controlled failures/slow responses. | Resilience and error mapping. | Real networks exhibit more combinations. |
| No secrets/network | Uses local fixtures only. | Safe deterministic developer runs. | Cannot qualify credentials, TLS roots, regions, entitlements, or account state. |

## Live pCloud E2E harness (`pcloud-live-e2e`)

All live tests are `#[ignore]`, require the runtime gate, use disposable
accounts, and must run serially because they mutate shared account state. A
strict release gate converts selected missing/degraded prerequisites to
failures, but it does not automatically make every soft skip in every binary
strict.

| Test binary | Feature it tries to prove | Important boundary/limitation |
|---|---|---|
| `account_utility` | Read/non-destructive account API utility calls | Account/region/service behavior varies |
| `account_utility_destructive` | Email/password mutation flow | Requires explicit destructive gate; interrupted rotation leaves a recovery marker |
| `auth_lifecycle` | Password/token login, logout, status, persistence/vault permissions | Ordinary no-TFA account recommended |
| `tfa_lifecycle` | TFA code/recovery/resend paths | Needs separate TFA fixture; TOTP expires and recovery code may be consumed |
| `backup_lifecycle` | Backup/device create/stop/delete | Must clean all server objects |
| `change_crypto_pass` | Crypto password-change rejection/email trigger | Full happy path needs email OTP out of band |
| `crypto` | Setup/unlock/status/mkdir/lock/re-unlock | Dedicated Crypto entitlement/password and backend compatibility evidence required |
| `drain` | Runtime drain state and in-flight guard | Mostly local state-machine evidence, not a remote-service requirement |
| `field_selectors` | CLI/response JSON and legacy shape selection | Proves selected response fields, not every command schema |
| `fleet_mtls` | Live configured controller heartbeat over the historically named agent | Requires an external HTTPS controller/CA; TLS authenticates the controller only, while Ed25519 HTTP headers identify the device; failures are not a fleet-server product test |
| `integrity_sweeper` | Run-once/status against a seeded root | Scale/latent corruption coverage remains separate |
| `mount_linux` | Upload fixture, kernel mount read/unmount/cleanup | Requires Linux `/dev/fuse`, credentials, and mount gate |
| `public_links` | Create/list/change password/expiry/delete | Public-link account policy/entitlement may vary |
| `tree_link_from_paths` | Resolve mixed paths and create a selection link | Cleanup and path fixture ownership are essential |
| `rate_limit` | Expensive-request burst returns categorized conflict/retry hint | Proves configured daemon limiter, not pCloud global quota behavior |
| `shares` | Invite/list/cancel/modify/remove with peer email | Single-account view cannot prove recipient acceptance |
| `shares_a_to_b` | Two-account invite/accept/visibility/revoke/cleanup | Distinct verified accounts, TFA-off login, and possible out-of-band acceptance required |
| `shares_active_a_to_b` | Observes an already accepted bilateral share | May rely on a deliberately retained artifact from a supervised prior pass |
| `snapshot_pipeline` | Snapshot create/verify/prune, optional GPG | Requires local `gpg`/recipient for encryption; restore drill is separate |
| `snapshot_prune` | GFS/retention set selection | Synthetic snapshot metadata, not remote object lifecycle by itself |
| `sync_loop_live` | Real remote/local reconciliation loop | Needs controlled scratch paths and deterministic cleanup |
| `sync_roots` | Add/list/change type/pause/resume/remove | Root overlap/path cleanup can affect later tests |
| `team_share_verb` | Business/team-share verb reachability | Synthetic invalid IDs do not prove a successful team lifecycle |
| `transfers` | Upload session, signed link, exact download, cleanup | Core real byte roundtrip; account region and dynamic transfer hosts must be reachable |
| `upload_writefromfile` | Specialized upload verb reaches backend | Current synthetic invalid-ID check is not a successful file roundtrip |
| `windows_liveness` | Native Windows login/transfer/basic service behavior | Uploaded file IDs may require manual cleanup from the trace log |

The local ignored `.env.live-e2e.local` template is intentionally outside Git
and is not part of the website build. The committed harness README/source is
the shareable reference; credentials must never enter this site or logs.

### What a live run needs to finish cleanly

- master gate, correct EU/US API host, dedicated verified primary account,
  token plus password credentials for full auth coverage, and a disposable
  writable scratch parent;
- a second distinct no-TFA account for bilateral sharing, plus inbox access;
- a separate TFA-enabled fixture/pass, because one environment cannot be both
  ordinary no-TFA and TFA challenged;
- optional Crypto password/entitlement, GPG key, fleet controller/CA, and
  native FUSE/WinFSP/macOS backend only for those scenarios;
- `--test-threads=1`, stable private `TMPDIR`, sufficient disk, DNS/TLS and
  outbound 443 to API and dynamic upload/download hosts;
- destructive and keep-artifacts gates off for the first run;
- after the run: no process/mount, no recovery marker, no trace IDs awaiting
  deletion, and no scratch object/link/share/backup/sync/Crypto artifact on
  either account.

## Chaos harness (`pcloud-chaos`)

| Scenario | Why it exists | Expected proof |
|---|---|---|
| Network blackhole | Simulates an endpoint that cannot be reached. | Retry budget/circuit breaker prevents indefinite hammering and returns a classified failure. |
| Slowloris/slow peer | Holds a connection with insufficient progress. | Timeout and cleanup terminate the operation without leaking state. |
| Disk-full journal | Fails durable state writes under storage exhaustion. | No false acknowledgment/publication; error is surfaced and existing journal remains recoverable. |
| SIGKILL mid-flush | Kills the writer at a critical persistence point. | Replay detects pending/completed boundary and does not lose or duplicate committed intent. |
| Clock jump/TTL | Moves wall/monotonic assumptions. | TTL/retry logic uses the intended clock and avoids negative/overflow/instant expiry surprises. |

These tests are opt-in because they are slow, platform-specific, or
intentionally disruptive. They prove the scripted fault and outcome—not every
possible I/O interleaving.

## Disaster-recovery drills

| Drill | Why it exists | What success means |
|---|---|---|
| Store corruption | Tests detection, backup/restore, and controlled rebuild behavior. | The daemon does not silently trust corruption and documented state can be restored/reconstructed. |
| Sync-root mass eviction | Exercises large destructive/drift-like state recovery. | Recovery preserves intended roots/data policy and surfaces what cannot be reconstructed. |
| Vault loss | Simulates missing durable token storage. | No plaintext credential recovery is attempted; operator reauthentication restores service safely. |
| Repro helper self-test | Proves artifact comparison scripts detect meaningful difference. | The reproducibility gate itself is not a no-op. |
| Memory-profile gate self-test | Proves profiler thresholds/harness failure behavior. | Memory regression tooling can fail on a known bad case. |

DR drills must run in disposable roots/VMs. A successful script is not a
substitute for restoring the actual signed/encrypted release artifacts and
operator keys.

## Fuzzing

| Target family | Inputs exercised | Why it exists |
|---|---|---|
| Root `ipc_request` | Arbitrary local IPC request frames/payloads | Harden the owner-authenticated but still hostile local parsing boundary. |
| Root `transport_frame` | Remote/binary transport frames | Find length/type/parser panics and resource problems. |
| Root `public_link_uri` | Link URI/text parsing | Reject malformed/untrusted public link inputs safely. |
| Proto `fuzz_binary_request_roundtrip` | Binary method encoding/decoding | Preserve wire roundtrip invariants. |
| Proto `fuzz_response_parser` / `fuzz_json_response` | Arbitrary service responses | Avoid panics, unbounded allocation, and unsafe value assumptions. |
| Proto `fuzz_listfolder_response` | Nested folder metadata | Protect a high-volume path and recursion/shape assumptions. |
| Proto `fuzz_ipc_method_decode` | Method discriminants/payloads | Keep method decoder total over malformed input. |
| Proto `fuzz_auth_flow_state` | Auth response/state combinations | Find impossible-transition/panic/secret-format edge cases. |
| Proto `fuzz_path_canonicalize` | Path bytes/components | Reject traversal, ambiguity, and platform normalization edge cases. |
| Crypto `fuzz_open_sector` | Arbitrary keys/ciphertext frames | Authentication failure must be safe and non-panicking. |
| Crypto `fuzz_pclsync_filename_decode` | Arbitrary compatible filename envelopes | Bound and reject malformed base32/text without decoder panic. |

Corpus/crash artifacts must be retained, minimized, converted to regression
tests, and reviewed for secret content before publication.

## Benchmarks and profiling

| Benchmark | Why it exists / measured path |
|---|---|
| Crypto AEAD sector | Enhanced sector seal/open throughput and overhead |
| Secret constant-time equality | Cost of hardened secret comparison |
| IPC codec | Local request/response encode/decode overhead |
| Protocol dispatch | Typed remote method/response routing cost |
| Store KV | SQLite typed-value throughput/latency |
| Engine | Planning/scheduling/reconciliation primitives |
| FS page cache | Cache lookup/eviction behavior |
| FS chunked flush | Large staged flush behavior |
| FS writeback flush | Journal/staging/writeback cost |
| Daemon cold start | Bootstrap/config/store/runtime startup latency |
| Daemon dispatch end-to-end | Local request through runtime response |
| Sync-root canonicalize | Local path validation/canonicalization cost |
| Vault open/close | Durable token vault lifecycle overhead |
| Embedded SDK upload session | High-level chunk/session call overhead |

Benchmark results require host, toolchain, profile, corpus, sample count, and
methodology. They guide regression/capacity decisions; they are not public
performance promises by themselves.

## Coverage and quality gates

| Gate | Why it exists | Important interpretation |
|---|---|---|
| `cargo fmt --check` | Stable mechanical style and smaller diffs | Says nothing about behavior |
| all-target check/build | Compiles libraries, binaries, examples, tests, benches for the host | Does not run behavior or other native targets |
| strict Clippy | Enforces workspace lint policy | Lint-clean is not bug-free/security proof |
| workspace tests | Runs non-ignored deterministic behavior | Live/destructive/native ignored tests remain separate |
| rustdoc/mdBook | Keeps public docs/examples/site buildable | Documentation truth still needs source/evidence review |
| cargo audit/deny | Checks known advisories/license/source policy | Cannot find unknown/application logic vulnerabilities |
| line coverage >90% | Prevents large unexecuted regions and tracks regression | Line execution is not assertion quality, branch/path completeness, or live proof |
| optional feature/MSRV checks | Proves selected compile combinations and compatibility contracts | Every provider/native runtime still requires its own tests |
| reproducible release build | Compares deterministic artifacts under controlled inputs | Signing/notarization often adds later nondeterministic envelopes and needs separate policy |

## Mutation testing: manual, not enforced

Mutation testing asks a different question from line coverage. `cargo-mutants`
temporarily changes executable expressions—such as a comparison, boolean,
constant, or returned value—and reruns the tests. A **caught** mutant makes at
least one test fail; a **surviving** mutant is concrete evidence that the test
suite did not distinguish the changed behavior. This is valuable for crypto,
authentication, retry, secret, and parser invariants where a line can execute
without an assertion proving the important result.

Mutation results are still finite evidence. Catching the generated mutants
does not prove security, protocol compatibility, concurrency schedules,
native behavior, or live pCloud behavior. Equivalent mutants can preserve
observable semantics and require human classification rather than a forced
test.

### What `.cargo/mutants.toml` controls

| Setting | Current meaning |
|---|---|
| `test_tool = "cargo"` | Run mutants through the ordinary Cargo test harness so the baseline matches the local coverage/test path. |
| `minimum_test_timeout = 120` | Give each mutant at least 120 seconds; cargo-mutants may calibrate a longer timeout from the baseline. |
| `exclude_re` | Skip `src/error.rs` and top-level `src/lib.rs` re-export/wiring files because those mutations are commonly noisy. This is a narrow exception, not permission to move logic into excluded files. |
| Commented MMR goal | Catch at least 75% of generated mutants per listed high-risk crate: `pcloud-crypto`, `pcloud-auth`, `pcloud-resilience`, and `pcloud-ipc`. The percentage is a review goal written in comments, not a machine-enforced threshold in this file. |

Run one crate from a clean worktree with the same locked toolchain used for
release work:

```sh
cargo install cargo-mutants --locked
cargo mutants -p pcloud-crypto
```

Repeat for each high-risk crate, retain the baseline and outcomes, calculate
the caught ratio after classifying timeouts/unviable/equivalent mutants, and
turn real survivors into focused regression tests. Do not simply add broad
exclusions to improve the ratio.

<span class="atlas-unqualified">Current enforcement status: manual.</span>
Neither `cargo xtask ci` nor `cargo xtask release` invokes cargo-mutants, the
disabled workflow archive is not an active gate, and no repository command
parses/enforces the 75% goal. Therefore a successful ordinary CI/release run
does **not** imply mutation testing ran. A release record may cite mutation
evidence only when it retains the command, tool version, crate, commit,
outcomes, classifications, and resulting ratio from an explicit run.

## `xtask`: authoritative local CI/CD helper

GitHub workflows are disabled. `xtask` owns command order, Rust 1.96.1 main
toolchain, compatibility checks, coverage threshold, Docker, native Windows
SSH, packaging, release, and cleanup. This is good for reproducibility because
the same pipeline runs on a developer/release host and its skip policy is
versioned in Rust.

`PCLOUD_CI_SKIP_DOCKER=1` or `PCLOUD_CI_SKIP_WINDOWS=1` makes a run explicitly
partial. Docker proves Linux container/glibc/musl behavior, not a foreign
kernel. The Windows stage uses key transfer/cleanup plus a credential-bearing
session where CurrentUser DPAPI requires it; secrets stay in process
environment, not command arguments or repository files.

## Documentation generator as a verification feature

The architecture atlas generator reads current Cargo metadata and Git-visible
files, then regenerates package, target, feature-flag, API-capability, source
unit, declaration, and file inventories. The link checker and mdBook build
make missing pages/links fail. This does not prove the prose is correct, but it
does make silent omission of a crate, Cargo flag, API parity row, or Rust file
detectable.
