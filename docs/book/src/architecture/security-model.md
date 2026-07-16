# Architecture-Scoped Security Model

## Purpose

This page is the **architecture-scoped** security reference. It exists so an
architect or reviewer can read one page and know exactly which invariants the
daemon, the IPC channel, the vault, the audit hash-chain, and the proto
client enforce — and which test in the workspace proves each one. For the
broader threat-model and audit dossier, see the dedicated
[Security](../security/model.md) section; this page is the architecture
slice of that material.

All paths are relative to `` unless otherwise stated.

## If you're new to this codebase

The **thing to know**: security is concentrated in a small set of named
primitives, and the rest of the codebase is designed so that calling a
primitive wrong is *unnatural*. You do not add a `String` password to a
struct because the type you would reach for is `SecretString` and the
compiler directs you there. You do not log a token because the `Debug`
impl is redacted and `tracing` has a field-redaction layer on top. You
do not open the IPC socket world-readable because the platform trait
fixes the mode and the test suite fails a permissions assertion.

The four primitives are: `SecretString`/`SecretBytes` (secret containers),
the IPC peer-check (trust boundary), the vault envelope (durable secret
at rest), and the audit hash-chain (tamper-evident record of privileged
operations). Everything in this page is a consequence of one of those
four.

## High-level diagram

```
+--------------------------------------------------------------+
|                          client process                      |
|                                                              |
|   SecretString ---+   SecretBytes ---+                       |
|                   |                  |                       |
|                   v                  v                       |
|            +------+------+    +------+------+                |
|            | Request     |    | crypto args |                |
|            | (envelope)  |    | (never cli) |                |
|            +------+------+    +-------------+                |
|                   |                                          |
+-------------------|------------------------------------------+
                    | 0600 socket, 0700 dir, peer-cred gate
                    v
+-------------------------------------------------------------+
|                          daemon process                     |
|                                                             |
|   peer_identity() ---> catch_unwind() ---> dispatch()       |
|                                                             |
|   +----------------+   +------------------+   +---------+   |
|   | auth vault     |   | audit hash-chain |   | store   |   |
|   | 0600 / BLAKE3  |   | BLAKE3-linked    |   | WAL DB  |   |
|   +----------------+   +------------------+   +---------+   |
|         ^                       ^                  ^        |
|         |                       |                  |        |
|         +---- opt-in only ------+------- typed ----+        |
|                                                             |
|   +--------------------------------------------------+      |
|   |      rustls + TLS 1.3, no plaintext downgrade    |      |
|   +--------------------------------------------------+      |
+--------------------------------------|----------------------+
                                       v
                           binapi.pcloud.com:8398
```

## Enumerated security invariants

Each invariant is stated in imperative form, gives its enforcement site,
and names at least one test.

Citation verification: every test path listed below exists in-tree and
was re-validated 2026-04-16 when the citations were reconciled against the
actual test files. Invariants marked `[review-only]` are enforced by code
but not yet exercised from a userspace test — the enforcement site is
named instead of a test so a reviewer can audit the invariant directly.

IPC-boundary invariants are consolidated in
`crates/pcloud-ipc/tests/security_invariants.rs`; secret-container invariants
remain in the owning `pcloud-secret` crate so the publishable IPC package does
not acquire an unrelated dev-dependency.

### Secret containers

- **SEC-01** — Every secret-bearing long-lived field uses `SecretString`
  or `SecretBytes`.
  - Enforcement: `crates/pcloud-secret/src/{secret_string,secret_bytes}.rs`.
  - Tests: `crates/pcloud-secret/tests/redaction_and_zeroize.rs`,
    `crates/pcloud-secret/tests/serialize_is_forbidden.rs`
    (compile-fail guard against `Serialize` being re-derived).
- **SEC-02** — `Debug` for any secret type redacts contents.
  - Enforcement: hand-written `impl Debug` in `pcloud-secret`.
  - Tests: `crates/pcloud-secret/tests/redaction_and_zeroize.rs` and its
    property tests over arbitrary strings/bytes.
- **SEC-03** `[review-only]` — `tracing::event!` cannot log a secret field
  even if a caller tries.
  - Enforcement: the observability crate never accepts raw
    `SecretString` / `SecretBytes` as a tracing field (the types do not
    implement `Value`/`Display`), and the `redact_field` helper at
    `crates/pcloud-secret/src/redact.rs` is the only audit-sanctioned way
    to mention a secret by name.
  - Status: the dedicated subscriber-layer test is not yet wired. Call
    sites are currently guarded by the compile-time absence of `Display`/
    `Value` impls on the wrappers, which is verified by code review.
