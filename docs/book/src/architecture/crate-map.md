# Crate Map

The workspace is intentionally split into many small crates. The split exists
for three reasons: compile-time isolation (a change in `pcloud-proto` does
not force the CLI to rebuild), trust-boundary clarity (the CLI crate cannot
accidentally reach into the daemon's vault), and testability (each crate has
its own unit tests, and integration crates wire them together). This page
walks every crate in functional groups, with one paragraph per crate. Paths
are relative to `crates/`.

## Client surfaces

**`pcloud-cli`** — the `pcloudc` binary. It owns argument parsing, the
`Command` enum, global flags, and the IPC client that talks to the daemon.
It holds no authoritative state: every query and every mutation is a
serialized request. It formats responses for humans and maps them to exit
codes. Its tests are snapshot-style: given a parsed command, produce the
expected `Request`. It never links the daemon, the store, or the proto
client.

**`pcloud-web`** — an Axum-based HTTP facade. It exposes a narrow JSON and
HTML surface for operators and reuses the same IPC client as the CLI. The
web crate has no remote credentials of its own; it is a browser-facing
skin over the daemon. TLS termination, if needed, is expected to come from
a reverse proxy — the crate itself listens on loopback.

## IPC and daemon

**`pcloud-ipc`** — the wire protocol between clients and the daemon. It
defines the `Request` and `Response` enums, the framing (8-byte
little-endian length prefix plus JSON body), the bounded-size guard, and
the thin `IpcClient` helper. Both `pcloud-cli` and `pcloud-web` depend on
it; the daemon depends on it for decoding. It is a pure library with no
side effects.

**`pcloud-daemon`** — the Unix daemon crate. It contains `bootstrap.rs`
(process setup, directory creation, vault unlock, store open), `runtime.rs`
(the dispatch engine, request handler, and panic-safe wrappers), the
per-subsystem backends (`auth_backend`, `sync_backend`, `transfer_backend`,
`public_link_backend`, `shares_backend`, `backup_backend`, `account_backend`),
and the platform-specific Unix IPC acceptor. It is the trust-boundary crate:
every authoritative decision is made inside it.

**`pcloud-daemon-win`** — the Windows daemon crate. It mirrors
`pcloud-daemon`'s dispatch surface but replaces the Unix-socket acceptor
with a named-pipe acceptor, SID-DACL construction, and `GetNamedPipeClientProcessId`-based
peer verification. It reuses the platform-neutral backends and runtime
directly; only the IPC, service-control, and vault glue differ.

**`pcloud-backends`** — a shared library crate holding backend implementations
that both `pcloud-daemon` and `pcloud-daemon-win` consume. This is where the
per-subsystem business logic lives in its platform-neutral form, so the two
daemon binaries stay thin.

## Protocol

**`pcloud-proto`** — the typed HTTPS client for `binapi.pcloud.com`. It is
organized per-API-family: `auth_api.rs`, `transfer_api.rs`,
`public_links_api.rs`, `shares_api.rs`, `backup_api.rs`, `account_api.rs`,
and so on. Each module owns request serialization, response parsing, and
error classification for its family. The crate uses blocking `rustls` +
`ureq`, and it is the only crate that holds a `reqwest`/`ureq`-style client.
It rejects plaintext downgrades in production builds.

## Domain model

**`pcloud-model`** — the shared data types that cross the IPC boundary and
the persistence boundary. `SyncRoot`, `BackupDevice`, `PublicLink`,
`ShareRecord`, `CryptoFolder`, `TransferHandle`, `AccountProfile`, and their
associated enums. It is a pure types crate: no I/O, no network, no
secret-bearing storage. Both `pcloud-proto` and `pcloud-store` map into and
out of these types.

## Data layer

**`pcloud-store`** — the SQLite-backed relational store. It wraps `rusqlite`
in a `Storage` type that exposes typed repositories (sync roots, public
links, shares, backup devices, etc.), an explicit migration runner, and
transactional helpers. Everything is WAL-mode, schema changes are additive,
and the crate refuses to open a store whose schema is newer than it
understands.

**`pcloud-cache`** — an in-memory cache layered in front of `pcloud-store`
for read-heavy surfaces (folder listings, account profile, link metadata).
It uses bounded LRU maps with explicit TTLs and invalidation hooks wired to
the backends, so a mutation in the daemon invalidates the cache entry it
touches.

## Filesystem

**`pcloud-fs`** — the virtual-drive crate. It contains mount scaffolding, a
`PlatformMount` trait, policy validation (where can we mount?), RAII mount
handles, signal-aware unmount cleanup, an in-memory read path, a staging
area for writes, the writeback journal, and platform-specific FUSE
(Linux), fuse-t (macOS), and WinFSP (Windows) backends. It is the crate
that `bd-1du.4` is actively hardening into full mounted-drive parity. The
page cache in this crate is the O(1) LRU + `Arc<Vec<u8>>` hot-path
documented in [Performance](./performance.md).

