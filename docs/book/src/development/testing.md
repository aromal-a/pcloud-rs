# Testing

The workspace runs a **seven-layer testing pyramid**. Every layer gates on
CI; every layer is runnable locally with the commands below. The goal is
not "more tests" — it is **different classes of evidence** that the code is
correct. A unit test proves a branch is taken. A property test proves an
invariant holds over thousands of inputs. A fuzz target proves no adversary
input causes a panic or memory error. A mutation run proves the tests
actually catch a broken implementation.

## The Pyramid at a Glance

| Layer              | Count / scope                                  | Local cadence | CI gate                      |
| ------------------ | ---------------------------------------------- | ------------- | ---------------------------- |
| Unit tests         | **1247 passing** across the workspace          | Every change  | Every PR, blocking           |
| Property tests     | **7 properties × 128 cases** each              | Every change  | Every PR, blocking           |
| Fuzz targets       | 4 targets, cargo-fuzz, 10 min / target         | Nightly       | Nightly CI, blocking         |
| Mutation testing   | `cargo-mutants`, 4 crates, **75 % MMR floor**  | Manual / weekly | Weekly Sun 03:00 UTC       |
| Chaos scenarios    | **5 scenarios** in `pcloud-chaos`              | Manual        | Weekly + opt-in env flag     |
| Coverage           | `cargo-llvm-cov`, **65 % → 80 %** ratchet      | Every change  | Every PR, blocking           |
| Live E2E           | Tag-gated against real account                 | Pre-release   | Release candidates only      |

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
Current targets:

- `pcloud-ipc/fuzz/fuzz_targets/framer.rs` — IPC decoder
- `pcloud-proto/fuzz/fuzz_targets/response_parser.rs` — server response JSON
- `pcloud-crypto/fuzz/fuzz_targets/sealed_box.rs` — sealed-box decoder
- `pcloud-fs/fuzz/fuzz_targets/journal.rs` — journal replay

**Run locally** (nightly toolchain required):

```sh
rustup toolchain install nightly
cargo +nightly fuzz run framer -- -max_total_time=300
```

Corpora live under `fuzz/corpus/<target>/` and are seeded from real
payloads. New panics or OOMs are automatically minimised and filed as a
bead tagged `fuzz-finding` with the reproducer attached.

**CI gate:** nightly job runs each target for **10 minutes**. A new crash
or slow unit blocks the next release tag until the bead is triaged.

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

**Floor:** **75 % mutants must be caught** per crate (the Minimum
Mutation Ratio — MMR). A drop below floor blocks the weekly job and
opens a triage bead.

**Schedule:** weekly, **Sundays at 03:00 UTC**, via a dedicated GitHub
Actions workflow. The run takes ~4 hours across the four crates.

**Run locally on a single crate:**

```sh
cargo install cargo-mutants
cargo mutants -p pcloud-crypto --timeout 60
```

CI uploads `mutants.out/` as an artefact. Triage surviving mutations into
beads tagged `test-gap`.

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

**CI gate:** default two scenarios gate nightly. Full five gate weekly.

## 6. Coverage

We use `cargo-llvm-cov` with a **ratcheting floor**:

- **current floor: 65 %** (line coverage, workspace-wide)
- **target: 80 %** by the time `bd-1du.10` closes
- the floor **ratchets upward** — PRs that lower it are blocked

### Per-crate floors

Security-sensitive crates carry tighter per-crate floors that override the
workspace number:

| Crate              | Floor |
| ------------------ | ----- |
| `pcloud-crypto`    | 85 %  |
| `pcloud-auth`      | 85 %  |
| `pcloud-resilience`| 80 %  |
| `pcloud-secret`    | 90 %  |
| `pcloud-ipc`       | 80 %  |

A drop in any of these blocks the PR regardless of the workspace number.

**Run locally:**

```sh
cargo install cargo-llvm-cov
cargo llvm-cov --workspace --summary-only
cargo llvm-cov --workspace --html     # browse target/llvm-cov/html/
```

**CI gate:** a custom script compares the PR's coverage to `main`'s and
to `ci/coverage-floor.toml`. The floor never lowers automatically —
increases land as explicit commits updating `ci/coverage-floor.toml`
with a changelog entry.

## 7. Live End-to-End Tests

`crates/pcloud-live-e2e/` contains tests that require a real pCloud
account. These are **tag-gated**: they do not run on PRs unless the
PR carries the `live-e2e` label, and they always run on release
candidates.

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
