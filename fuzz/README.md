# Fuzzing pcloud-rs (Rust)

Nightly fuzzing is wired up by the `fuzz` job in
[`.github/workflows/rust.yml`](../../.github/workflows/rust.yml). The job runs
daily at 02:00 UTC (and on manual `workflow_dispatch`), discovers every
`cargo-fuzz` target under `**/fuzz/fuzz_targets/*.rs`, and executes
each for up to 10 minutes (`-max_total_time=600`). Corpora are persisted
between runs via `actions/cache@v4` and crash artifacts are uploaded and
filed as GitHub issues automatically.

This document captures the conventions for running and maintaining fuzz
targets locally.

## Where fuzz targets live

The `cargo-fuzz` harnesses live next to the crate they exercise:

- `crates/pcloud-proto/fuzz/fuzz_targets/*.rs`
- `crates/pcloud-ipc/fuzz/fuzz_targets/*.rs`

Each `fuzz` directory is a standalone cargo package with its own
`Cargo.toml`. Do not edit those files as part of CI/hygiene work unless the
change is scoped to a specific target.

## Where corpora live

Corpora are stored alongside each fuzz project, keyed by target name:

```
<crate>/fuzz/corpus/<target>/
```

For example:

```
crates/pcloud-proto/fuzz/corpus/fuzz_json_response/
crates/pcloud-ipc/fuzz/corpus/fuzz_ipc_frame/
```

CI restores and re-saves this tree across runs, so newly discovered inputs
accumulate automatically. Do not commit corpus files to git — they are
cached by CI and regenerated locally.

## Running a target locally

From the workspace root:

```bash
cd .
cargo fuzz run <target>
```

`cargo-fuzz` will locate the nearest `fuzz/` project. If you need to point
at a specific harness explicitly (for example when multiple crates define
the same target name), use `--fuzz-dir`:

```bash
cargo fuzz run <target> --fuzz-dir crates/pcloud-proto/fuzz
```

To cap a local run the same way CI does (10 minutes per target):

```bash
cargo fuzz run <target> -- -max_total_time=600
```

`cargo-fuzz` requires a nightly toolchain. Install one if you have not yet:

```bash
rustup toolchain install nightly --profile minimal
```

## Investigating a crash

When libFuzzer finds an input that panics, aborts, leaks, or times out, it
writes the reproducer into the fuzz project directory as `crash-*`,
`leak-*`, `timeout-*`, or `oom-*`. Re-run the target against that file to
reproduce deterministically:

```bash
cargo fuzz run <target> path/to/crash-<hex>
```

## Minimizing a new crash

Before filing a bead or a regression test, shrink the crashing input to
the smallest equivalent reproducer:

```bash
cargo fuzz tmin <target> <crash-file>
```

`tmin` will iteratively rewrite the crash file in place with a smaller
input that still triggers the same failure. Commit the minimized
reproducer as a regression seed (for example under
`<crate>/fuzz/corpus/<target>/`) so future runs always exercise it.

## Hygiene checklist

- Keep corpora small; if a target's corpus exceeds a few thousand inputs
  run `cargo fuzz cmin <target>` to coverage-minimize it.
- Do not silently delete crash files; either fix the bug and add a
  regression, or move the file into a `known-failures/` subtree with a
  tracking bead reference.
- Do not loosen the `-max_total_time` budget in CI without updating
  `PLAN_A_PLUS.md` and the fuzz job comment.
