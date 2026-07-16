# Testing

The workspace runs a **seven-layer testing pyramid**. The goal is
not "more tests" — it is **different classes of evidence** that the code is
correct. A unit test proves a branch is taken. A property test proves an
invariant holds over thousands of inputs. A fuzz target proves no adversary
input causes a panic or memory error. A mutation run proves the tests
actually catch a broken implementation.

> **Honesty note (2026-04-26, audit-06 wave-G8 M-01):** Not all layers are
> currently enforced as hard PR gates. The table's "CI gate" column describes
> the intended policy and current enforcement. Scheduled/manual layers do not
> block ordinary pull requests, but a failure in a job that does run is not
> silently converted to success. Release-selected live tests are separately
> hard-gated.

## The Pyramid at a Glance

| Layer              | Count / scope                                  | Local cadence | CI gate (current enforcement)                |
| ------------------ | ---------------------------------------------- | ------------- | -------------------------------------------- |
| Unit tests         | see `cargo test --workspace --locked` output   | Every change  | Every PR, blocking                           |
| Property tests     | 10 `proptest` modules; module-specific budgets | Every change  | Every PR, blocking                           |
| Fuzz targets       | 14 targets across 5 fuzz workspaces, 5 min each | Nightly     | Scheduled/manual jobs fail on crashes       |
| Mutation testing   | `cargo-mutants`, 5 crates, **75 % MMR floor**  | Manual / weekly | *(not yet in CI)*                          |
| Chaos scenarios    | **5 scenarios** in `pcloud-chaos`              | Manual        | *(not yet in CI; deferred, see ci.yml)*      |
| Coverage           | `cargo-llvm-cov`, 65% workspace + critical floors | Weekly / manual | Push/PR and weekly/manual hard floors    |
| Live E2E           | broad weekly/manual suite + strict release subset | Weekly / release | Release transfer/share/Linux-mount gates |

## 1. Unit Tests

Unit tests live next to the code they cover in `#[cfg(test)] mod tests`
blocks. Rules:

- Run in under **10 ms** each on average.
- Never touch the filesystem outside `tempfile::tempdir()`.
- Never open a network socket.
- Never sleep on wall-clock time — use `tokio::time::pause()`.
- Be deterministic across runs and platforms.

**Run locally:**

```sh
cd .
cargo test --workspace --locked
```

**Iterate on one crate:**

```sh
cargo test -p pcloud-daemon --lib --locked -- --nocapture
```

Current count: use `cargo test --workspace --locked` output from the
current branch; do not copy historical counts into reviews. Expect 5–10
new unit tests per new feature; commands missing unit coverage fail
review.

**CI gate:** runs on every PR. A single failure blocks merge.

## 2. Property Tests

We use `proptest` to generate inputs and assert invariants. The workspace
currently has ten property-test modules across `pcloud-crypto`,
`pcloud-daemon`, `pcloud-ipc`, `pcloud-proto`, `pcloud-resilience`,
`pcloud-embedded-sdk`, and `pcloud-secret`. They cover:

- frame, response, request, and method round-trips plus malformed input,
- encryption/decryption, redaction, constant-time equality, and zeroization,
- sync/resolver state transitions and upload-session progress,
- circuit-breaker behavior over generated failure sequences.

Case budgets are module-specific. The protocol framer and crypto sealing suites
pin 128 cases for predictable PR latency; other suites use the repository's
current `proptest` defaults unless their source specifies a local budget.

**Run locally:**

```sh
cargo test -p pcloud-proto  --locked --test proptest_framer
cargo test -p pcloud-crypto --locked --test proptest_seal
```

**Deep-dive with more cases:**

```sh
PROPTEST_CASES=10000 cargo test -p pcloud-proto --locked --test proptest_framer
```

Failing inputs auto-shrink to a minimal counter-example and persist in
`proptest-regressions/<file>.txt`. **Commit the regression file** so the
failing case is always re-checked.