- **SEC-04** — A dropped secret is overwritten via `zeroize::Zeroize`.
  - Enforcement: `#[derive(ZeroizeOnDrop)]` on both wrappers; belt-and-
    braces hand-rolled `impl Zeroize` delegates to the inner buffer.
  - Tests: `crates/pcloud-secret/tests/redaction_and_zeroize.rs` and
    `crates/pcloud-secret/tests/proptest_zeroize_invariants.rs` (asserts
    `<Wrapper as Zeroize>::zeroize` empties the wrapper and that
    `expose_secret()` sees the scrubbed state).
  - Note: observing the memory content of a freed allocation is UB in
    safe Rust. The tests therefore verify the wrapper-level contract;
    the actual post-drop scrub is guaranteed by the `zeroize` crate's
    `ZeroizeOnDrop` derive which is the crate's own responsibility.

### IPC trust boundary

- **SEC-10** — The IPC socket is `0600` on a `0700` parent directory.
  - Enforcement: `crates/pcloud-ipc/src/transport.rs::IpcServer::bind`
    (lines 201–222: `set_permissions(parent, 0o700)` then
    `set_permissions(socket_path, 0o600)`).
  - Tests: `crates/pcloud-ipc/tests/security_invariants.rs::sec_10_ipc_socket_is_0600_on_0700_parent`
    (stat()s both paths after `bind`).
- **SEC-11** — Peer identity is verified (`SO_PEERCRED` / `LOCAL_PEERCRED` /
  SID comparison) before the first body byte is read.
  - Enforcement: `crates/pcloud-ipc/src/transport.rs`
    (`read_request_frame` / `authorize_peer` sequence).
  - Tests: `crates/pcloud-ipc/tests/peer_and_protocol.rs` (predicate
    coverage: non-owner / root-when-owner-is-user / owner-accept),
    `crates/pcloud-ipc/tests/security_invariants.rs::sec_11_*`
    (regression guard against the predicate drifting).
  - Note: a real cross-UID socket connection is not reproducible from a
    single-user test process without a sandbox. The kernel-level rule
    enforced by the 0600 mode at SEC-10 is the user-facing proof; the
    predicate tests above cover the authorization decision.
- **SEC-12** — IPC body size is capped at 1 MiB before allocation.
  - Enforcement: `MAX_IPC_PAYLOAD_LEN = 1024 * 1024` in
    `crates/pcloud-ipc/src/protocol.rs`, gate invoked by both the
    encoder and the transport read-frame path.
  - Tests: `crates/pcloud-ipc/tests/request_size_cap.rs`
    (transport-level: a 10 MiB declared length is rejected before any
    allocation, server stays up, follow-up client succeeds),
    `crates/pcloud-ipc/tests/security_invariants.rs::sec_12_*`
    (encoder-level: a 1.5 MiB payload fails to encode; constant pin).
- **SEC-13** — IPC protocol version is pinned and mismatches close the
  connection cleanly.
  - Enforcement: `IPC_PROTOCOL_VERSION = 1` in
    `crates/pcloud-ipc/src/protocol.rs`.
  - Tests: `crates/pcloud-ipc/tests/peer_and_protocol.rs::decode_request_rejects_version_mismatch`,
    `crates/pcloud-ipc/tests/security_invariants.rs::sec_13_decode_rejects_mismatched_version`.

### Auth vault

- **SEC-20** — The auth vault file is `0600` on a `0700` parent.
  - Enforcement: `crates/pcloud-daemon/src/vault/file.rs::store_token`
    (`set_permissions` on parent and file; `O_CREAT|O_EXCL|mode(0o600)`
    on the tmp file).
  - Tests: `crates/pcloud-daemon/src/vault/file.rs::tests::store_token_writes_secure_file_and_loads_it`
    (inside the crate, guards the mode round-trip).
- **SEC-21** — A vault file whose metadata fails the security check
  (wrong owner, world/group-accessible mode, non-regular file, non-UTF-8
  contents) is treated as absent/rejected, never repaired.
  - Enforcement: `crates/pcloud-daemon/src/vault/file.rs::validate_vault_file`
    and the UTF-8 check in `load_token`.
  - Tests: `crates/pcloud-daemon/src/vault/file.rs::tests::{load_token_rejects_group_readable_file, load_token_rejects_non_utf8_contents, store_token_refuses_to_follow_symlink_at_tmp_path}`.
  - Note: this fork's vault persists a trimmed UTF-8 bearer token, not a
    BLAKE3-enveloped ciphertext. The envelope-based SEC-21 wording
    describes the target shape tracked by ADR-0007; the current rejection
    behaviour is equivalent for the "treat as absent, do not repair"
    invariant and is the shipped code path today.
