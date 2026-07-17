# Verification and release evidence

## Local quality gates

Typical source gates are:

```bash
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo doc --workspace --no-deps --locked
cargo audit
cargo deny check
```

The exact CI workflow may add feature combinations, fuzzing, coverage,
packaging, license, dependency, reproducibility, or native jobs.

## What each gate proves

| Gate | Proves | Does not prove |
|---|---|---|
| check/clippy | current target compiles cleanly | native behavior on other OSes |
| unit/integration tests | encoded invariants under test fixtures | real pCloud account behavior |
| mock server | protocol behavior for scripted replies | production server compatibility |
| live E2E | selected real-account behavior | all packages/platforms |
| kernel mount test | adapter works on that kernel/config | signed installers or other OSes |
| package install test | artifact lifecycle on that image | hardware/NAS variants |
| reproducible build | same inputs produce same bits under tested environment | signature trust or runtime correctness |
| signing/notarization | artifact identity/platform acceptance | application correctness |
| registry install test | published SDK dependency chain resolves | daemon/native support |

## Atlas validation

The documentation generator fails if Cargo metadata cannot be read. `mdbook
build` validates chapter paths and renders the complete site. A separate link
checker in `tools/check_links.py` checks internal Markdown targets before
serving. `tools/check_feature_coverage.py` additionally proves that every
current Cargo package and flag has explicit rationale, every canonical API
matrix row is rendered, every package-owned Rust unit is cataloged, and every
feature chapter remains in navigation.

```bash
python3 docs/architecture-atlas/tools/generate.py
python3 docs/architecture-atlas/tools/check_feature_coverage.py
python3 docs/architecture-atlas/tools/check_links.py
mdbook build docs/architecture-atlas
```

## Source snapshot note

Generated pages include the current Git HEAD, worktree file count, generation
time, and a warning when the tree is dirty. That makes the atlas traceable but
does not make a dirty snapshot a release baseline.
