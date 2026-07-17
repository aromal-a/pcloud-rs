# Architecture Atlas Feature-Coverage Audit

Date: 2026-07-17  
Scope: current working tree at `/home/ezechiel203/Projects/FORKS/pcloud-rs`  
Deliverable constraint: audit only; no generated pages, book chapters, or
navigation were edited.

## Executive result

The atlas is already strong as a **source index** and as an explanation of the
main daemon/IPC/RemoteFs path. It is not yet a complete **feature reference**.
Its hand-authored chapters explain the process model, canonical RemoteFs,
selected request paths, durability, broad security boundaries, SDK choice,
and the high-level platform matrix. Generated pages enumerate all current
Cargo packages, files, features, targets, and Rust declarations.

Those generated tables do not answer the new requirement by themselves. For
most features the site still lacks one or more of:

- why the feature exists and what problem it solves;
- strengths and deliberate design trade-offs;
- user/operator/developer use cases;
- public, CLI, IPC, runtime, protocol, and configuration entrypoints;
- end-to-end data/control flow and ownership;
- single-user versus multi-account/enterprise applicability;
- security and crypto implications;
- platform availability and package/service integration;
- implemented/scaffolded/experimental/unshipped/release-qualified status;
- limitations, non-goals, and test/qualification evidence.

Current repository facts that the completed site must cover:

- **42 Cargo packages**, including the repository-owned `xtask` pipeline;
- **6 binary targets**;
- **37 Cargo feature flags** across 11 packages, of which 26 are non-default;
- user surfaces spanning CLI, stable SDK, embedded SDK, web UI, mount, and an
  experimental WebDAV subset;
- single-user, multi-account supervisor, business/team sharing, policy, IdP,
  fleet, KMS/HSM, HA, residency, audit, tracing, DLP, and plugin surfaces;
- two crypto backends/profiles plus raw/KMS DEK modes and selectable
  providers;
- verification-only crates and harnesses whose behavior must not be presented
  as shipped product functionality.

Overall assessment: **good architecture skeleton; incomplete feature atlas**.
The highest-risk documentation omissions are crypto, complete CLI/API
capability coverage, enterprise/multi-account architecture, feature flags,
plugin execution/trust boundaries, protocol-family ownership, and the now
authoritative local `xtask` CI/CD model.

## Audit method and evidence rule

The audit compared:

1. `cargo metadata --format-version 1 --no-deps` for packages, targets, and
   feature flags;
2. every `crates/*/src` module and crate-level documentation;
3. CLI `Command`, IPC `Method`/`Request`, daemon/backends, protocol families,
   config modules, store repositories, platform modules, examples, benches,
   fuzz targets, live tests, chaos tests, DR drills, packaging, and `xtask`;
4. all hand-authored atlas chapters under `docs/architecture-atlas/src`
   (generated catalogs were treated as indexes, not explanatory coverage).

Maturity statements must follow current executable source and tests. Existing
README/module prose is useful evidence but cannot be copied blindly: this
audit found current source-documentation contradictions (listed below).

## What the atlas already covers well

| Area | Existing strength |
|---|---|
| Process model | Clear CLI/SDK/web to native IPC to daemon to pCloud flow |
| Canonical namespace | RemoteFs rationale, ID-first resolution, operations, durability, and consumers |
| Core entrypoint choice | Public SDK versus embedded SDK versus CLI/protocol guidance |
| Durability | Upload/download state machines, state ownership, drain/recovery concepts |
| Trust boundaries | IPC peer identity, vault/secrets, TLS, mount, and plugin boundary overview |
| Platforms | Tier intent, mount versus portable API distinction, native qualification caveat |
| Navigation | Generated per-crate file/declaration inventory and exhaustive Git-visible file inventory |
| Development skeleton | Vertical-slice extension guidance and broad verification taxonomy |

The problem is breadth and depth beyond these paths, not absence of a useful
foundation.

## Top omissions, ordered

### P0 — Crypto is not documented as a complete product architecture

The current crypto discussion is too small for the implementation. A complete
crypto section must distinguish and connect:

- `CryptoBackend::PclsyncCompat` versus `CryptoBackend::Enhanced`;
- official-client interoperability versus the stronger but incompatible
  enhanced format;
- `CryptoMode::Raw` versus `CryptoMode::Kms` DEK handling;
- `crypto-provider-rustcrypto` versus `crypto-provider-aws-lc-fips`;
- `pclsync-v2`, `legacy-c-compat`, and `test-helpers` feature semantics;
- setup/start/stop/reset/status/hint, password scoring and password rotation;
- Argon2/enhanced key derivation, pCloud-compatible PBKDF2 profile, RSA-4096
  key envelopes, authenticated tree, filename encoding, sector formats,
  CBC-CTS/CTR primitives, folder/file-key caches, metadata visibility, and
  lockout/state transitions;
- RSA and temporary-password crypto sharing;
- mount/read/write integration and context requirements;
- KMS provider injection, AWS KMS, Vault Transit, the explicitly
  unimplemented PKCS#11 provider, cache TTL/eviction, rewrap, failure mode,
  and offline/read-only policy;
- live/offline KATs, round trips, fuzzers, and which evidence is still
  external.

Entry points span `pcloud-crypto`, `pcloud-kms`, `pcloud-config::crypto_kms`,
daemon crypto backend/runtime, IPC, CLI, embedded SDK, mount runtime, and
share backends. No single atlas chapter currently joins them.

