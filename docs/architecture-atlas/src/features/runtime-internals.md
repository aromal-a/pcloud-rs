# Runtime, storage, resilience, and internal helpers

This chapter documents the features an end user may never name but every
reliable request depends on. Their purpose is architectural: keep data types,
configuration, protocol, state, side effects, and recovery in separate owners
so one feature can be reasoned about without reading the entire daemon.

## Foundation types

### `pcloud-model`

| Module/feature | Why it exists | Good for, and why |
|---|---|---|
| Typed IDs | Distinguishes account, file, folder, share, sync, transfer, and related identifiers from arbitrary integers. | Cross-layer safety: a file ID cannot casually be passed where a folder/share ID belongs. |
| Auth model | Represents user/session/challenge data without transport ownership. | Auth state, IPC, stores, and UI can share vocabulary. |
| File/folder metadata | Describes remote namespace values independent of cache/protocol/UI. | RemoteFs, mount, sync, SDK conversion, and test fixtures. |
| Conflict model | Names divergence/conflict kinds and outcomes. | Planner, UI, and persistence agree on preservation semantics. |
| Crypto model | Carries non-secret Crypto state/results across layers. | IPC/CLI status without exposing keys. |
| Health model | Defines build/runtime health payloads. | Daemon, web, CLI, supervisors, and fleet use one status shape. |
| Public-link model | Represents link IDs, options, policies, and summaries. | Protocol, backend, IPC, CLI, SDK, and plugins avoid opaque JSON. |
| Share model | Represents contacts, requests, active shares, targets, and permissions. | Consumer and business/team collaboration with typed target classes. |
| Sync model | Defines roots, directions, states, and summaries. | Store, engine, IPC, CLI, and UI agree on lifecycle. |
| Transfer model | Defines transfer IDs/state/progress and receipts. | Progress, pending work, journals, and automation. |

The crate contains pure serializable vocabulary and no network, database, or
process ownership. That keeps it reusable and prevents a data type from
quietly performing side effects.

### `pcloud-error`

The shared error taxonomy exists to replace global `last_error`, raw errno,
and string matching. Stable codes/categories are good for IPC, CLI exit-code
mapping, SDK errors, retry classification, and audit-safe support messages.
Structured variants preserve cause and context; display strings are not the
programmatic contract and must remain secret-free. Each boundary translates
errors once rather than leaking lower-layer implementation types upward.

### `pcloud-secret`

Secret wrappers, explicit exposure, zeroization, redacted formatting, no
serde/Clone, and constant-time comparison are fully explained in
[Cryptography, secrets, and key custody](crypto.md). They are a foundation
feature used by auth, protocol, IPC, vaults, KMS, IdP, CLI prompts, and Crypto.

## Configuration architecture (`pcloud-config`)

Configuration is executable policy, not a loose bag of strings.

```text
secure defaults
   ↓
versioned profile file ── permissions + schema + migrations
   ↓
targeted PCLOUD_* overrides
   ↓
cross-field validation
   ↓
immutable/controlled runtime profile
```

