# Workspace coverage

Coverage is owned by the local `xtask` pipeline. GitHub Actions is disabled;
the archived workflow under `.github/workflows-disabled/coverage.yml` is
migration history and is not authoritative.

## Run the gate

```bash
cargo install cargo-llvm-cov
rustup component add llvm-tools-preview
cargo xtask coverage
```

The command:

1. removes stale coverage profiles;
2. runs the locked workspace tests on Rust 1.96.1, excluding only `xtask`
   itself because it is build orchestration rather than shipped product code;
3. adds the opt-in real Linux FUSE suites and deterministic Unix chaos tests;
4. writes `target/xtask/lcov.info`;
5. requires workspace line coverage **strictly above 90%** plus the security-critical
   per-crate floors in `scripts/coverage-check.sh`.

Test, benchmark, example, fuzz-target, and build-script source files are
omitted from the report. Product crates, including the daemon, CLI, SDK,
backends, filesystem, web API, and platform code compiled on the coverage
host, remain in scope.

## Current working-tree measurement

The verified local run on 2026-07-16 measured **82.65%**
(`77,060 / 93,234` lines). The policy therefore fails by design until at
least 83,911 lines are covered (6,851 more at the current denominator). Do
not describe the greater-than-90% target as passing until a fresh
`cargo xtask coverage` exits successfully.

The largest current crate-level gaps are:

| Crate | Uncovered / instrumented lines | Coverage |
|---|---:|---:|
| `pcloud-daemon` | 4,192 / 17,413 | 75.93% |
| `pcloud-cli` | 2,758 / 10,836 | 74.55% |
| `pcloud-fs` | 2,071 / 8,822 | 76.52% |
| `pcloud-backends` | 1,639 / 11,215 | 85.39% |
| `pcloud-sdk` | 981 / 3,272 | 70.02% |

These are concentrated in daemon orchestration, CLI execution/parsing,
filesystem platform/backend adapters, the embedded SDK, and remote backend
error paths. The LCOV file is the auditable source for prioritising tests.

## Security-critical floors

The shared parser also enforces:

| Crate | Line floor |
|---|---:|
| `pcloud-secret` | 90% |
| `pcloud-crypto` | 85% |
| `pcloud-auth` | 85% |
| `pcloud-resilience` | 80% |
| `pcloud-ipc` | 80% |

The workspace or per-crate floors must never be lowered automatically to make
a release pass.