### P0 — No complete capability catalog for CLI, IPC, SDK, and protocol

`entrypoints.md` lists the remote file commands but the CLI has well over one
hundred command variants covering status/health/SLO, auth/TFA, sync, excludes,
conflicts, shares/teams, public/upload links, bookmarks, notifications,
crypto, mount, stat/filesystem, reload/doctor/migration/verification,
snapshots, integrity, HA, audit verifier, upload sessions, account utilities,
downloads, and backup/device operations.

The site needs a generated or maintained cross-surface matrix:

`user intent -> CLI command -> IPC Request/Method -> daemon handler -> owning
backend/engine -> protocol call/store side effect -> SDK exposure -> platform
and maturity`.

Without it, an implementer can find declarations but cannot determine which
features are actually reachable, internal-only, stable-SDK-visible, or
backend-only.

### P0 — Enterprise and multi-user architecture is almost entirely absent

The site mentions enterprise crates but does not explain the combined model:

- multi-account `pcloud-supervisor`, account registry/routing, sub-daemon
  spawning, account-scoped runtime/store/vault/IPC paths, and current scaffold
  versus wired behavior;
- business teams, account-team share, multi-user A-to-B share tests, share
  permission mutation, encrypted sharing, contacts, and requests;
- `pcloud-fleet` identity, privacy-scrubbed heartbeats, signed commands, mTLS,
  and the missing production fleet server/transport boundaries;
- `pcloud-idp` OIDC Authorization Code + PKCE, JWKS/issuer/audience/nonce
  validation, trusted-issuer exchange, the default HTTP exchanger, explicitly
  unsafe plaintext test feature, and absent official pCloud exchange/SAML;
- `pcloud-policy` default-deny Rego evaluation, permission checks, reload,
  audit, and daemon dispatch integration;
- HA lease, rolling drain/handoff, audit verifier, integrity sweeper, data
  residency, bandwidth schedules, rate limits, metered networks, transport
  selection, trace propagation, health/SLOs, snapshots, DLP, KMS, and fleet;
- what applies to one user, one account, multiple accounts on one host,
  business teams, and centrally managed enterprise fleets.

### P0 — Cargo feature flags have no operator/developer reference

Generated crate pages list names and dependency expansions but do not state
why a feature exists, when to enable it, platform/provider constraints,
security changes, binary/dependency cost, incompatible combinations, or the
qualification gate. The complete flag inventory appears below.

### P1 — Plugin architecture and built-in plugins lack usable documentation

The atlas needs a coherent progression from signed manifest and capability
policy (`pcloud-plugin-api`), through host message bus and capability checks
(`pcloud-plugin-host`), to Wasmtime execution (`pcloud-plugin-wasmtime`) and
built-ins:

- auto-heal checksum response/quarantine/escalation;
- DLP audit-only versus strict scanning and privacy rules;
- public-link expiry state, notification window, and rate limit;
- backup schedule DSL and CLI helper;
- trust, audit, resource limits, absence of secret access, and what is or is
  not wired into daemon startup.

The current security chapter's short plugin paragraph is not enough.

### P1 — Core internal pipelines are described only at box-diagram level

Missing explanatory feature maps include auth/session refresh; sync event,
planner, conflict, recovery, selective sync and differential transfer;
cache layers and encrypted/sealed cache objects; store schema/repositories;
resilience primitives; audit/metrics/SLO/OTLP; configuration layering and
reload; snapshots/integrity; mount read/write/writeback/orphan cleanup; and
daemon bootstrap/background-service ordering.

### P1 — Protocol families and wire layers lack a complete ownership map

The protocol chapter must cover account, auth, backup, crypto, diff, folder,
notifications, public links, shares, sync, upload/download/transfer, binary
request/response framing, TLS validation, redaction, resilient transport,
async/chunked transfer, parallel download, signed download URLs, retry and
error classification. It must distinguish pCloud remote protocol from local
daemon IPC and from HTTP downloads.

### P1 — Platform/package architecture is broad but not operationally complete

The current target table needs drill-down pages for native IPC, vault, mount,
service lifecycle, filesystem discovery, packaging format, signing, upgrade,
uninstall, and release evidence on each target. Package/service assets are
indexed but not explained. NAS must have a per-vendor lifecycle and hardware
qualification page rather than one shared sentence.

### P1 — Verification helpers are not mapped to the features they prove

Test, mock, chaos, fuzz, live, benchmark, mutation, coverage, DR, memory, and
reproducibility helpers are cataloged as files, but readers cannot answer
which feature/invariant each proves, which require credentials/kernel/native
hardware, or which do not constitute release evidence.

### P1 — Local CI/CD truth has changed

GitHub Actions YAML is intentionally archived under
`.github/workflows-disabled/`; `.github/workflows/README.md` points to
`cargo xtask ci`. `xtask` is therefore the repository's authoritative local
CI/CD orchestrator, not merely an “experimental / bounded” crate. Its
preflight, compatibility, host, coverage, Docker, Windows SSH, packaging,
CI, and release stages need an operator chapter. The atlas verification page
currently presents generic commands and does not explain this authority or
the partial-run environment flags.

## Per-package coverage inventory (all 42 packages)

“Index only” means the generated page lists files/features/declarations but
the authored atlas does not supply the required rationale/use-case/flow/status
explanation.