| Module/feature | Why it exists | Good for, and why |
|---|---|---|
| Profile/environment | Selects development/test/production defaults and groups all subsystem config. | Reproducible deployment and tests; one value can be validated as a whole. |
| Loader | Reads a profile, checks permissions, parses, migrates, applies overrides, and validates. | Safe startup: malformed/insecure config fails before services or credentials start. |
| Schema validator | Rejects unknown/wrongly typed/out-of-range settings. | Catching typos that would otherwise silently disable security or durability. |
| Versioned migrations | Converts older profile envelopes forward. | Non-destructive upgrades with explicit schema epochs and tests. |
| Environment overrides | Maps documented `PCLOUD_*` values to typed fields. | Containers, CI, and packaging without editing files. Parsing errors are surfaced, not ignored. |
| Canonical paths | Resolves config/state/runtime/cache/plugin directories and optional legacy migration. | Correct per-user/platform layout and test/multi-instance isolation. |
| Runtime permissions | Defines owner-only runtime directory requirements. | Protecting IPC endpoints, pidfiles, and temporary control state. |
| API/TLS | Sets mode, host, port, SNI, connect/read timeouts, and revocation posture. | EU/US routing and secure transport; plaintext-in-production combinations are rejected. |
| Auth/vault | Selects auto/file/Keychain/DPAPI/Secret Service and durable-token policy. | Platform-native secret storage with explicit fallback semantics. |
| Product features | Holds runtime feature decisions distinct from Cargo compilation. | Turning behavior on/off without pretending unavailable compiled code exists. |
| Extensions | Enables plugins and grants network/sync/Crypto capabilities plus trusted signing keys. | Least-privilege extension deployment. Cross-field validation blocks grants when plugins are disabled. |
| Mount | Sets cache sizes, metadata TTL, options, and optional auto-mount path. | Predictable memory/security behavior for native drives. |
| Sync loop | Configures polling/ticks/concurrency and background reconciliation. | Tuning desktops versus servers while one engine owns semantics. |
| Limits | Bounds memory, concurrency, queue, and resource consumption. | Preventing large accounts or hostile clients from exhausting the daemon. |
| Resilience | Configures retry/backoff/circuit/timeout behavior. | Adapting to site/service reliability without changing protocol clients. |
| Rate limits | Sets per-category operation budgets. | Separating cheap status traffic from expensive or mutation-heavy requests. |
| Bandwidth schedule | Defines time windows and transfer limits. | Night/day and metered-network policy. |
| Transport selector | Chooses/falls back among supported transport modes. | Controlled experimentation and platform compatibility without silent unsafe downgrade. |
| Observability | Enables log/metric/trace behavior. | Lean personal defaults and richer managed deployments from one schema. |
| Traceparent | Parses/creates W3C trace context. | Cross-process/service request correlation with strict syntax. |
| Audit verifier | Schedules audit-chain verification. | Early detection of tampering in long-running installations. |
| Integrity sweeper | Sets schedule, pause-on-battery, and scan policy. | Background corruption detection without harming foreground use. |
| HA | Defines node/lease/renew/fencing-like active-passive behavior. | Managed redundant daemon deployments. |
| Data residency | Defines allowed regions and warn/strict behavior. | Jurisdiction-aware client-side refusal/audit. |
| Crypto KMS | Selects provider, key/module/address, cache, and secret environment names. | External key custody without putting credentials in profile files. |
| Upgrade/handoff | Sets drain/handoff timeouts and upgrade policy. | Controlled daemon replacement. |

## Authentication and session internals

| Unit | Why it exists | Good for, and why |
|---|---|---|
| Auth commands | Represent external stimuli—begin, password, token, TFA, refresh, logout—as data. | Deterministic state-machine tests and one transition entrypoint. |
| Auth states | Name unauthenticated, collecting, challenge, authenticated, refresh, and failure state. | Rejecting invalid operations before protocol I/O. |
| Auth events | Emit safe state changes without credentials. | CLI/UI/daemon observation and audit without secret leakage. |
| Session manager | Owns state plus current session and executes commands. | One authority prevents concurrent callers from inventing state. |
| Protocol orchestrator | Bridges pure auth decisions to real pCloud calls. | Keeping I/O out of transition logic while still supporting real login/TFA. |
| Lifecycle policy | Tracks expiry, idle logout, and refresh thresholds. | Long-running services and conservative security policy. |
| Refresh coordinator | Ensures one refresh attempt and applies success/failure coherently. | Avoiding refresh storms and split session state. |
| Session refresh loop | Drives timed lifecycle ticks and vault synchronization from the daemon. | Background continuity without embedding timers in auth primitives. |

## Backend business layer (`pcloud-backends`)

Backends turn typed protocol calls into product behavior and errors. They are
reusable without becoming a process-global daemon.

