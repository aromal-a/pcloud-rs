# Fuzzing pcloud-rs (Rust)

Nightly fuzzing is wired up by the `fuzz` job in
[`.github/workflows/fuzz.yml`](../.github/workflows/fuzz.yml). The job runs
daily at 02:00 UTC (and on manual `workflow_dispatch`). Targets are enumerated
explicitly in the workflow matrix (not auto-discovered); when you add a new
`fuzz_targets/*.rs` file, add its name to the corresponding matrix list in
`fuzz.yml`. Corpora are persisted between runs via `actions/cache@v4`. Crash
artifacts are NOT automatically filed as GitHub issues — triage them manually
by following the steps below.

> **Note (2026-04-26):** `continue-on-error: true` is set on all fuzz jobs.
> A libFuzzer crash is a finding to triage, not broken infrastructure.
> A human must review uploaded crash artifacts and file a bead before the
> next release tag is cut.

This document captures the conventions for running and maintaining fuzz
targets locally.

## Where fuzz targets live

The `cargo-fuzz` harnesses live next to the crate they exercise:

- `crates/pcloud-proto/fuzz/fuzz_targets/*.rs`
- `crates/pcloud-ipc/fuzz/fuzz_targets/*.rs`
- `crates/pcloud-crypto/fuzz/fuzz_targets/*.rs`
- `crates/pcloud-daemon/fuzz/fuzz_targets/*.rs`

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
cargo +nightly fuzz run --fuzz-dir <crate>/fuzz <target>
```

There is no workspace-root `fuzz/Cargo.toml`; always point at the crate-local
fuzz package or `cd` into the crate first. Examples:

```bash
cargo +nightly fuzz run --fuzz-dir crates/pcloud-ipc/fuzz fuzz_ipc_frame -- -max_total_time=300
cargo +nightly fuzz run --fuzz-dir crates/pcloud-crypto/fuzz fuzz_open_sector -- -max_total_time=300
cargo +nightly fuzz run --fuzz-dir crates/pcloud-daemon/fuzz fuzz_auth_vault_decode -- -max_total_time=300
cargo +nightly fuzz run --fuzz-dir crates/pcloud-proto/fuzz fuzz_json_response -- -max_total_time=300
```

Current CI target list:

- `crates/pcloud-ipc`: `fuzz_ipc_frame`
- `crates/pcloud-crypto`: `fuzz_open_sector`, `fuzz_pclsync_filename_decode`
- `crates/pcloud-daemon`: `fuzz_auth_vault_decode`
- `crates/pcloud-proto`: `fuzz_auth_flow_state`,
  `fuzz_binary_request_roundtrip`, `fuzz_ipc_method_decode`,
  `fuzz_json_response`, `fuzz_path_canonicalize`,
  `fuzz_response_parser`, `fuzz_listfolder_response`

To cap a local run the same way CI does, pass `-max_total_time=300`.

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
cargo +nightly fuzz run --fuzz-dir <crate>/fuzz <target> path/to/crash-<hex>
```

## Minimizing a new crash

Before filing a bead or a regression test, shrink the crashing input to
the smallest equivalent reproducer:

```bash
cargo +nightly fuzz tmin --fuzz-dir <crate>/fuzz <target> <crash-file>
```

`tmin` will iteratively rewrite the crash file in place with a smaller
input that still triggers the same failure. Commit the minimized
reproducer as a regression seed (for example under
`<crate>/fuzz/corpus/<target>/`) so future runs always exercise it.

## Hygiene checklist

- Keep corpora small; if a target's corpus exceeds a few thousand inputs
  run `cargo +nightly fuzz cmin --fuzz-dir <crate>/fuzz <target>` to
  coverage-minimize it.
- Do not silently delete crash files; either fix the bug and add a
  regression, or move the file into a `known-failures/` subtree with a
  tracking bead reference.
- Do not loosen the `-max_total_time=300` budget in CI without updating
  `PLAN_A_PLUS.md` and the fuzz job comment.