| Package | Functional scope requiring coverage | Current authored coverage | Required correction/addition |
|---|---|---|---|
| `pcloud-model` | IDs and auth, conflict, crypto, health, public-link, share, sync, transfer domain types | Index only / generic layer mention | Domain vocabulary, serialization boundaries, ID invariants, consumers |
| `pcloud-error` | Stable categories/codes, retryability, safe display, cross-layer translation | Brief error-boundary paragraph | Full taxonomy, mapping rules, user/CLI/SDK behavior, stability contract |
| `pcloud-config` | Profiles, env layering, schema/migrations, paths, auth/API, mount, HA, KMS, residency, integrity, extensions, bandwidth, rate/resilience, tracing, transport, upgrade | Generic ownership mention | Configuration reference architecture and each module's use/status/platform |
| `pcloud-kms` | KMS provider trait, plaintext/wrapped DEKs, TTL cache, AWS, Vault, PKCS#11 stub, serde | One security-reference link | Provider decision table, IAM/auth, failure/rewrap/cache flows, feature gates |
| `pcloud-secret` | `SecretString`/`SecretBytes`, expose sites, constant-time compare, zeroize, no serialization | Good high-level security mention | API use patterns, limitations, tests/bench, when redaction is insufficient |
| `pcloud-observability` | logging, audit chain, health, metrics/exporter, SLO, OTLP tracing, poisoned-lock handling | Partial metrics/audit mentions | Complete signals model, cardinality/redaction, feature flags, endpoints and ops |
| `pcloud-proto` | All remote API families plus binary/TLS/HTTP download/parallel/resilient transfer | Request-path examples only | Full remote protocol stack and family-by-family capability matrix |
| `pcloud-resilience` | clock, circuit breaker, global retry budget, metered network, pacing, rate limit, retry, timeout, resilient transport | Name only | Rationale, composition order, retry/idempotency rules, optional Tokio/metrics |
| `pcloud-ipc` | schema, framing, path validation, redaction, client/server, peer auth on Linux/Unix/Solaris/Windows | Good high-level path/security | Full method/request evolution, concurrency, limits, compatibility, platform details |
| `pcloud-plugin-api` | signed manifests, capabilities, typed operations/responses, registry, audit | Short boundary mention | Complete trust/lifecycle/capability model and daemon wiring status |
| `pcloud-plugin-autoheal` | integrity-event observation, notification, quarantine, escalation/rate limits | Index only | User/operator use case, limits, host events, current wiring/maturity |
| `pcloud-plugin-dlp` | audit-only/strict scanning, rules, entropy/binary detection, privacy | Index only | DLP policy, false-positive/data handling, upload integration, current wiring |
| `pcloud-policy` | policy input/decision, null engine, real Regorus engine, default deny, secure file reload | Index only | Correct stale “stub” prose; document implemented engine and daemon enforcement |
| `pcloud-auth` | login/TFA/recovery/refresh/logout state machine and secret invariants | Auth path sketch | Full state transitions, orchestrator/manager/events, retry and persistence boundaries |
| `pcloud-store` | SQLite bootstrap/WAL/migrations/tx/retry/integrity and all repositories | State table and durability concepts | Schema/repository ownership, backup/recovery, cache distinction, account scoping |
| `pcloud-engine` | diff poller/events, local scan, planner, conflict/reconcile/recovery, scheduler, selective sync, power/stall, transfers | Broad sync path | Complete event/state machine, conflict matrix, rsync integration, recovery evidence |
| `pcloud-rsync` | block signatures, rolling hash, delta ops/server-copy differential sync | Index only | Algorithm, strengths/costs, entrypoints, engine integration and maturity |
| `pcloud-cache` | generic page/checksum caches, eviction, staging, sealed blobs/cipher | Cache-is-not-authority statement | Each cache type, bounds/invalidation, crypto posture, platform/storage effects |
| `pcloud-fs` | portable backend, inode/path, metadata/page cache, read path, staging/journals/writeback, FUSE/native adapters, orphan cleanup, integrity/SLO hooks | Good high-level mount path/platform matrix | Operation-by-operation lifecycle, native callback maps, cache/write semantics and evidence |
| `pcloud-crypto` | Complete compatibility/enhanced crypto, content/metadata/key/share/KMS/state policies | Severely under-covered | Dedicated multi-page crypto architecture; reconcile stale introductory claims |
| `pcloud-daemon` | bootstrap/composition, dispatch, vaults, account scope, sync/mount, HA, reload, rate/bandwidth/power/metered, health/metrics/audit/integrity, drain/signals | Good composition skeleton | Background-service catalog, startup/shutdown order, configuration and feature switches |
| `pcloud-backends` | account/auth/backup/crypto/folder/notifications/public links/shares/sync/transfer plus path resolution, residency, snapshots, upload state/journal/session, RemoteFs | RemoteFs excellent; rest mostly absent | Per-backend capability/side-effect/error matrix and cross-backend orchestration |
| `pcloud-p2p` | policy/config, mDNS discovery status, peer inventory, transfer scaffold | Index only | Resolve contradictory docs; explicitly separate discovery reality from absent transfer |
| `pcloud-session` | deterministic session supervisor and refresh tick extracted from daemon | Index only | Lifecycle ownership, timing/re-auth behavior, vault relationship, re-export compatibility |
| `pcloud-cli` | All command families, parsing/aliases, config, prompts, progress, JSON/i18n/field selectors, doctor/migrate/verify | Remote file examples only | Complete command taxonomy and cross-surface reachability matrix |
| `pcloud-embedded-sdk` | Broad in-process compatibility facade, auth/crypto/links/shares/account/transfer/raw dispatch/upload sessions | Correct stability distinction | Full use-case and lifecycle guide, examples, differences from stable SDK |
| `pcloud-sdk` | Stable blocking IPC `Client`/`RemoteDrive` and owned SemVer types | Good entrypoint overview | Complete method/error/threading/platform reference and registry-release status |
| `pcloud-live-e2e` | Credentialed account, auth/TFA, crypto, transfer, share/team, sync, mount, fleet, snapshot, integrity, Windows tests | Generic live-test mention | Environment/account topology/destructive-test matrix and claim boundaries |
| `pcloud-fleet` | Device identity, privacy-safe heartbeat/SLO, signed commands, mTLS and reference server; production server gap | Index only | Enterprise deployment/threat/status chapter; separate real mTLS code from missing server |
| `pcloud-compat` | Legacy RPC/folder list/SHM producer and opt-in peek binary | Index only | Migration/debug use cases, Linux/Unix constraints, `legacy-shm` risk/status |
| `pcloud-mockserver` | Deterministic protocol test server and scripted flows | Verification name only | Supported scenarios, trust limits, how backends/proto tests consume it |
| `pcloud-chaos` | Blackhole, slowloris, disk-full, SIGKILL-mid-flush, clock-jump fault tests | Verification name only | Fault model, expected invariants, how to run and interpret |
| `pcloud-web` | Operator HTTP UI, routes/templates, token/host/bind policy | Brief entrypoint warning | Route/auth/deployment/reverse-proxy/CSP/status and non-goals |
| `pcloud-daemon-win` | Experimental Windows SCM wrapper over ordinary daemon runtime | Correctly called experimental | Exact non-public use case, SID/session limitations, why public package uses per-user daemon |
| `pcloud-idp` | OIDC+PKCE broker/exchange/JWKS, default HTTP exchange, null broker, future flows | Index only | Full SSO flow, operator bridge requirement, validation/security and feature flags |
| `pcloud-plugin-host` | Capability message bus, backend trait, no-op backend | Index only | Relationship to plugin API/Wasmtime, host-call lifecycle, current integration status |
| `pcloud-plugin-wasmtime` | Wasmtime backend, resource/fuel/memory policy and host-call seam | Index only | Sandbox strengths/limits, MSRV/dependency cost, supported ABI and wiring status |
| `pcloud-plugin-publink-expiry` | Link expiry observation, state file, notifications/rate limits | Index only | Operator configuration, permissions/privacy, lifecycle and current wiring |
| `pcloud-plugin-backup-schedule` | Bounded schedule DSL, resume operations, clocks, CLI helper | Index only | DSL reference, scheduling semantics, missed runs/timezones, current wiring |
| `pcloud-supervisor` | Multi-account registry/routing and sub-daemon spawner/stop | Index only | Single vs multi-account topology, account-scoped paths, scaffold/wiring truth |
| `pcloud-webdav` | Bounded parser/listener/PROPFIND and RemoteFs IPC adapter | Correct experimental warning | Verb matrix, body/temp-file behavior, listener security, missing RFC features and bootstrap |
| `xtask` | Authoritative local CI/CD: preflight/compat/host/coverage/package/Docker/Windows/release | Generated page misclassifies as experimental; verification omits authority | Dedicated local-CI operator page and accurate tooling maturity |