| Backend/helper | Why it exists | Good for, and why |
|---|---|---|
| Account backend | Orchestrates registration, verification, lost/change password, language, API servers, and promotions. | Account lifecycle with consistent auth/error handling. |
| Auth backend | Drives password/token/TFA/recovery protocol operations. | Real login orchestration behind the pure auth state machine. |
| Folder backend | Owns list/metadata/create/rename/move/delete/copy and typed folder errors. | RemoteFs and all namespace consumers. |
| Transfer backend | Owns signed links, HTTP bytes, upload sessions, progress, integrity, and bandwidth/resilience. | Upload/download without duplicating control/data path policy. |
| Sync backend | Validates and persists sync-root lifecycle and syncability. | CLI/daemon setup distinct from engine reconciliation. |
| Backup backend | Wraps backup/device API lifecycle. | Deletion-safe/backup-specific control semantics. |
| Notifications backend | Lists/marks notifications. | User/plugin notification flows. |
| Public-link backend | Owns file/folder/tree/upload link lifecycle and options. | Controlled public exposure and link cleanup. |
| Path resolver | Resolves mixed public-link selections from remote paths. | Tree links without trusting cache-only metadata. |
| Shares backend | Owns contacts, requests, active shares, business/team methods, and Crypto-share transport integration. | Collaboration with target/permission-specific errors. |
| Crypto backend | Coordinates server Crypto password-change/key blob transport around primitive operations. | Keeping email OTP/API state out of `pcloud-crypto`. |
| `RemoteFs` | Gives every drive-like consumer one live ID-first namespace and durability contract. | CLI/SDK/sync/mount/gateways; fully explained in [RemoteFs](../remote-fs.md). |
| Ignore patterns | Compiles/applies sync filename globs. | Consistent exclusion across scanner/backend. |
| Mount discovery | Finds mountpoints and ignore paths during syncability checks. | Preventing recursive self-sync and mount conflicts. |
| Residency | Resolves/caches/evaluates region and emits audit outcomes. | Client-side jurisdiction policy. |
| Snapshot | Builds manifest/tar/database/GPG artifacts and retention support. | DR and deletion-safe backups. |
| Upload state | Models resumable upload lifecycle and transitions. | Correct restart/retry decisions. |
| Upload journal | Appends crash-recoverable transfer intent/acknowledgments. | Power/process failure recovery. |
| Upload session registry | Controls active sessions and status. | Pause/resume/cancel/list/drain. |
| Backend mocks | Supply deterministic protocol behavior. | Integration tests without real accounts. |

## Daemon composition and background services (`pcloud-daemon`)

