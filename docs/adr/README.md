# Architecture Decision Records

This directory holds Architecture Decision Records (ADRs) for the
`pcloud-rs` Rust rewrite under ``. Each ADR captures a single
non-trivial decision, its rationale, and its alternatives. The format
itself is defined in ADR 0001.

Status values in use: `Accepted`, `Proposed`, `Superseded by ADR NNNN`,
`Deprecated`. Never rewrite an ADR in place to change the decision;
write a new one and supersede the old.

## Index

- [0001 — Record Format](0001-record-format.md) — Accepted. Defines the
  template, file-naming rules, and status lifecycle for all ADRs.
- [0002 — IPC Socket Framing](0002-ipc-socket-framing.md) — Accepted.
  Binary length-prefixed frames over `AF_UNIX`, not JSON-over-HTTP.
- [0003 — Sync Mutex Choice](0003-sync-mutex-choice.md) — Accepted.
  `parking_lot::Mutex`/`RwLock` workspace-wide: poison-free and faster
  on the uncontended path.
- [0004 — Panic Guard Default-On](0004-panic-guard-default-on.md) —
  Accepted. `catch_unwind` at the IPC dispatch boundary is
  unconditional; documents what is and is not caught.
- [0005 — Token Vault Layout](0005-token-vault-layout.md) — Accepted.
  `0600` file under a `0700` directory, atomic writes, and opt-in
  durability via `PCLOUD_DURABLE_AUTH_TOKENS=1`.
- [0006 — No Update Check](0006-no-update-check.md) — Accepted. The
  rewrite deliberately does not mirror `psync_check_new_version*`;
  distro channels own updates.
- [0007 — Crypto Password Not Persisted](0007-crypto-password-not-persisted.md) —
  Accepted. A retained-security carve-out against the C behaviour:
  crypto passwords live only in memory.
- [0008 — Streaming Download Buffer Size](0008-streaming-download-buffer-size.md) —
  Accepted. 64 KiB buffer on the streaming copy loop, justified
  against syscall count, memory footprint, and TLS record size.
- [0009 — Parity Matrix Truth Source](0009-parity-matrix-truth-source.md) —
  Accepted. `STATUS.md` is authoritative for aggregate parity claims;
  `CLAUDE.md` and `README.md` are consumers.
- [0010 — FUSE Write-Path Daemon Wiring Pending](0010-fuse-write-path-daemon-wiring-pending.md) —
  Superseded by ADR 0020. Records the earlier open mounted-drive write
  questions.
- [0011 — Daemon vs Library-Only](0011-daemon-vs-library-only.md) —
  Accepted. Long-lived `pcloudd` owns state; CLI, SDK, web, and
  plugins talk to it over IPC. Enterprise surfaces attach at the
  daemon boundary, not per-client.
- [0012 — `RequestEnvelope` for `traceparent`](0012-traceparent-envelope-wrapper.md) —
  Accepted. Tracing propagation added via a wire-compatible envelope
  wrapper rather than rippling `traceparent` through every
  `Request::*` variant. `try_from_wire` preserves pre-envelope peers.
- [0013 — OPA Rego via `regorus`](0013-opa-rego-via-regorus.md) —
  Accepted. Policy evaluation uses a pure-Rust Rego engine with
  default-deny, file-perm guard, and transactional hot-reload —
  instead of a custom DSL, CGO, or an OPA subprocess.
- [0014 — Hand-Rolled OIDC Broker](0014-hand-rolled-oidc-broker.md) —
  Accepted. `pcloud-idp` ships a hand-rolled PKCE S256 broker for
  pre-alpha control over algorithm policy and secret wrapping; the
  `openidconnect` crate is reconsidered post-`bd-1du.10`.
- [0015 — `0600` Vault Permission Enforcement](0015-vault-0600-permission-enforcement.md) —
  Accepted. Enforces `0600` file / `0700` parent / owner-match /
  atomic writes on every secret-bearing file, at both write and load
  time. Generalises ADR 0005.
- [0016 — Secret-Wrapping Discipline](0016-secret-wrapping-discipline.md) —
  Accepted. Project-wide rule for `SecretString` / `SecretBytes`
  usage, `Debug` redaction, audit/trace redaction, and
  zeroise-on-drop guarantees.
- [0017 — JSON-in-`message` Response Shape](0017-json-in-message-response-shape.md) —
  Accepted. Structured IPC responses embed a JSON payload in the
  existing `message` field to avoid a wire revision; CLI `--json`
  emits it verbatim.
- [0018 — Native Field Selector (Not `jq`)](0018-native-field-selector-syntax.md) —
  Accepted. `pcloudc --select` ships a small native selector grammar
  so scripted operators don't need `jq` installed, on any platform.
- [0019 — IPC Serve Loop Is Single-Threaded](0019-ipc-serve-loop-single-threaded.md) —
  Accepted. Serial daemon dispatch remains intentional until the runtime owns
  only `Send`-safe handles or telemetry justifies an actor boundary.
- [0020 — FUSE Write Durability and Bounded Staging](0020-fuse-write-durability.md) —
  Accepted. `fsync` is server-durable, staging ceilings return `ENOSPC`, and
  both FUSE compositions share one resumable write service.