## Cargo feature-flag inventory (all 37 flags)

The replacement site should have one feature-flags page generated from Cargo
metadata, with hand-authored consequences layered on top. Empty `default`
features are still listed because they define the package's base behavior.

| Package | Flag | Current effect | Documentation needed |
|---|---|---|---|
| `pcloud-config` | `default` | Empty base configuration build | State that KMS factories are absent |
|  | `kms-factory` | Adds `pcloud-kms`/secret provider factory | When daemon can materialize providers |
|  | `aws-kms` | `kms-factory` + AWS provider | AWS runtime/IAM/dependency/platform qualification |
|  | `vault-kms` | `kms-factory` + Vault provider | Endpoint/token/TLS/failure configuration |
|  | `pkcs11-kms` | `kms-factory` + PKCS#11 feature | Explicitly state provider remains `NotImplemented` |
| `pcloud-kms` | `default` | Provider trait/null provider only | Base/no-provider behavior |
|  | `serde` | Serialization for key identifiers/wrapped blobs | Persistence/wire use and secret exclusions |
|  | `aws` | AWS SDK + Tokio provider | Provider behavior and async bridge cost |
|  | `vault` | Blocking Vault Transit HTTP provider | Token/env/TLS and retry behavior |
|  | `pkcs11` | PKCS#11 dependencies | Stub status and hardware gate |
| `pcloud-observability` | `default` | Core logging/audit/health/metrics types | Which exporters are absent |
|  | `json-logs` | JSON log formatting | Schema/redaction/operator ingestion |
|  | `prometheus-exporter` | Prometheus scrape output/server support | Bind/security/cardinality |
|  | `tracing-otlp` | OpenTelemetry/OTLP tracing stack | Export config, traceparent, redaction, cost |
| `pcloud-resilience` | `default` | Enables `transport-metrics` | Default dependency/metrics consequence |
|  | `transport-metrics` | Observability-backed transport metrics | Metric set and overhead |
|  | `tokio-timeout` | Optional cancellation-safe Tokio timeout | Only for async consumers; runtime requirement |
| `pcloud-crypto` | `default` | `pclsync-v2` + RustCrypto provider | Default interoperability/provider truth |
|  | `pclsync-v2` | RSA/PBKDF2/AES/CBC/CTR compatibility implementation | Exact format/interoperability and test evidence |
|  | `legacy-c-compat` | Legacy compatibility marker/path | What changes, if anything, versus `pclsync-v2` |
|  | `crypto-provider-rustcrypto` | Pure-Rust provider selection | Supported algorithms/platforms |
|  | `crypto-provider-aws-lc-fips` | AWS-LC/FIPS provider selection | FIPS claim boundary and native qualification |
|  | `test-helpers` | Test-only helper APIs | Never enable as a product capability |
| `pcloud-daemon` | `default` | Daemon without optional exporters/tracing | Base services available |
|  | `metrics` | Prometheus exporter integration | Endpoint/lifecycle/ops |
|  | `json-logs` | JSON structured logs | Schema and deployment use |
|  | `tracing-otlp` | OTLP dispatch/backend spans | Collector setup and sensitive-field rules |
| `pcloud-embedded-sdk` | `default` | Empty marker | State there are no optional capabilities |
| `pcloud-live-e2e` | `default` | Live tests disabled | Safe ordinary workspace behavior |
|  | `live` | Enables credentialed live suite surface | Required env/accounts/destructive safeguards |
| `pcloud-fleet` | `default` | Core wire/identity/null-agent surface | No production fleet integration implication |
|  | `mtls` | mTLS fleet agent/reference path | Certificates, server gap, native/live evidence |
| `pcloud-compat` | `default` | Library without SysV SHM peek binary | Portable/base migration behavior |
|  | `legacy-shm` | Builds legacy SHM producer/peek binary | Platform, permissions, debug-only use |
| `pcloud-idp` | `default` | Enables `oidc-http-exchange` | Default network behavior must be explicit |
|  | `oidc-http-exchange` | Concrete trusted-issuer HTTP exchange | Operator bridge and absence of official endpoint |
|  | `insecure-plaintext-exchange` | Allows plaintext local test exchange | Test-only, never production; threat warning |