**CI gate:** runs with the default 128-case budget on every PR.

## 3. Fuzz Targets

Fuzz targets run under `cargo-fuzz` (libFuzzer) on a nightly toolchain.
The authoritative target list is the matrix in `.github/workflows/fuzz.yml`.
Current targets:

- `pcloud-ipc`: `fuzz_ipc_frame` — IPC frame decoder
- `pcloud-crypto`: `fuzz_open_sector` — sector AEAD decoder
- `pcloud-crypto`: `fuzz_pclsync_filename_decode` — pclsync base32+AES filename decoder
- `pcloud-daemon`: `fuzz_auth_vault_decode` — auth-vault token parser
- `pcloud-proto`: `fuzz_auth_flow_state`, `fuzz_binary_request_roundtrip`,
  `fuzz_ipc_method_decode`, `fuzz_json_response`, `fuzz_path_canonicalize`,
  `fuzz_response_parser`, `fuzz_listfolder_response`
- cross-crate root workspace: `transport_frame`, `ipc_request`,
  `public_link_uri`

**Run locally** (nightly toolchain required):

```sh
rustup toolchain install nightly-2026-06-03
cd crates/pcloud-ipc
cargo +nightly-2026-06-03 fuzz run fuzz_ipc_frame -- -max_total_time=300
```

Corpora live under `<crate>/fuzz/corpus/<target>/` and are cached by CI.
When a crash is found, minimize with `cargo fuzz tmin` and commit the
reproducer as a regression seed.

**CI gate (current):** scheduled/manual matrix. Every fuzz target is wired;
crashes fail the owning job and artifacts are retained with `if: always()`.
A crash generates an artifact for human triage but does not block PRs.
Tracked under `bd-1du.10` for hardening.

## 4. Mutation Testing

`cargo-mutants` mutates the source, re-runs the test suite, and flags any
mutation the tests did not catch. A surviving mutation is direct evidence
that a branch is not actually exercised, even if line coverage shows
green.

**Scope:** 5 crates for the manual/weekly release check:

- `pcloud-crypto`
- `pcloud-auth`
- `pcloud-resilience`
- `pcloud-secret`
- `pcloud-ipc`

**Floor:** **75 % mutants must be caught** per crate (Minimum Mutation
Ratio — MMR). A drop below floor opens a triage bead.

**Schedule (current):** *(not yet in CI)* — no dedicated GitHub Actions
workflow exists. Run locally before cutting a release tag. Tracked under
`bd-1du.10`.

**Run locally on a single crate:**

```sh
cargo install cargo-mutants --locked
cargo mutants -p pcloud-crypto --timeout 60
```

Triage surviving mutations into beads tagged `test-gap`.

## 5. Chaos Testing

`crates/pcloud-chaos/` exercises the daemon under **adversarial
environmental conditions**. Five scenarios ship:

### Lightweight scenarios

1. **Network blackhole** during streaming download — verify resume tokens
   and idempotent retry recover cleanly.
2. **Clock jump** (host clock jumps ±24 h mid-request) — verify auth
   token refresh and cache expiry survive.

### Heavy opt-in (`PCLOUD_CHAOS=1`)

3. **SIGKILL** mid-upload — verify journal rollback, no torn writes, no
   orphaned temp files on restart.
4. **Disk full** during writeback — verify graceful error surface and
   that the journal replays correctly after space is freed.
5. **Slowloris peer** (IPC client sends one byte per 5 s) — verify
   daemon timeouts fire and do not starve other peers.

The opt-in trio is heavy: each scenario serialises for minutes and
requires privileged hooks. No GitHub Actions chaos workflow exists today;
the entire chaos layer is developer-run/manual until the timing instability
noted in `.github/workflows/ci.yml` is resolved.

**Run locally (default two):**

```sh
cargo test -p pcloud-chaos --test scenarios --locked -- --test-threads=1
```

**Run locally (full five):**

