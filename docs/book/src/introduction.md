# Introduction

> **TL;DR** — `pcloud-rs` is a from-scratch Rust rewrite of the legacy C/C++
> pCloud console client. It is **not** yet a drop-in replacement. Parity is
> tracked row-by-row; the authoritative count lives in
> [`STATUS.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/STATUS.md).
> This handbook documents what the Rust path actually does today, not what we
> hope it will do by the next release.

## What this project is

`pcloud-rs` is the next generation of the pCloud console client. This fork
contains the Rust workspace only: typed protocol clients, a daemon/runtime
split, secure local IPC, SQLite persistence, mounted-drive adapters, and two
deliberately separate Rust SDK surfaces. The removed legacy C/C++ client is
available from the upstream `pcloudcc` repository and is used only as a
behavioural/parity reference.

The rewrite exists because the C codebase had accumulated years of
hand-rolled concurrency, leak-prone allocations, and security defaults that
no longer match enterprise expectations. Rust lets us keep the network
protocol behaviour bit-for-bit compatible while tightening the memory,
secret-handling, and IPC model — without having to prove manual correctness
on every pointer.

The client executable is **`pcloudc`** and the daemon executable is
**`pcloudd`**.

## What this project is *not*

The retained C capability matrix is functionally complete, but native release
qualification, clean-baseline integration, SDK publication, and credentialed
live tests remain open. Until those gates close we do **not** describe the
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
| **T1** | Linux, macOS, Windows, FreeBSD, NetBSD, OpenBSD, DragonFly BSD, illumos/OmniOS, Solaris | Native release-commit gates are release-blocking. Solaris-family targets qualify the portable library/CLI/API surface; kernel mounting is explicitly unsupported there. |
| **T2** | Synology DSM, QNAP QTS/QuTS, ASUSTOR ADM | Candidate packages plus vendor-hardware install, upgrade, reboot, transfer, and uninstall qualification. |

A T1 regression blocks a release. Tier 2 remains explicitly unqualified until
the vendor-hardware matrices pass. Mounted-drive implementations are FUSE on
Linux/BSD, fuse-t on macOS, and WinFSP on Windows. A workflow definition or a
locally built package is not native-platform qualification evidence.

## High-level architecture

```
 +------------------+        +-------------------------------+
 |  pcloudc (CLI)   |        |  pcloud-sdk 1.x               |
 |  interactive +   |        |  blocking RemoteDrive client |
 |  --json scripts  |        |                               |
 +--------+---------+        +---------------+---------------+
          | local IPC                        | local IPC
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
composable: `pcloud-proto` owns the pCloud wire format, backends own side
effects, the daemon owns lifecycle and secrets, and the CLI/public SDK are two
front doors to the same owner-authenticated IPC surface. The broad historical
in-process API still exists separately as the unpublished
`pcloud-embedded-sdk`; it is a first-party compatibility surface, not the
stable third-party SDK.

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

Read the full [security model](security/model.md) and the rationale for each
rejected legacy behaviour in the parity matrix.

## How to read this book

The chapters are ordered for a first-time user working through a full
install, login, and sync. If you are a returning reader:

- **Operators** — use the [runbook](operations/runbook.md) for service and
  incident procedures.
- **Packagers and distributors** — see the
  [packaging reference](reference/packaging.md) for binary layout, file modes,
  and current publication status.
- **SDK consumers** — [SDK reference](reference/sdk.md) documents the focused
  `pcloud-sdk` 1.x contract and its daemon requirement.
- **Contributors** — [Crate map](architecture/crate-map.md) covers the crate
  layout; [Adding a command](development/adding-a-command.md) covers the
  request workflow.

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
- [`STATUS.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/STATUS.md)
  — the single source of truth for "is feature X done yet?".
- [Architecture Decision Records](adr/index.md) — why the
  rewrite made the shape it did.