No other current workspace package declares Cargo features. That absence is
itself useful: for example WebDAV/P2P/plugin crates are separate experimental
packages rather than hidden daemon feature toggles.

## Binary target inventory (all 6)

| Binary | Source | Role | Missing atlas detail |
|---|---|---|---|
| `pcloudd` | `crates/pcloud-daemon/src/main.rs` | Ordinary cross-platform per-user daemon | Full modes, startup services, exit/failure behavior |
| `pcloudc` | `crates/pcloud-cli/src/main.rs` | User/operator CLI | Complete command and reachability matrix |
| `pcloud-web` | `crates/pcloud-web/src/main.rs` | Optional operator HTTP UI | Routes/auth/bind/reverse-proxy model |
| `pcloudd-svc` | `crates/pcloud-daemon-win/src/main.rs` | Experimental Windows SCM host | Why unshipped; service/session/SID limitations |
| `pcloud-compat-shm-peek` | `crates/pcloud-compat/src/bin/shm_peek.rs` | Feature-gated legacy SHM diagnostic | `legacy-shm`, platforms, safe/debug usage |
| `xtask` | `xtask/src/main.rs` | Authoritative repository CI/CD runner | Commands, prerequisites, partial flags, Windows SSH and release semantics |

## Protocol and internal subsystem gaps

The following groups are present in source and must each appear in an
explanatory capability map. The parenthesized names are the complete current
module-level inventory, not proposed marketing labels.

| Owner | Subsystems that need explicit feature coverage |
|---|---|
| `pcloud-proto` | account, auth, backup, crypto, diff, folder, notifications, public links, shares, sync, transfer/upload/download; binary API, request/response types, transport, TLS, resilient transport, redaction, HTTP download, async transfer, parallel download |
| `pcloud-backends` | account, auth, backup, crypto, folder, notifications, public links, shares, sync, transfer; RemoteFs, path resolver, ignore patterns, mount discovery, residency, snapshot, upload journal, upload sessions/state, mocks |
| `pcloud-daemon` | account scope, bootstrap, dispatch/runtime, auth vault and DPAPI/file/Keychain/Secret-Service/Windows-secure-file backends, config reload, session refresh, sync loops, mount runtime, HA lease, audit verifier, integrity sweeper, health/metrics, bandwidth schedule, rate limit, metered network, power, transport factory, serve/signals/drain |
| `pcloud-engine` | diff events/poller, filesystem events, local scan, planner, scheduler, conflict resolver, reconcile worker, divergence sweeper, recovery, selective sync, session manager, stall detection, power, bandwidth, upload/download/differential transfer |
| `pcloud-fs` | backend/errors, inode/path normalization, fs watcher, metadata/page caches, read path, staging, journal/write journal/write path/writeback, mount/service/orphan cleanup, FUSE adapter/shim, integrity/SLO hooks, Linux/macOS/Windows/BSD native adapters and FFI/mount discovery |
| `pcloud-crypto` | content, keys, metadata, folder policy, password scorer, crypto policy/state/utilities, pCloud KDF/RSA/sector/modes/auth-tree/filename/profile, share RSA/temppass, backend/mode/provider selection |
| `pcloud-config` | API/auth/env/loader/schema/migration/paths/runtime, features/extensions, mount/sync, limits, resilience/rate/bandwidth/metered transport, observability/traceparent, audit/integrity, HA/residency/KMS/upgrade |
| `pcloud-store` | migrations/schema/transactions/retry/integrity and account, audit, diff-state, file-metadata, preferences, settings, sync-graph, upload-resume, values repositories |
| `pcloud-observability` | logging, audit chain, health, metrics/exporter, SLOs, tracing, lock-poison handling |
| `pcloud-auth` / `pcloud-session` | commands/events/state/lifecycle/manager/orchestrator/refresh plus deterministic session supervisor and refresh loop |
| `pcloud-cache` | checksum cache, generic page cache, eviction, staging, sealed blobs and cache cipher |
| `pcloud-plugin-*` | manifest/registry, host bus, Wasmtime backend, autoheal, DLP, link expiry, backup schedule |
| Helpers | rsync rolling/signature/delta, P2P discovery/policy/transfer status, supervisor registry/spawner, mock/chaos/live test support |

