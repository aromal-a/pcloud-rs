# Testing

The workspace runs a **seven-layer testing pyramid**. The goal is
not "more tests" — it is **different classes of evidence** that the code is
correct. A unit test proves a branch is taken. A property test proves an
invariant holds over thousands of inputs. A fuzz target proves no adversary
input causes a panic or memory error. A mutation run proves the tests
actually catch a broken implementation.

> **Honesty note (2026-04-26, audit-06 wave-G8 M-01):** Not all layers are
> currently enforced as hard PR gates. The table's "CI gate" column describes
> the **intended steady-state** policy; current enforcement is shown in
> parentheses. Layers marked *(advisory)* run in CI with
> `continue-on-error: true` and do not block merges today. Layers marked
> *(not yet in CI)* have no scheduled workflow. These gaps are tracked under
> `bd-1du.10` and must close before a release tag is cut.

## The Pyramid at a Glance

| Layer              | Count / scope                                  | Local cadence | CI gate (current enforcement)                |
| ------------------ | ---------------------------------------------- | ------------- | -------------------------------------------- |
| Unit tests         | see `cargo test --workspace` output            | Every change  | Every PR, blocking                           |
| Property tests     | **7 properties × 128 cases** each              | Every change  | Every PR, blocking                           |
| Fuzz targets       | 11 targets across 4 crates, cargo-fuzz, 5 min  | Nightly       | Nightly CI, `continue-on-error` *(advisory)* |
| Mutation testing   | `cargo-mutants`, 4 crates, **75 % MMR floor**  | Manual / weekly | *(not yet in CI)*                          |
| Chaos scenarios    | **5 scenarios** in `pcloud-chaos`              | Manual        | *(not yet in CI; deferred, see ci.yml)*      |
| Coverage           | `cargo-llvm-cov`, informational report         | Weekly / manual | Weekly, `continue-on-error` *(advisory)*   |
| Live E2E           | weekly + manual dispatch, real account         | Weekly / manual | `continue-on-error` *(advisory)*            |

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
cargo test -p pcloud-daemon --lib -- --nocapture
```

Current count: **1247 passing**. Expect 5–10 new unit tests per new
feature; commands missing unit coverage fail review.

**CI gate:** runs on every PR. A single failure blocks merge.

## 2. Property Tests

We use `proptest` to generate inputs and assert invariants. The workspace
currently ships **seven properties** with **128 cases each**, split
between two crates:

### `pcloud-ipc` — `proptest_framer` (4 properties)

- length-prefix framer round-trips for arbitrary payload lengths,
- decoding tolerates arbitrary chunk-boundary splits,
- oversize payloads are rejected deterministically,
- idempotent re-encode after a round-trip.

### `pcloud-crypto` — `proptest_seal` (3 properties)

- `SecretBytes` + `SealedBox` round-trip preserves plaintext,
- single-bit tamper detection fires deterministically,
- empty and max-length plaintexts encode and decode cleanly.

**Run locally:**

```sh
cargo test -p pcloud-ipc    -- proptest_framer
cargo test -p pcloud-crypto -- proptest_seal
```

**Deep-dive with more cases:**

```sh
PROPTEST_CASES=10000 cargo test -p pcloud-ipc -- proptest_framer
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

**Run locally** (nightly toolchain required):

```sh
rustup toolchain install nightly
cd crates/pcloud-ipc
cargo +nightly fuzz run fuzz_ipc_frame -- -max_total_time=300
```

Corpora live under `<crate>/fuzz/corpus/<target>/` and are cached by CI.
When a crash is found, minimize with `cargo fuzz tmin` and commit the
reproducer as a regression seed.

**CI gate (current):** nightly job, `continue-on-error: true` *(advisory)*.
A crash generates an artifact for human triage but does not block PRs.
Tracked under `bd-1du.10` for hardening.

## 4. Mutation Testing

`cargo-mutants` mutates the source, re-runs the test suite, and flags any
mutation the tests did not catch. A surviving mutation is direct evidence
that a branch is not actually exercised, even if line coverage shows
green.

