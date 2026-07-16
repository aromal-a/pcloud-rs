# Local CI/CD

GitHub Actions is intentionally inactive. The authoritative pipeline is the
repository-owned `xtask`:

```bash
cargo xtask ci
```

The historical workflow YAML is retained under
`.github/workflows-disabled/`; GitHub only loads YAML from
`.github/workflows/`, which contains documentation and no active workflow.

## Stages

| Command | Purpose |
|---|---|
| `cargo xtask preflight` | Rust/tool availability and GitHub-disablement checks |
| `cargo xtask compat` | Rust 1.89/1.91 MSRV and optional-feature checks |
| `cargo xtask host` | format, compile, Clippy, tests, rustdoc, books, audit/deny |
| `cargo xtask coverage` | instrumented tests and the strict greater-than-90% line gate |
| `cargo xtask package` | NAS, portable Unix, SDK package, and metadata checks |
| `cargo xtask docker` | OCI build/smoke plus Debian portability compilation |
| `cargo xtask windows` | dirty-tree transfer and native Windows tests over SSH |
| `cargo xtask release` | full CI plus reproducible release binaries |

The normal and release toolchain is pinned to Rust 1.96.1. The explicit Rust
1.89 portable-core and Rust 1.91 Wasmtime MSRV checks are compatibility
contracts, not the toolchain used for ordinary builds.

The coverage stage requires line coverage strictly above 90%. The verified 2026-07-16
full-workspace result is 82.65% (`77,060 / 93,234` lines), so
`cargo xtask ci` intentionally remains red until the LCOV report exceeds the
threshold; see `docs/coverage.md`.

## Windows

The Windows stage uses key-only SSH for source transfer and cleanup, then a
credential-bearing SSH session for execution because Windows CurrentUser
DPAPI returns `ERROR_ACCESS_DENIED` in public-key-only OpenSSH logons. Set
`PCLOUD_CI_WINDOWS_PASSWORD` in the calling process environment; it is passed
to `sshpass` through its environment and is never written to the repository
or placed in a command argument. The stage transfers the current working tree
(including uncommitted and untracked source files, excluding build output) to
an isolated directory, installs Rust 1.96.1 with rustup, and runs:

- formatting, all-target check, and warnings-as-errors Clippy;
- workspace tests and the portable filesystem suites;
- native daemon/CLI build;
- real per-user named-pipe startup, status, shutdown, and clean-exit smoke;
- the live WinFSP mount test when WinFSP is installed.

Configuration is supplied through `PCLOUD_CI_WINDOWS_HOST`,
`PCLOUD_CI_WINDOWS_USER`, `PCLOUD_CI_WINDOWS_KEY`, and
`PCLOUD_CI_WINDOWS_ROOT` (an isolated run directory is created below this
base), plus `PCLOUD_CI_WINDOWS_PASSWORD` for the DPAPI-capable execution
session. The password is required at runtime but is not persisted.

## Docker and target scope

Docker proves Linux OCI, glibc, and musl behavior. Windows is tested on a real
Windows host. Docker cannot emulate macOS kernel/system-extension behavior or
turn a Linux kernel into BSD, illumos, or Solaris; those targets still require
native VMs or hosts before a release may claim qualification. Cross-compilation
is useful as an additional compile gate but is not substituted for native
runtime, mount, packaging, signing, or upgrade evidence.

## Partial runs

`PCLOUD_CI_SKIP_DOCKER=1` and `PCLOUD_CI_SKIP_WINDOWS=1` are available for
developer iteration. An invocation using either flag is explicitly partial
and is not release evidence.