## Platform and integration coverage required

| Target/integration | Required feature documentation beyond the existing table |
|---|---|
| Linux | AF_UNIX/`SO_PEERCRED`, Secret Service/file vault fallback, FUSE, mountinfo, systemd user/system units, AppArmor/SELinux, deb/nfpm/AppImage/Flatpak/Snap/Docker, native gate evidence |
| macOS | `getpeereid`, Keychain, fuse-t FFI, mount discovery, LaunchAgent, Homebrew, pkg/dmg, entitlements, signing/notarization/stapling, native evidence |
| Windows | named pipe/DACL/TokenUser SID, DPAPI, WinFSP, ordinary per-user daemon, experimental SCM host, WiX MSI/Burn, Chocolatey/Scoop/WinGet, signing, native evidence |
| FreeBSD | FUSE/fuser, `getpeereid`, rc.d, package candidate and native mount/service gate |
| NetBSD | native FUSE/device, `getpeereid`, rc.d/pkgsrc candidate and gate |
| OpenBSD | fusefs, `getpeereid`, rc.d/ports candidate and gate |
| DragonFly BSD | fusefs, `getpeereid`, rc.d/native artifact and gate |
| illumos/OmniOS/Solaris | `getpeerucred`, owner-file vault, explicit no-mount contract, SMF/generic Unix packaging and native API/CLI/service gate |
| Synology | SPK builder, package-local persistence/supervisor, FUSE policy, hardware lifecycle matrix |
| QNAP | QPKG builder/config/start-stop, persistent roots, hardware lifecycle matrix |
| ASUSTOR | APK/config/icon/start-stop, persistent roots, hardware lifecycle matrix |
| Generic services | systemd, launchd, rc.d, OpenRC, runit, s6, dinit, SysV, SMF, wrapper/env contract |
| Distribution/security | generic Unix tarball, reproducibility, signing, uninstall/upgrade, permissions, service user, policy assets |

Each page must have two status columns: **implementation exists** and
**release/native qualification evidence exists**. They are not interchangeable.

## Verification-only and helper coverage required

These categories need a “what it proves / prerequisites / what it does not
prove” matrix linked back to feature pages:

- unit and crate-integration tests;
- property tests for framing, state machines, retry/circuit breaker, sync/path
  resolution, secrets, crypto sectors, and method round trips;
- **14 fuzz targets**: root IPC request/public-link URI/transport frame;
  crypto sector and filename decode; daemon vault decode; IPC frame; protocol
  auth state, binary request, IPC method, JSON response, listfolder response,
  path canonicalization, and response parser;
- `pcloud-mockserver` deterministic flows;
- `pcloud-chaos` blackhole, clock jump, disk-full journal,
  SIGKILL-mid-flush, and slowloris scenarios;
- `pcloud-live-e2e` account utilities (including destructive), auth/TFA,
  backup, crypto/password rotation, drain, selectors, fleet mTLS, integrity,
  Linux mount, public links, rate limits, single- and multi-account shares,
  team share, snapshots/prune, sync, transfers/server copy, and Windows
  liveness;
- native FUSE/fuse-t/WinFSP and cross-platform IPC/vault/mountinfo tests;
- DR drills: store corruption, mass sync-root eviction, vault loss;
- 14 current Criterion benches covering secrets, protocol/IPC, store, engine,
  filesystem paths, daemon startup/dispatch/vault, and embedded upload session;
- runnable examples for config, cache, secrets, IPC, crypto, and embedded SDK;
- coverage floor, mutation configuration, memory profile gate,
  reproducibility/self-tests, package validation, supply-chain audit/deny, and
  shell/YAML/link/version gates;
- disabled GitHub workflow archive as migration history only, versus active
  `cargo xtask` execution.

Test-helper types such as fixed/manual clocks, capturing notifiers, no-op
backends/providers, stub HTTP servers, and mock transports should be labeled
**test support**, even when they are public Rust items.

## Source-truth contradictions to resolve before writing feature pages

1. `pcloud-policy` introductory docs still describe `RegoPolicyEngine` as a
   stub, while the same file contains a real `regorus::Engine`, policy loading,
   evaluation, reload, default-deny translation, and tests.
2. `pcloud-p2p` says both “no networking/nothing advertised” and later claims
   a real mDNS discovery runtime. The actual `discovery.rs` behavior and
   daemon wiring must be verified and stated once; peer transfer remains
   absent/scaffolded.