**`pcloud-bench`** — Criterion micro-benchmarks for the performance-critical
paths (`chunked_flush`, `upload_session`, `page_cache_evict`). Landed under
plan item `C4`; results are regression-gated by the release checklist.

## Crypto and security

**`pcloud-crypto`** — the Crypto Folder implementation. It owns AES-256-GCM
sector encryption, deterministic filename encoding for metadata entries,
key derivation from the user's passphrase, the crypto-aware share
temppass flow, and password-rotation helpers. Keys live inside
`SecretBytes`, are zeroized on drop, and are never logged.

**`pcloud-secret`** — the low-level secret wrappers. It defines
`SecretString` and `SecretBytes`: owned buffers that zeroize on `Drop`,
refuse to `Debug` print their contents, and provide explicit `expose`
helpers so the leak site is always searchable in a grep.

**`pcloud-auth`** — auth-state management sitting between `pcloud-proto`'s
auth_api and `pcloud-daemon`'s auth_backend. It owns the TFA flow state
machine, the token envelope, and the vault-read/vault-write choreography,
so the daemon backend stays a thin orchestration layer.

## Observability

**`pcloud-observability`** — structured logging, metrics, the audit
hash-chain writer, and redaction rules. The crate enforces redaction at
the `tracing` field layer, so any accidental secret field is filtered
before it reaches a subscriber. The audit writer is here because audit is
an observability property, not a store property.

## Resilience

**`pcloud-resilience`** — retry policies, circuit breakers, backoff
schedules, and bounded-concurrency helpers used by both the proto client
and the backends. Centralizing them here means a network hiccup is
handled consistently whether it happened during an auth call or a
folder-list call.

## Engine

**`pcloud-engine`** — the sync engine. It owns folder walking, diffing,
scheduling uploads and downloads, resolving conflicts, and driving the
transfer queue. It is called from the daemon's `sync_backend` and
interacts with `pcloud-store` (for sync-root records and pending work),
`pcloud-proto` (for folder and file metadata), and `pcloud-cache` (for
read amplification).

## SDK

**`pcloud-sdk`** — the embeddable Rust SDK. Third parties who want to
drive pCloud without running the daemon can link this crate directly. It
re-exports the safe subset of `pcloud-proto`, adds high-level helpers
(`upload_data`, `upload_file`, `upload_file_as`, etc.), and exposes
crypto, public-link, share, and account operations with the same
`SecretString` discipline the daemon uses internally.

## Config

**`pcloud-config`** — configuration loading, validation, and layering. It
merges built-in defaults, system config (`/etc/pcloud/…` or the Windows
equivalent), user config (under `PcloudDirs`), and environment variables,
and produces a frozen `AppConfig` at daemon startup. It is the only crate
that touches config files.

## Miscellaneous and test support

**`pcloud-p2p`** — peer-to-peer LAN transfer scaffolding (opt-in, disabled
by default). Retained as an isolated crate so it can be audited and
feature-gated independently.

**`pcloud-plugin-api`** — the plugin registry surface. It defines the
versioned trait that dynamic plugins must implement, the discovery path,
and the sandboxing contract. Used sparingly today; reserved for extension
points we do not want to bake into the core.

**`pcloud-compat`** — compatibility shims for the legacy C client's
on-disk artifacts. It can read an old `data.db`, extract sync-root and
auth records, and migrate them into the Rust `store.sqlite3` and vault.
Read-only for anything that would be destructive.

**`pcloud-chaos`** — fault-injection harness. It wraps `pcloud-proto`
calls and `pcloud-store` transactions with deterministic failure modes
(timeouts, truncated reads, partial writes, random I/O errors) used by
the test suite to prove resilience.

**`pcloud-mockserver`** — an in-process mock of `binapi.pcloud.com` for
protocol-level tests. It speaks real TLS with a test root, replays canned
responses, and supports scenario scripting. Used by `pcloud-proto` and
backend tests.

**`pcloud-live-e2e`** — the live end-to-end test crate. It runs against a
real pCloud account (credentials supplied via env), exercises the full
auth + sync + transfer + public-link + share + crypto surface, and is
gated off by default so CI can opt in deliberately.

**`pcloud-error`** — the shared `Error`/`Result` types. Typed errors with
explicit error kinds, no stringly-typed anything, and `Display`
implementations that never include secret material. Every other crate in
the workspace depends on it, so a single error taxonomy spans the
codebase.

## If you're new to this codebase