- **SEC-22** `[review-only]` — Durable auth persistence is opt-in; default
  behaviour never writes a bearer token to disk.
  - Enforcement: `pcloud_config::FeatureFlags::durable_auth_tokens_enabled`
    (default `false`); `bootstrap.rs` never calls `store_token` unless
    the flag is explicitly opted into.
  - Status: end-to-end bootstrap test for the opt-in gate is tracked
    under `bd-1du.10`; the flag's default is code-visible.
- **SEC-23** `[review-only]` — The vault never stores a cleartext password
  (deliberate divergence from the legacy C client; see
  [ADR-0007](../adr/0007.md)).
  - Enforcement: `crates/pcloud-daemon/src/vault/file.rs::store_token`
    accepts only a `SecretString` passed through the bearer-token code
    path. There is no `store_password` surface; the authenticator
    exchanges the password for a token before any vault write.
  - Status: absence of a function is not directly testable. The
    invariant is a structural guarantee verified by code review — no
    call site writes the plaintext password to disk.

### Audit hash-chain

- **SEC-30** — Privileged operations are recorded with a BLAKE3-linked
  hash-chain; each record chains into the previous record's digest.
  - Enforcement: `crates/pcloud-store/src/repositories/audit.rs::{AuditRepository::append_event, AuditRepository::verify_chain, rebuild_hash_chain}`.
  - Tests: inline unit tests in the same module
    (`tampered_entry_hash_is_detected`, `tampered_prev_hash_is_detected`,
    `hmac_mismatch_is_detected`) and the public
    `pcloud_store::verify_audit_chain` entry point.
- **SEC-31** `[review-only]` — Audit persistence failure surfaces as an
  error on the control path; it is never silently swallowed.
  - Enforcement: `AuditRepository::append_event` returns `Result`; the
    dispatch arm at `crates/pcloud-daemon/src/runtime.rs` propagates the
    error and turns it into `ResponseStatus::InternalError`.
  - Status: the dedicated failure-surface integration test is tracked
    under `bd-1du.10`.
- **SEC-32** `[review-only]` — The audit chain is updated atomically via
  SQLite WAL (BEGIN / INSERT / COMMIT); a crash mid-write leaves a
  consistent chain by virtue of the WAL rollback guarantee, and
  `verify_chain` / `rebuild_hash_chain` detect any break.
  - Enforcement: `pcloud-store` opens SQLite in WAL mode (see
    `pcloud_store::schema::apply_schema_v8`); `verify_audit_chain` is
    called on startup to catch any residual breakage.

### Transport

- **SEC-40** — Production configs reject plaintext downgrade from TLS.
  - Enforcement: `crates/pcloud-config/src/api.rs::ApiEndpoint::validate`
    (`Environment::Production` + `ApiMode::Plaintext` returns
    `InvalidApiEndpoint`).
  - Tests: `crates/pcloud-config/src/api.rs::tests::{production_plaintext_is_rejected, production_tls_is_allowed, development_plaintext_is_allowed}`.
- **SEC-41** `[review-only]` — TLS roots are pinned to the Mozilla root
  set shipped with `webpki-roots`; no system-trust override in
  production.
  - Enforcement: `crates/pcloud-proto/src/transport.rs` (the rustls
    client config is built from `webpki-roots` only; there is no
    `with_native_roots` call on any production path).
- **SEC-42** `[review-only]` — Endpoint override requires an explicit
  opt-in and is refused in production builds.
  - Enforcement: `crates/pcloud-config/src/api.rs` — the
    `apply_api_server_hint` helper only updates host/SNI/port inside an
    already-validated `ApiEndpoint`; production rejects `Plaintext`, so
    a plaintext override cannot take effect.

### Panic guard

- **SEC-50** — Every dispatch arm runs inside `catch_unwind`; a panic
  converts into `Response::InternalError` and (when the `metrics`
  feature is on) increments the panic counter.
  - Enforcement: `crates/pcloud-daemon/src/runtime.rs::handle_request`
    (the `catch_unwind(AssertUnwindSafe(...))` block around
    `handle_request_dispatch`).
  - Tests: `crates/pcloud-ipc/tests/security_invariants.rs::sec_50_catch_unwind_pattern_yields_internal_error`
    (pattern-level regression guard; the production wrapper site is
    not exercisable from outside the daemon crate without creating a
    dev-dep cycle).