| Service/unit | Why it exists | Good for, and why |
|---|---|---|
| Bootstrap | Creates validated paths/config, store, vault, transports, backends, RuntimeShell, and services in dependency order. | One auditable composition root and deterministic startup failure. |
| RuntimeShell | Owns live auth, composed backends, store, engines, mount, and process state. The default daemon does **not** own the standalone policy, plugin, fleet, IdP, or multi-account supervisor crates. | Authoritative mutation and status for the actually composed personal/runtime surface; callers cannot create global duplicates. |
| Dispatch | Maps every typed IPC request to category/rate/drain checks, validation, backend/engine work, audit/metrics where present, and response. No Rego/plugin/fleet/IdP enforcement is currently composed here. | One reachability/security choke point whose current dependencies are visible rather than aspirational. |
| Serve loop | Binds native IPC, authenticates peers, frames requests, and invokes dispatch until shutdown. | Portable local service boundary. |
| Signals | Converts supported process signals into reload/drain/shutdown behavior. | Service-manager and terminal lifecycle on Unix. Windows uses native control paths. |
| Account scope | Roots configuration/state/IPC by account. | Tests and future isolated multi-account sub-daemons. |
| Auth vault shim | Preserves compatibility around the newer platform vault abstraction. | Incremental migration without two secret persistence policies. |
| Config reload | Validates/safely applies reloadable settings on SIGHUP. | Operator changes without process kill. Non-reloadable state remains explicit. |
| Session refresh | Integrates refresh ticks/expiry/vault with runtime authentication. | Long-lived sync/mount continuity. |
| Sync loop | Schedules autonomous reconciliation. | Sync that runs without an attached CLI. |
| Sync-loop runtime bridge | Connects pure engine work to RemoteFs/store/backends. | Reusable engine without hiding side effects. |
| Mount runtime | Composes native mount adapter with canonical RemoteFs and owns handles. | One mount authority and cleanup. |
| HA lease | Coordinates active/passive ownership. | Redundant managed deployments without double writers. |
| Audit verifier service | Periodically verifies tamper-evident audit records. | Detecting historical corruption. |
| Integrity sweeper service | Schedules bounded scans with power policy. | Latent-corruption detection. |
| Bandwidth schedule applier | Updates the active transfer pacer from time/network policy. | Dynamic bandwidth limits without restarting transfers. |
| Metered network | Supplies best-effort cost classification. | Avoiding expensive background transfer on constrained links. |
| Power source | Supplies battery/AC classification. | Pausing expensive sweeps/sync on battery. |
| Per-session category limiter | Limits IPC request families. | Protecting daemon/account from one local client or expensive action flood. |
| Transport factory | Selects validated protocol transport/resilience composition. | Consistent TLS/timeouts/retry and controlled fallback. |
| Health server | Offers a minimal supervisor probe independent of the richer web UI. | Kubernetes/systemd/NAS health checks. Bind exposure still requires policy. |
| Metrics server | Feature-gated Prometheus export tied to runtime lifecycle. | Managed observation without adding a second daemon. |
| Vault backends | Select file, Keychain, DPAPI, Secret Service, or Windows secure file. | Platform-native durable auth tokens. Fully detailed in [Crypto](crypto.md). |

## Persistent store (`pcloud-store`)

SQLite is durable local state, not a remote authority. WAL/transactions,
schema versions, integrity checks, and repository boundaries make crash and
upgrade behavior explicit.

| Unit/repository | Why it exists | Good for, and why |
|---|---|---|
| Schema/bootstrap | Creates the full current database shape. | Fresh install and deterministic tests. |
| Migrations | Advances old schema versions transactionally. | Upgrade without discarding state. |
| Transactions | Provides commit/rollback ownership. | Multi-row invariants and all-or-nothing mutation. |
| Busy/locked retry | Classifies SQLite contention and applies bounded backoff. | Concurrent background/client work without infinite blocking. |
| Integrity helper | Runs/normalizes SQLite integrity checks. | Startup/maintenance corruption detection. |
| Account repository | Stores non-secret account metadata/state. | Binding local state to the expected account. |
| Audit repository | Appends/verifies tamper-evident chained events. | Compliance/investigation with local tamper detection. |
| Diff-state repository | Persists remote diff cursor per sync root. | Restartable remote-change polling. |
| File-metadata repository | Caches local/remote metadata for planning and acceleration. | Sync/mount performance; never authoritative absence. |
| Preferences repository | Persists typed user preferences. | UI/behavior that survives restart. |
| Settings repository | Mirrors compatible typed setting families. | Migration and daemon behavior. |
| Sync-graph repository | Persists roots, relationships, and sync state. | Restartable reconciliation and conflict prevention. |
| Upload-resume repository | Persists source/destination/session/offset identity. | Safe large-upload resume. |
| Values repository | Provides typed bool/int/string/blob-like legacy value helpers. | Compatibility without raw SQL throughout callers. |

## Cache internals (`pcloud-cache`)

Page/checksum caches, staging, eviction, and sealed local blobs are covered in
[Transfers, sync, backup, and mounted drives](sync-mount-transfer.md). The
important internal rule is that cache APIs are bounded and optional; no cache
miss is translated into remote `NotFound`.

