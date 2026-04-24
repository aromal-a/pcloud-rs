# Windows Tier-3 → Tier-2 bring-up

_Session date: 2026-04-24. Commit range: `8b1c0fe..24fb5bf` (17 commits)._

## Promotion

- Before: Tier-3 scaffolded-only, compile-tested in CI only.
- After: Tier-2 — workspace compiles clean and `--lib` tests pass on a
  real Windows MSVC host.
- Not Tier-1: named-pipe IPC unwired, integration tests unrun, live
  WinFSP mount unexercised.

## Host

- Windows 10/11 x86_64
- MSVC 14.44
- Rust 1.95
- WinFSP 2.1.25156

## Compile gate

`cargo check --workspace` on Windows MSVC as of `24fb5bf`:
0 errors, 0 warnings.

Commits that unblocked compile (chronological):

- `89293de` — `pcloud-store`: cfg-gate Unix-only file-permission tightening for Windows
- `dd8ba71` — `pcloud-ipc` + `pcloud-fs`: unblock Windows cargo check
- `0d30a2b` — 2 residual Windows compile fixes
- `36faaa6` — Spectre-libs stub + missing-docs
- `b31d952` — wrap `GetProcAddress` in `unsafe{}` for Rust 2024
- `22a0e08` — unbreak `pcloud-cli` Windows compile; clear last `pcloud-fs` warning
- `3d11828` — Rust 2024 implicit-autoref lint in WinFSP volume-label copy
- `ed114f4` — `pcloud-web` web-token writer: cfg-gate Unix-only file mode
- `ad44ef2` — `pcloud-daemon` Windows compile pass
- `48e927d` — iteration 4: daemon cross-platform sweep residuals
- `ebf712b` — gate daemon serve-loop re-exports to Unix
- `d79004d` — `pcloudd` binary + `pcloud-daemon-win` `serve_with_shutdown` (no-op stub)
- `fb7147e` — gate Unix-only daemon tests + dpapi `SecretString::from`

## Unit-test gate

`cargo test --workspace --lib` on Windows: **1449 passing, 0 failing,
2 ignored** across 33 test binaries.

Commits that unblocked tests:

- `e13c890` — `.gitattributes` `eol=lf` + CRLF-safe source scan + HTTP body assert
- `88739da` — final 3 Windows test failures (see bug 2 below)
- `39bb035` — Windows health-server test: don't assert on body framing
- `24fb5bf` — `shutdown(Write)` before drop (see bug 1 below)

## Production-logic bugs surfaced and fixed

These are real bugs exposed by Windows, not Windows-only shims:

1. **`TcpStream::drop` FIN race in `pcloud-daemon::health_server`
   (commit `24fb5bf`).** On Windows the drop-implicit close could
   discard the HTTP response tail before the client read it. Fixed by
   calling `shutdown(Write)` explicitly before drop so the client sees
   the complete body before FIN.
2. **Hardcoded path separator in
   `pcloud-backends::mount_discovery::is_ignored_under`
   (commit `88739da`).** Nested-root classification used `/` only,
   which broke on Windows canonical `\\?\`-prefixed paths. Fixed to
   accept both `/` and `\`.

## Not yet verified

- `cargo test --workspace --tests` (integration suites) on Windows.
- Named-pipe IPC through `BoundIpcServer`: `serve_with_shutdown`
  returns `Unsupported` on Windows; `pcloudd-svc` is a no-op stub.
- Live WinFSP mount against a real pCloud account.
- `pcloudd-svc` Windows Service install/start/stop against a real SCM.

macOS posture unchanged — still Tier-3 scaffolded-only, unverified on
hardware.
