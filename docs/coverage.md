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

The verified local run on 2026-07-16 passed at **90.09%**
(`85,716 / 95,146` lines). This is a complete `cargo +1.96.1 xtask coverage`
run, including the opt-in real Linux FUSE suites and deterministic Unix chaos
tests. The generated `target/xtask/lcov.info` is the auditable source for the
measurement.

## Security-critical floors

The shared parser also enforces:

| Crate | Verified coverage | Line floor |
|---|---:|---:|
| `pcloud-secret` | 100.00% (`51 / 51`) | 90% |
| `pcloud-crypto` | 92.94% (`4,820 / 5,186`) | 85% |
| `pcloud-auth` | 96.22% (`1,094 / 1,137`) | 85% |
| `pcloud-resilience` | 89.85% (`1,735 / 1,931`) | 80% |
| `pcloud-ipc` | 87.17% (`1,121 / 1,286`) | 80% |

The workspace or per-crate floors must never be lowered automatically to make
a release pass.