**Scope:** 4 crates on the weekly schedule:

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
cargo install cargo-mutants
cargo mutants -p pcloud-crypto --timeout 60
```

Triage surviving mutations into beads tagged `test-gap`.

## 5. Chaos Testing

`crates/pcloud-chaos/` exercises the daemon under **adversarial
environmental conditions**. Five scenarios ship:

### Default-run (every CI trigger)

1. **Network blackhole** during streaming download — verify resume tokens
   and idempotent retry recover cleanly.
2. **Clock jump** (host clock jumps ±24 h mid-request) — verify auth
   token refresh and cache expiry survive.

### Opt-in (`PCLOUD_CHAOS=1`)

3. **SIGKILL** mid-upload — verify journal rollback, no torn writes, no
   orphaned temp files on restart.
4. **Disk full** during writeback — verify graceful error surface and
   that the journal replays correctly after space is freed.
5. **Slowloris peer** (IPC client sends one byte per 5 s) — verify
   daemon timeouts fire and do not starve other peers.

The opt-in trio is heavy: each scenario serialises for minutes and
requires privileged hooks. They run in the **weekly** chaos job and are
skipped on normal PRs.

**Run locally (default two):**

```sh
cargo test -p pcloud-chaos --test scenarios -- --test-threads=1
```

**Run locally (full five):**

```sh
PCLOUD_CHAOS=1 cargo test -p pcloud-chaos --test scenarios -- --test-threads=1
```

Chaos tests deliberately serialise (`--test-threads=1`) because they
install process-wide signal handlers and fault injectors.

**CI gate (current):** *(not yet in CI)* — chaos tests are deferred due to
timing instability on shared GitHub runners. See the comment block in
`.github/workflows/ci.yml` for the deferred-chaos closure path.
Run locally with `PCLOUD_CHAOS=1` before cutting a release tag.

## 6. Coverage

We use `cargo-llvm-cov` to generate line-coverage reports.

**Target posture (intended, not yet enforced as a hard PR gate):**

- workspace floor: **65 %** (target: 80 % by `bd-1du.10` close)
- per-crate floors for security-sensitive crates:

| Crate              | Floor |
| ------------------ | ----- |
| `pcloud-crypto`    | 85 %  |
| `pcloud-auth`      | 85 %  |
| `pcloud-resilience`| 80 %  |
| `pcloud-secret`    | 90 %  |
| `pcloud-ipc`       | 80 %  |

**CI gate (current):** coverage runs weekly, `continue-on-error: true`
*(advisory)*. No per-PR gate or ratcheting floor enforcement exists yet.
The job publishes an LCOV artifact for human inspection. Tracked under
`bd-1du.10` to agree on thresholds and flip to a hard gate.

**Run locally:**

```sh
cargo install cargo-llvm-cov
cargo llvm-cov --workspace --summary-only
cargo llvm-cov --workspace --html     # browse target/llvm-cov/html/
```

## 7. Live End-to-End Tests

`crates/pcloud-live-e2e/` contains tests that require a real pCloud
account. Every test function carries `#[ignore]` and a runtime gate
(`PCLOUD_LIVE_E2E=1`), so a plain `cargo test` never runs them.

**CI gate (current):** runs weekly and on manual dispatch,
`continue-on-error: true` *(advisory)*. Does NOT block PRs today.
Some families (crypto, sharing, FUSE, fleet) require additional env vars;
runs without those vars soft-skip the relevant tests — see
`crates/pcloud-live-e2e/README.md`. Tracked under `bd-1du.10` to promote
to a protected singleton gate for release candidates.

```sh
export PCLOUD_LIVE_E2E=1
export PCLOUD_E2E_USERNAME=staging-bot@example.com
export PCLOUD_E2E_PASSWORD=…          # use a vaulted env
export PCLOUD_E2E_TFA_SECRET=…        # optional, for TFA scenarios

cargo test -p pcloud-live-e2e --locked
```

The staging account credentials live in the release team's 1Password
vault. **Never** wire a personal account to CI.

**CI gate:** runs only on release candidate tags (`rc-*`) and on PRs
explicitly labelled `live-e2e`. A failure blocks the release tag.

## Running Everything Locally

Before a big PR, run the full pre-merge gate:

```sh
cd .
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test   --workspace --locked
cargo llvm-cov --workspace --summary-only
cargo audit --deny warnings
cargo deny check
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