3. `pcloud-crypto` introductory parity prose says password change/team crypto
   are not mirrored, while later code contains backend-aware password change,
   RSA/share helpers, compatibility profiles, and KMS rewrap. Coverage must be
   derived from current reachable paths, not that stale paragraph.
4. `pcloud-fleet` describes missing actual HTTPS transport but also contains
   mTLS agent/reference-server code. Document exactly what is test/reference
   implementation versus daemon-wired production fleet transport.
5. Generated maturity rules classify `xtask` as experimental/bounded although
   its source and `.github/workflows/README.md` define it as authoritative
   local CI/CD. Tooling maturity needs its own category.

These are documentation inconsistencies, not automatically code defects.
They make a source-reconciled feature matrix necessary.

## Recommended site structure

Keep the current source inventory, but add an audience-and-capability layer
above it. Recommended `SUMMARY.md` structure:

```text
1. Start here
   - Product map and maturity legend
   - Choose an interface
   - Single-user, multi-account, team, enterprise deployment chooser

2. User features
   - Authentication, token persistence, TFA and recovery
   - Remote files: stat/list/get/put/cat/copy/move/delete/mkdir
   - Sync roots, selective sync, exclusions, conflicts and differential sync
   - Mounted drive
   - Sharing, contacts, teams and share requests
   - Public links, upload links, bookmarks and notifications
   - Backup/devices and snapshots
   - Crypto Folder (user view)
   - CLI complete command reference
   - Web UI
   - Experimental WebDAV

3. Developer/library interfaces
   - Stable pcloud-sdk
   - Embedded SDK
   - IPC protocol and method matrix
   - Remote pCloud protocol/API families
   - RemoteFs contract
   - Error model and compatibility/versioning
   - Examples and integration recipes

4. Crypto and key management
   - Crypto architecture and lifecycle
   - PclsyncCompat interoperability profile
   - Enhanced profile
   - Content sectors, names, metadata and key hierarchy
   - Password change, lockout and recovery
   - Crypto sharing/team flows
   - Raw versus KMS mode
   - AWS KMS, Vault, PKCS#11 status
   - RustCrypto versus AWS-LC/FIPS providers
   - Threat model, limitations and verification

5. Runtime internals
   - Daemon bootstrap/composition/background services
   - Auth/session lifecycle
   - Backends and protocol ownership
   - Sync engine and rsync delta pipeline
   - Store schema/repositories and caches
   - Transfer/retry/rate/bandwidth/metered behavior
   - Mount read/write/writeback/journal pipeline
   - Observability, audit, metrics, SLOs and tracing
   - Configuration/reload/path model

6. Enterprise and multi-user
   - Capability ladder: user -> account -> multi-account -> team -> fleet
   - Multi-account supervisor and account-scoped daemon topology
   - Business/team sharing
   - OIDC/PKCE identity broker and trusted-issuer exchange
   - Policy/Rego enforcement
   - Fleet identity, mTLS heartbeat and signed commands
   - KMS/HSM
   - HA, drain and rolling handoff
   - Data residency, DLP and integrity
   - Enterprise audit/trace/SLO operations

7. Plugins and extensions
   - Manifest/signature/capability trust model
   - Host bus and Wasmtime runtime
   - Auto-heal
   - DLP
   - Public-link expiry
   - Backup schedule
   - Build/register/test a plugin

8. Platforms and operations
   - Capability/qualification matrix
   - One chapter per Tier-1 OS family
   - One chapter per NAS vendor
   - Service-manager matrix
   - Package/installer/signing/reproducibility matrix
   - Upgrade, uninstall, backup/restore and disaster recovery

9. Build configuration
   - All Cargo feature flags
   - Runtime configuration fields and environment variables
   - Build profiles, MSRV/provider/platform combinations

10. Verification and contribution
   - Authoritative cargo xtask pipeline
   - Unit/property/integration matrix
   - Mock/chaos/DR matrix
   - Fuzz target matrix
   - Live/native credential and hardware matrix
   - Bench/coverage/mutation/memory/reproducibility
   - Feature documentation acceptance checklist

11. Generated source reference
   - Current crate/file/declaration inventories
```

## Required feature-page template

Every feature page should contain the same fields so “complete” is measurable:

1. **What it is** — one precise capability statement.
2. **Why it exists** — problem and alternatives/trade-offs.
3. **Strengths** — concrete properties, not marketing adjectives.
4. **Use cases** — user, developer, sysop, enterprise as applicable.
5. **Entry points** — CLI, SDK, IPC, daemon, protocol, config, source paths.
6. **Architecture/data flow** — owner and dependencies, with one schematic.
7. **State and failure behavior** — persistence, retries, idempotency, recovery.
8. **Security/crypto** — secrets, trust boundary, permissions, threats.
9. **Platforms** — portable versus native seams and packages.
10. **Maturity** — implemented, reachable, shipped, qualified, experimental,
    scaffolded, test-only; use separate booleans rather than one vague tier.
11. **Limitations/non-goals** — explicit missing pieces.
12. **Evidence** — unit/mock/live/native/package tests and prerequisites.
13. **Related features** — cross-links and incompatibilities.

## Recommended implementation order

1. Generate the cross-surface capability matrix from CLI/IPC/Cargo metadata;
   add hand-authored status/rationale fields.