There are 35+ crates. That is a lot. The **thing to know** is that the
workspace is split along three axes simultaneously, and every crate lives at
the intersection of those three:

1. **Trust axis** — is this crate allowed to hold a secret, open the store,
   or hit the network?
2. **Platform axis** — is this crate platform-neutral, platform-specific, or
   a platform-abstraction hub?
3. **Process axis** — does this crate run in the client process, the daemon
   process, both, or neither (library-only)?

If you are looking for "where does X live?" and the answer is not obvious,
triangulate on those axes. For example: peer authentication is secret-adjacent
(trust), platform-specific (platform), and runs inside the daemon (process) —
so it lives in `pcloud-ipc` with per-platform `cfg` modules, not in
`pcloud-cli`.

## Stability tiers

Each crate is assigned a stability tier. The tier is a promise about how
carefully we treat API changes, not a quality claim — pre-alpha crates can
still be well-tested.

- **Tier S (stable core)** — `pcloud-secret`, `pcloud-error`, `pcloud-model`,
  `pcloud-ipc`. API changes require an ADR. These crates are what plugins
  and SDK consumers see.
- **Tier I (internal stable)** — `pcloud-proto`, `pcloud-store`,
  `pcloud-crypto`, `pcloud-auth`, `pcloud-backends`, `pcloud-observability`,
  `pcloud-resilience`, `pcloud-engine`. API is stable inside the workspace;
  external consumers should use `pcloud-sdk` instead.
- **Tier E (evolving)** — `pcloud-daemon`, `pcloud-daemon-win`,
  `pcloud-cli`, `pcloud-web`, `pcloud-sdk`, `pcloud-config`, `pcloud-cache`,
  `pcloud-fs`, `pcloud-session`. Changes are scrutinised but do not
  block on an ADR.
- **Tier X (experimental / bounded)** — `pcloud-p2p`, `pcloud-kms`,
  `pcloud-idp`, `pcloud-plugin-*`, `pcloud-fleet`, `pcloud-chaos`,
  `pcloud-mockserver`, `pcloud-compat`, `pcloud-bench`, `pcloud-live-e2e`,
  `pcloud-policy`. Feature-gated or test-harness crates. May change shape
  without notice.

## Dependency arrows (high-level)

Text-only diagram; read edges as "depends on".

```
pcloud-cli ------> pcloud-ipc ------> pcloud-model
    |                                    ^
    v                                    |
pcloud-sdk --+-> pcloud-proto -----------+
             +-> pcloud-backends --+
                                   |
pcloud-daemon / pcloud-daemon-win -+
    |                              |
    |                              +-> pcloud-store ---> pcloud-model
    |                              +-> pcloud-cache ---> pcloud-store
    |                              +-> pcloud-engine --> pcloud-proto, pcloud-store
    |                              +-> pcloud-fs -----> pcloud-plugin-api
    |                              +-> pcloud-crypto --> pcloud-secret
    |                              +-> pcloud-auth ---> pcloud-secret, pcloud-proto
    |                              +-> pcloud-observability
    |                              +-> pcloud-resilience
    |                              +-> pcloud-session
    |                              +-> pcloud-policy, pcloud-kms, pcloud-idp
    |
    +-> pcloud-config ---> pcloud-model
    +-> pcloud-plugin-api (loads: pcloud-plugin-autoheal, -backup-schedule,
                                  -publink-expiry, -dlp)

pcloud-web --> pcloud-ipc (same client path as pcloud-cli)

test-only:
  pcloud-mockserver --> pcloud-proto
  pcloud-chaos ------> pcloud-proto, pcloud-store
  pcloud-live-e2e ---> pcloud-sdk
  pcloud-bench ------> pcloud-fs, pcloud-daemon
  pcloud-compat -----> pcloud-store
  pcloud-fleet ------> pcloud-sdk (fleet orchestration harness)
```

Rules:

- Nothing depends on `pcloud-cli` except the binary itself and its tests.
- `pcloud-sdk` is a *peer* of `pcloud-daemon`, not upstream or downstream of
  it. Both consume `pcloud-backends`.
- `pcloud-observability` is allowed to be depended on from anywhere except
  `pcloud-secret` and `pcloud-error` (they would create a cycle through
  `tracing`).

## Public surface per crate (abbreviated)

Only crates whose public surface is not self-evident from the earlier
descriptions are listed here; for the rest, the per-crate paragraph above is
the canonical reference.

- `pcloud-secret::{SecretString, SecretBytes}` — owned, zeroizing, redacted
  `Debug`. Expose via `as_bytes()`/`expose_secret_str()` at the leaf call
  site only.