- **SEC-51** `[review-only]` — Background-thread panics flow into a
  Prometheus gauge via the global panic hook.
  - Enforcement: `crates/pcloud-daemon/src/runtime.rs::install_panic_metrics_hook`
    plus `RuntimeShell::refresh_panic_metric`.
  - Status: the process-wide panic hook is only installed when the
    `metrics` feature is on, and observing the gauge requires the
    `/metrics` HTTP endpoint. An end-to-end test is tracked under
    `bd-1du.10`.

## State machine: auth-vault lifecycle

The vault has three states. The transitions are what the test matrix
guards.

| From       | Event                       | To          | Enforcement                        |
|------------|-----------------------------|-------------|------------------------------------|
| Absent     | login ok, `persist=true`    | Present     | `auth_vault.rs::put`               |
| Absent     | login ok, `persist=false`   | Absent      | no disk write                       |
| Present    | startup, integrity ok        | Present     | `auth_vault.rs::open`              |
| Present    | startup, integrity fail      | Absent      | treat as absent; require re-login  |
| Present    | logout                       | Absent      | `auth_vault.rs::erase`             |
| Present    | rotate                       | Present'    | atomic replace                      |

## Tradeoffs and design decisions

- **No raw-password persistence** ([ADR-0007](../adr/0007.md)). The legacy
  C client stored a password so it could re-derive the crypto key on
  startup. We explicitly refuse: we store only a bearer token when the
  user opts in, and we require re-unlock for crypto folders.
- **BLAKE3 over SHA-256 for audit chain** because BLAKE3 is faster, we
  have no FIPS constraint, and the security-margin difference is
  irrelevant at our use case.
- **File-based vault fallback even on Linux desktops** because
  Secret Service is a D-Bus dependency and we refuse to require a
  desktop to run the daemon.
- **`catch_unwind` is default-on** ([ADR-0004](../adr/0004.md)). A buggy
  dispatch arm cannot take the daemon down and cannot hide — it emits a
  typed error and increments a counter.

## Concurrency model (security-relevant)

- The audit writer runs on a dedicated background thread; the dispatch
  arm enqueues synchronously and waits for ack, so a failed audit write
  is visible on the control path.
- The vault is guarded by a `parking_lot::Mutex`; reads are rare (only at
  startup and at token-refresh time), so no RwLock.
- The secret container types are `Send` but not `Sync`; a caller that
  needs cross-thread sharing must wrap in `Arc<Mutex<Secret…>>`, which is
  rare and always reviewed.

## Performance notes (security-adjacent)

- `SecretString`/`SecretBytes` zeroize on drop is a per-byte write; the
  cost is negligible except for multi-MiB keys (which we do not store).
- BLAKE3 audit-chain append is measured at ~2 µs per record on
  Linux x86_64; audit write does not dominate any control-path budget.
- TLS handshakes are cached across calls (rustls client config is
  `Arc`-shared); the amortised handshake cost is zero after the first
  call.

## Extension points

- **Custom KMS** — implement `pcloud-kms::KmsProvider` to replace the
  local secret-material store with an enterprise KMS. Pkcs11 stub is in
  tree; no live HSM interop yet.
- **Custom vault** — implement `PlatformVault` for a non-default backing
  (e.g., Hashicorp Vault via a sidecar). The envelope and integrity rules
  remain unchanged.
- **Custom audit sink** — the audit writer can be configured to
  additionally emit to an external SIEM; the hash-chain invariants are
  preserved on the local file regardless.

## Open `bd` trackers

- **`bd-1du`** — parity epic (security posture cited in closure
  criteria).
- **`bd-1du.4`** — mount-layer security (FUSE ACLs, WinFSP DACL).
- **`bd-1du.4.6.1`** — enterprise readiness (KMS/IDP/policy surfaces).
- **`bd-1du.10`** — parity proof gating "no misleading release claims"
  language.

## Cross-references

- [Overview](./overview.md) — the five platform abstractions and the
  concurrency domains.
- [Crate Map](./crate-map.md) — `pcloud-secret`, `pcloud-auth`,
  `pcloud-observability` ownership.
- [Request Lifecycle](./request-lifecycle.md) — where each invariant
  sits in the request pipeline.
- [Platform Support](./platform-support.md) — per-platform peer-check
  and vault backing.
- [Security Model (full)](../security/model.md) — the
  broader threat-model page.
- [Threat Model](../security/threat-model.md).
- [External Audit Dossier](../security/audit-dossier.md).
- [ADR-0004](../adr/0004.md), [ADR-0005](../adr/0005.md),
  [ADR-0007](../adr/0007.md).