```sh
PCLOUD_CHAOS=1 cargo test -p pcloud-chaos --test scenarios --locked -- --test-threads=1
```

Chaos tests deliberately serialise (`--test-threads=1`) because they
install process-wide signal handlers and fault injectors.

**Local CI gate:** `cargo xtask coverage` runs the deterministic ignored chaos
suites serially with `PCLOUD_CHAOS=1`. Longer destructive/live scenarios
remain explicit release-operator exercises.

## 6. Coverage

We use `cargo-llvm-cov` to generate line-coverage reports.

**Enforced posture:**

- workspace floor: **90 %**
- per-crate floors for security-sensitive crates:

| Crate              | Floor |
| ------------------ | ----- |
| `pcloud-crypto`    | 85 %  |
| `pcloud-auth`      | 85 %  |
| `pcloud-resilience`| 80 %  |
| `pcloud-secret`    | 90 %  |
| `pcloud-ipc`       | 80 %  |

**Local CI gate:** `cargo xtask coverage` publishes
`target/xtask/lcov.info` and hard-fails through
`scripts/coverage-check.sh` when the workspace or any critical-crate floor is
missed. GitHub Actions is intentionally disabled.

**Run locally:**

```sh
cargo install cargo-llvm-cov --version 0.8.7 --locked
cargo xtask coverage
```

## 7. Live End-to-End Tests

`crates/pcloud-live-e2e/` contains tests that require a real pCloud
account. Every test function carries `#[ignore]` and a runtime gate
(`PCLOUD_LIVE_E2E=1`), so a plain `cargo test` never runs them.

The broad suite runs weekly and on manual dispatch; missing optional
family-specific variables still skip those families. The release workflow
selects transfer/public-link, two-account share, and Linux mount tests under
`PCLOUD_RELEASE_GATE=1`, where missing prerequisites and degraded behavior are
failures. See `crates/pcloud-live-e2e/README.md`.

```sh
export PCLOUD_LIVE_E2E=1
export PCLOUD_TEST_USER=staging-bot@example.com
export PCLOUD_TEST_PASSWORD=…          # use a vaulted env

cargo test -p pcloud-live-e2e --locked -- --ignored --test-threads=1
```

The staging account credentials live in the release team's 1Password
vault. **Never** wire a personal account to CI.

**CI gate:** `.github/workflows/ci.yml` runs the broad suite only on the weekly
schedule and manual `workflow_dispatch`; failures are not swallowed. The tag
release workflow separately selects strict transfer/public-link,
two-account-share, and native Linux mount tests with
`PCLOUD_RELEASE_GATE=1`, so those paths block publication.

## Running Everything Locally

Before a big PR, run the full pre-merge gate:

```sh
cd .
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test   --workspace --locked
cargo llvm-cov --workspace --locked --summary-only
cargo audit --deny warnings \
  --ignore RUSTSEC-2023-0071
cargo deny --locked check
```

Nightly-only layers (fuzz, mutants, full chaos) are opt-in locally but
must be green in CI before a release tag is cut.

## Which Layer for What

Rule of thumb when adding coverage:

- **Pure logic, single function** → unit test.
- **Two or more modules cooperating** → integration test under `tests/`.
- **Invariant over a wide input space** (codecs, parsers, allocators) →
  property test.
- **Attacker-influenced input** (IPC frames, protocol responses, sealed
  boxes, filesystem journals) → property test **and** fuzz target.
- **Security-critical module** → add to mutation-testing crate list and
  lift the coverage floor.
- **Failure-mode behaviour under environmental faults** (disk, clock,
  peers, signals) → chaos scenario in `pcloud-chaos`.
- **Real-server-only behaviour** → live E2E.

Do not stack unit tests on top of unit tests when a property, fuzz, or
chaos test is the better tool. The P2 testing wave (`RUST-PLANS/
32-P2-TESTING-HARDENING.md`) is the canonical reference for that
placement decision.