- `pcloud-error::{Error, ErrorKind, Result}` — typed errors. No `anyhow`,
  no `eyre`. `Error::kind()` is exhaustive on a closed `ErrorKind` enum.
- `pcloud-ipc::{Request, Response, IpcClient, protocol}` — frames, bounded
  body, peer-cred helpers.
- `pcloud-model::*` — pure types, no behaviour.
- `pcloud-plugin-api::{Plugin, PluginContext, PluginResult}` — versioned
  plugin trait, discovery path (`~/.config/pcloud/plugins.d` on Linux).
- `pcloud-kms::{KmsProvider, LocalKms, Pkcs11KmsStub}` — KMS abstraction;
  `Pkcs11KmsStub` is a stub (no live HSM interop yet; see
  [`bd-1du.4.6.1`](../adr/index.md) for the broader enterprise readiness
  epic).
- `pcloud-policy::{PolicyProvider, MountPolicy, SharePolicy, DlpPolicy}` —
  declarative policy traits.
- `pcloud-idp::{IdpProvider, OidcBroker}` — identity-provider integration
  for enterprise SSO; no live OTLP interop yet, only schema scaffolding.

## Tradeoffs and design decisions

- **Why not a single monolithic crate?** It would compile faster on a
  single change to one module but slower on a clean build; more importantly,
  trust boundaries would be policed by review alone, not by `Cargo.toml`.
- **Why separate `pcloud-daemon-win` instead of `#[cfg(windows)]` in
  `pcloud-daemon`?** Because the Windows path has additional dependencies
  (`windows-sys`, SCM glue) that we do not want to pull into the Unix
  build's dep graph. A separate crate keeps both binaries lean.
- **Why keep `pcloud-mockserver` in-tree?** Because live e2e runs depend on
  shaped server responses; shipping our own TLS-speaking mock gives us
  deterministic protocol tests without pinning against a real staging env.
- **Why a dedicated `pcloud-chaos`?** Deterministic fault injection needs
  to be composed with other test harnesses, and a library crate gives us a
  clean integration surface.

## Concurrency ownership

Each crate owns its own concurrency primitives; none leak across crate
boundaries except through explicit handles.

- `pcloud-daemon` / `pcloud-daemon-win`: IPC acceptor threads, dispatch.
- `pcloud-engine`: fixed-size worker pool + cancellation token.
- `pcloud-fs`: signal-aware mount helper + per-inode page-cache LRU.
- `pcloud-web`: tokio runtime (isolated to this crate).
- `pcloud-observability`: a background writer task for the audit
  hash-chain.

Everything else is sync-only.

## Security invariants

- `pcloud-secret` is the only crate allowed to own `Drop`-zeroizing raw
  buffers. A grep for `impl Drop` on string/byte types in any other crate
  is a review block.
- `pcloud-cli` must not link `rusqlite`, `rustls`, or any secret vault
  crate. Enforced in `Cargo.toml` and double-checked in CI.
- `pcloud-observability` redacts tracing fields at the subscriber layer;
  crates that log must go through this crate, not directly to `tracing`.

## Performance notes

- `pcloud-fs` is the hot crate: page cache, chunked flush,
  `flush_latency_seconds` histogram. See
  [Performance](./performance.md).
- `pcloud-cache` fronts `pcloud-store` for folder listings and link
  metadata; TTLs are tuned per surface.
- `pcloud-proto` uses blocking `rustls` + `ureq`; no per-request
  allocation of TLS configs (they are `Arc`-reused).

## Extension points

- `pcloud-plugin-api` — versioned plugin trait. Four in-tree plugins live
  under `pcloud-plugin-*` and serve as reference implementations.
- `pcloud-backends::Backend` — new daemon-mediated features implement this
  trait and wire through `pcloud-daemon::runtime::dispatch`.
- `pcloud-kms::KmsProvider` — alternative secret-material providers.
- `pcloud-policy::PolicyProvider` — deployment-specific policy bindings.
- `pcloud-idp::IdpProvider` — SSO/OIDC adapters (scaffolded).

## Open `bd` trackers

- **`bd-1du`** — parity epic (umbrella for crate-surface completeness).
- **`bd-1du.4`** — `pcloud-fs` parity work (FUSE/WinFSP/fuse-t).
- **`bd-1du.4.6.1`** — enterprise readiness (KMS, IDP, policy).
- **`bd-1du.10`** — parity proof; docs/release gating.

## Cross-references

- [Overview](./overview.md) for the process and data-flow picture.
- [Request Lifecycle](./request-lifecycle.md) for the code path through
  these crates.
- [Performance](./performance.md) for the `pcloud-fs` hot paths.
- [Platform Support](./platform-support.md) for which crates are
  platform-conditional.
- [Security Model](./security-model.md) for the trust-boundary enforcement.