## Resilience (`pcloud-resilience`)

| Primitive | Why it exists | Good for, and why |
|---|---|---|
| Injectable clock | Makes timing state machines deterministic in tests. | Retry, breaker, limiter, and timeout behavior without real sleeps. |
| Retry policy/backoff | Classifies attempts and schedules exponential/other delays with jitter. | Transient idempotent failures. It does not bless non-idempotent replay. |
| Global retry budget | Shares a finite token pool across requests. | Preventing many individually reasonable retries from creating a storm. |
| Circuit breaker | Opens after failures, probes half-open, and closes on recovery. | Fast failure and upstream protection during outages. |
| Rate limiter | Token-bucket admission. | API categories, local clients, and service quotas. |
| Bandwidth pacer | Token-bucket byte pacing. | Network fairness and schedules. |
| Metered detector | Supplies best-effort network-cost input. | Laptop/mobile policy without hard dependency on a platform framework. |
| Timeout | Adds cancellation-safe Tokio timeout only when feature-enabled. | Existing async consumers; blocking users do not inherit Tokio by default. |
| Resilient transport | Composes retry, breaker, timeout, and metrics around HTTP work. | Consistent external-service behavior. Operation idempotency remains the caller's responsibility. |
| Transport metrics | Records attempts, failures, breaker states, and latency through canonical observability. | SLOs and diagnosis of resilience behavior itself. |

## Observability (`pcloud-observability`)

| Feature | Why it exists | Good for, and why |
|---|---|---|
| Structured logging | Emits levels/targets/fields with redaction discipline. | Human and machine diagnosis. Business logic does not choose a log backend. |
| JSON logs | Optional stable machine representation. | SIEM and containers. Feature gating keeps default dependencies smaller. |
| Audit envelope | Records security/business actions separately from debug logs. | Compliance and replayable decision history. |
| Health/build report | Exposes version/build/runtime summaries. | Supervisors and support. No credentials are included. |
| Metric families | Defines canonical counters/gauges/histograms. | Comparing CLI, mount, sync, transfer, policy, and resilience behavior. |
| Prometheus exporter | Renders text format over a small HTTP responder. | Scrape-based monitoring without a full web dependency. |
| SLO catalog | Turns metrics into named target/pass reports. | Fleet and release gates. Operators still choose consequence/escalation. |
| OTLP tracing | Creates/exports spans through optional OpenTelemetry stack. | Multi-service causality and latency. Heavy dependencies are optional. |
| Poison-safe lock extension | Converts poisoned mutex access into a controlled recovery/reporting path. | Long-running metrics/audit code where a panic should not cause silent future panics. |

## Experimental/internal helper families

| Family | Why it exists | Current truth |
|---|---|---|
| `pcloud-rsync` | Differential block signatures/deltas reduce retransmission. | Algorithm/source exists; full pCloud differential wire integration/performance proof remains bounded. |
| `pcloud-p2p` | Reserves LAN policy/lifecycle types for a possible future accelerator. | Inert scaffold: `start` opens no socket, advertises nothing, `peers()` is always empty, and no planner or transfer exists; not wired into pcloudd. |
| `pcloud-supervisor` | Models isolated multi-account processes and routing. | Scaffold, not the supported default runtime. |
| `pcloud-plugin-*` | Lets optional behavior evolve behind signed/capability message boundaries. | Standalone API/host/Wasmtime/built-ins have code, but pcloudd has no plugin dependencies or RuntimeShell plugin field: none is wired into the default daemon. |
| `pcloud-compat` | Contains exact legacy ABI helpers. | Non-canonical and non-default; use only for migration/tests. |

For every private helper function, serde shim, test fixture, mock, and module
not individually repeated above, use the generated [complete internal module
and helper catalog](../generated/features/source-units.md) and its per-crate
declaration links.
