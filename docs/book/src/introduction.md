# Introduction

> **TL;DR** — `pcloud-rs` is a from-scratch Rust rewrite of the legacy C/C++
> pCloud console client. It is **not** yet a drop-in replacement. Parity is
> tracked row-by-row; the authoritative count lives in
> [`STATUS.md`](https://github.com/pcloudcom/pcloud-rs/blob/main/STATUS.md).
> This handbook documents what the Rust path actually does today, not what we
> hope it will do by the next release.

## What this project is

`pcloud-rs` is the next generation of the pCloud console client. Two
code trees live side-by-side in the repository:

- **Legacy C/C++** — `main.cpp`, `pclsync_lib.cpp`, `pclsync/`. Retained for
  capability auditing and as a behavioural reference while the Rust rewrite
  catches up on the long tail of features. New development on this tree is
  limited to security fixes.
- **Rust workspace** — `pcloud-rs`. The forward-looking implementation:
  typed protocol clients, a daemon/runtime split, secure local IPC, SQLite
  persistence, and an embeddable SDK. All new features land here.

The rewrite exists because the C codebase had accumulated years of
hand-rolled concurrency, leak-prone allocations, and security defaults that
no longer match enterprise expectations. Rust lets us keep the network
protocol behaviour bit-for-bit compatible while tightening the memory,
secret-handling, and IPC model — without having to prove manual correctness
on every pointer.

The binary you will actually run is called **`pcloudc`** (short, no extra
`c`). The daemon is **`pcloud-daemon`**. The legacy C binary was
`pcloud-rs`; we keep the old name as a transparent alias on Unix during the
transition.

## What this project is *not*

The Rust tree is **substantially complete** but the final parity-proof
beads are still open (`bd-1du.4` filesystem/mount parity, `bd-1du.10`
parity-proof gate). Until those are closed we do **not** describe the
Rust path as:

- "full parity",
- "production ready",
- "enterprise ready",
- "drop-in replacement".

If a doc, release note, or marketing blurb ever uses one of those phrases,
cross-check it against `STATUS.md` and
`C_FEATURE_PARITY_MATRIX.csv`. If the matrix disagrees, the
matrix wins. When in doubt, file an issue — silent drift between docs and
reality is the failure mode we care most about preventing.

## Platform support tiers

| Tier | Platforms | Policy |
|------|-----------|--------|
| **T1** | Linux (glibc, x86_64 + aarch64), macOS 13+, Windows 10+ | CI-gated, packaged, release-blocking. |
| **T2** | FreeBSD 13+, Linux-musl (Alpine, static builds) | Built in CI, tested on best-effort, community-supported. |
| **T3** | OpenBSD, NetBSD, Windows 7/8 | Source-only. Patches welcome; breakage does not block a release. |

A T1 regression blocks a release. A T2 regression opens a tracker bead but
ships. A T3 regression is accepted if a fix is non-trivial — users on T3
platforms are expected to build from source and be comfortable debugging
their own environment. The mounted-drive experience is Linux-first (FUSE
via `fuse3`), macOS-second (`fuse-t`). Windows uses a separate
projected-filesystem (ProjFS) backend; that surface is still gated behind
`bd-1du.4`. BSD FUSE support exists but is T2/T3.

## High-level architecture

```
 +------------------+        +-------------------------------+
 |  pcloudc (CLI)   |        |  SDK / embedders              |
 |  interactive +   |        |  (Rust crate: pcloud-sdk)     |
 |  --json scripts  |        |                               |
 +--------+---------+        +---------------+---------------+
          | local IPC                        | in-process
          | Unix socket 0600                 |
          | SO_PEERCRED / ucred              |
          v                                  v
 +------------------------------------------------------------+
 |  pcloud-daemon                                             |
 |  +--------------------------------------------------------+|
 |  | runtime: request dispatcher, auth vault, audit log     ||
 |  +--------------+-----------------------------------------+|
 |                 v                                          |
 |  +----------+----------+----------+----------+-----------+ |
 |  |  auth    | transfer |  sync    |  crypto  | backup /  | |
 |  | backend  | backend  | backend  | backend  | shares /  | |
 |  |          |          |          |          | publink   | |
 |  +----+-----+----+-----+----+-----+----+-----+----+------+ |
 +-------+----------+----------+----------+----------+--------+
         v          v          v          v          v
                 +----------------------------------+
                 |  pcloud-proto (typed API client) |
                 |  TLS-mandatory in production     |
                 +---------------+------------------+
                                 v
                       pCloud API (eapi/api.pcloud.com)
```

Each box is a crate in the `pcloud-rs` workspace. Crates are thin and
composable: `pcloud-proto` owns the wire format, backends own side
effects, the daemon owns the lifecycle, and the CLI/SDK are just two
different front doors to the same IPC surface. The separation means you
can embed the daemon in another Rust program (via `pcloud-sdk`) without
ever starting an IPC socket, or you can drive a running daemon from any
language that can write a line-delimited JSON protocol.

## Security posture snapshot

The Rust rewrite is intentionally stricter than the C client on every
security dimension we could tighten without breaking the protocol:

- **Secrets** — `SecretString` / `SecretBytes` wrappers zeroize on drop
  and redact in `Debug`. Passwords and tokens do not live in plain
  `String` / `Vec<u8>` on long-lived structs. Secrets never reach logs,
  telemetry, or crash dumps.
- **Auth token persistence** — off by default. Turning it on requires
  `PCLOUD_DURABLE_AUTH_TOKENS=1` *and* an explicit CLI opt-in. The vault
  file is `0600` inside a `0700` parent; ownership and mode are
  re-validated on every read. Raw password persistence (a feature of the
  C client) is **not** mirrored and will not be, under any flag.
- **Local IPC** — Unix domain socket, `0600` mode, `0700` parent dir,
  peer UID checked via `SO_PEERCRED` (or `getpeereid` on BSD). Malformed
  clients are isolated, not tolerated. Slow-loris and malformed-frame
  attacks from local peers are handled as first-class threats.
- **Transport** — production config rejects plaintext. There is no
  "skip TLS" escape hatch. API-server overrides still go through
  validation. Certificate verification is non-negotiable.
- **Failure surfaces** — audit and persistence errors are raised on the
  active control path rather than being silently logged and dropped. If
  the audit log can't fdatasync, the daemon refuses to proceed.

Read the full model in [`SECURITY-MODEL.md`](../security/security-model.md)
and the rationale for each rejected legacy behaviour in
[`REJECTED-RATIONALES-14042026.md`](../archive/rejected-rationales.md).

## How to read this book

The chapters are ordered for a first-time user working through a full
install, login, and sync. If you are a returning reader:

- **Operators** — skip to the [Operations handbook](../operations/index.md)
  for runbooks, systemd units, and incident response.
- **Packagers and distributors** — see
  [Packaging notes](../packaging/index.md) for the binary layout, file
  modes, and post-install hooks we expect.
- **SDK consumers** — [SDK reference](../sdk/index.md) documents the
  `pcloud-sdk` crate and the in-process daemon embedding pattern.
- **Contributors** — [Development](../dev/index.md) covers the crate
  layout, parity matrix workflow, and how to add a new command without
  breaking the existing CLI surface.

Every code block in this handbook is copy-paste runnable on at least one
of the T1 platforms. Commands are annotated where behaviour differs
between Linux, macOS, and Windows.

## Where to go next

- [Installation](getting-started/install.md) — packages for every
  supported platform, plus verification.
- [First login](getting-started/first-login.md) — interactive flow,
  automation flags, and what to do when 2FA or the daemon socket
  misbehaves.
- [First sync](getting-started/first-sync.md) — add a sync root, watch
  progress, remove it cleanly.
- [`STATUS.md`](https://github.com/pcloudcom/pcloud-rs/blob/main/STATUS.md)
  — the single source of truth for "is feature X done yet?".
- [Architecture Decision Records](../architecture/adrs.md) — why the
  rewrite made the shape it did.