2. Write the crypto/KMS/provider section and reconcile stale crypto prose.
3. Write the enterprise/multi-account capability ladder and topology.
4. Write protocol/backend/internal pipeline maps.
5. Write plugin architecture and built-in plugin pages.
6. Replace generic feature listings with the complete Cargo flag reference.
7. Expand platform/package/service pages and native evidence columns.
8. Add the verification-to-feature traceability matrix and `xtask` guide.
9. Make the generator fail when a Cargo package, binary, feature flag, CLI
   command family, IPC request family, platform target, or test-helper category
   lacks a documentation-owner entry.

## Completion criteria

The new requirement is met only when:

- all 42 packages appear in both generated reference and an explanatory
  ownership/capability map;
- all 37 feature flags and all 6 binaries have explicit behavioral status;
- every CLI/IPC feature family maps to its backend/protocol/store/SDK path;
- every crypto profile/mode/provider and KMS option has a decision table;
- single-user, multi-account, business/team, and enterprise/fleet flows are
  separately explained;
- every internal subsystem group above has rationale, entrypoint, and failure
  behavior coverage;
- every target/package distinguishes implementation from native/release
  qualification;
- test-only helpers cannot be mistaken for shipped features;
- known source-documentation contradictions are reconciled;
- a coverage check detects future undocumented packages/features/commands.

Until then, the atlas should continue to describe itself as an architecture
and source-navigation site, not as exhaustive feature documentation.

## Closure re-audit — 2026-07-17 (final)

The six focused gaps identified in the first closure pass are now closed.
The final automated checks report **42/42 packages**, **37/37 Cargo features**,
**186/186 C-parity decisions**, **139/139 CLI commands**, **44/44 IPC methods**,
**113/113 IPC requests**, **6/6 binaries**, **499/499 Cargo-owned Rust source,
test, and helper units**, **2,116 project files**, and **14 feature chapters**.
The link checker resolves **328/328 local targets**, and `mdbook build` succeeds.

| Prior gap | Final status | Closure evidence |
|---|---|---|
| Current CLI/API/IPC catalog was not exhaustive | **Closed** | The generated [current surface catalog](src/generated/features/current-surfaces.md) covers every current `Command`, IPC `Method`, IPC `Request`, and binary. CLI rows map purpose and use to their IPC or local route, runtime owner/side effect, platform, and maturity. IPC rows expose behavior, rationale, owner/effect, CLI/stable-SDK/embedded-SDK reachability, and the wired runtime arm. The separate 186-row C-parity catalog remains clearly identified as a compatibility view rather than the current Rust interface inventory. |
| Coverage checker did not enforce interfaces, binaries, platforms, or verification categories | **Closed** | [`check_feature_coverage.py`](tools/check_feature_coverage.py) extracts current enum variants and command routes, requires every CLI command to have an IPC or local route, verifies all `Command`, `Method`, and `Request` variants and all six binaries in the generated catalog, checks required platform and verification categories, and retains package, flag, C-parity, Rust-unit, file-inventory, navigation, and generated-Markdown checks. |
| Enterprise runtime wiring was overstated | **Closed** | [Collaboration and enterprise](src/features/collaboration-enterprise.md), [runtime internals](src/features/runtime-internals.md), and package guidance now distinguish implemented standalone contracts and engines from default `pcloudd` reachability. Policy, DLP, fleet, IdP, supervisor, plugin-host, and Wasmtime paths are explicitly documented as not wired into the default daemon where that is the source truth; fail-closed policy behavior is identified as an integrating caller's responsibility. |
| Fleet transport was inaccurately called classic mTLS | **Closed** | Enterprise, Cargo-flag, package, and verification guidance now state the actual trust model: rustls HTTPS authenticates the controller with the configured CA, while the device uses Ed25519-signed HTTP identity headers and verifies signed commands. `MtlsFleetAgent` is retained as a historical/API name, but `with_no_client_auth()` means the transport does not present a TLS client certificate and is not classic mutual TLS. |
| P2P discovery maturity was ambiguous | **Closed** | [Interfaces and automation](src/features/interfaces-automation.md), runtime internals, and package guidance now state directly that the LAN P2P implementation is an inert scaffold: `start` opens no network or mDNS socket, advertises nothing, and `peers()` remains empty; there is no peer inventory, planner, authentication, or transfer path, and it is not wired into the default daemon. |
| Platform lifecycle and mutation-testing detail was compressed | **Closed** | [Platform operations](src/features/platform-operations.md) now provides explicit lifecycle playbooks for Linux, macOS, Windows, FreeBSD, NetBSD, OpenBSD, DragonFly BSD, illumos/OmniOS, and Oracle Solaris 11.4, plus separate Synology DSM, QNAP QTS/QuTS hero, and ASUSTOR ADM build/install/start/upgrade/recovery/uninstall and hardware-qualification guidance. [Verification helpers](src/features/verification-helpers.md) documents `.cargo/mutants.toml`, its timeout/exclusions/comment-only target, the local command, evidence limits, and that neither `xtask ci` nor `xtask release` enforces mutation testing. |

### Final verdict

**Feature-coverage closure complete.** No concrete documentation blocker remains
within the six-gap scope. The atlas now provides exhaustive structural, source,
binary, and current-interface navigation while accurately separating implemented
code, default-runtime reachability, and platform or release qualification.

This verdict is about documentation coverage and source reconciliation. It does
not claim that the library and applications have completed native, live-service,
package-installation, or release qualification on every supported platform; the
atlas preserves those evidence boundaries and maturity caveats explicitly.
