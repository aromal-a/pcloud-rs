# Sharing, multi-user, and enterprise features

There are four different scales in this area:

```text
one account          folder invitations, public links, contacts
two or more people   incoming/outgoing shares and Crypto key exchange
one host, accounts   isolated sub-daemons supervised per account (scaffold)
managed fleet        IdP, policy, DLP, residency, KMS, fleet, HA, audit
```

The distinction matters. A team-share protocol method does not make the local
daemon multi-account, and a multi-account supervisor does not make the pCloud
service a corporate IdP. Each layer owns a different boundary.

The default `pcloudd` currently has no dependency on `pcloud-policy`,
`pcloud-fleet`, `pcloud-idp`, `pcloud-supervisor`, `pcloud-plugin-host`, or
`pcloud-plugin-wasmtime`. Those are real standalone contracts, engines, or
scaffolds, but they are not controls silently enforced by the ordinary daemon.

## Person-to-person folder sharing

| Feature | What and why it exists | Good for, and why | Entrypoint / maturity |
|---|---|---|---|
| Contacts | Lists known pCloud contacts. It exists to choose recipients without manually retyping identities. | Share UX and automation. Contact records stay separate from authenticated local identities. | shares protocol/backend/CLI; implemented |
| Incoming/outgoing shares | Lists active shares in both directions. It exists to inventory who can see what and what was shared to the user. | Access reviews and file browsers. Direction and share IDs are typed. | `ListIncomingShares` / `ListOutgoingShares`; implemented |
| Incoming/outgoing requests | Lists pending invitations before they become active shares. It exists because an invitation and a granted share have different permissions/cleanup. | Approval workflows and tests. Request IDs preserve the lifecycle boundary. | shares backend/protocol; implemented |
| Create folder invitation | Shares only a resolved folder with email, message, and permission bits. It exists for named collaboration rather than anonymous URL access. | Project folders and bilateral workflows. RemoteFs resolves live folder identity; typed permissions avoid magic bit manipulation in public SDK callers. | `RemoteFs::share_folder`, CLI/SDK; implemented |
| Accept/decline/cancel | Completes each side of the pending request state machine. It exists so senders and recipients can undo/refuse invitations safely. | User inbox and automated test accounts. Explicit request IDs make retries and cleanup observable. | share request methods; implemented |
| Remove active share | Revokes a current share. It exists for offboarding and mistaken grants. | Access control and E2E cleanup. Share ID is distinct from folder ID, preventing accidental folder deletion. | `RemoveShare`; implemented |
| Modify permissions | Changes an active share's permission bitmap. It exists to support least privilege without recreating the share. | Read-only to read/write transitions and revocation of manage/delete. | `ModifyShare`; implemented |
| Permission model | Represents read, create, modify, delete, and manage/re-share semantics. It exists to preserve pCloud permission detail across CLI/SDK/IPC. | Human collaboration and policy checks. SDK constants provide safe read-only/read-write defaults while retaining explicit fields. | model share types, public SDK `SharePermissions`; implemented |
| A-to-B live workflow | Exercises creation on account A, request visibility/acceptance on B, active share, and cleanup. It exists because one-account mocks cannot prove identity routing or email/request lifecycle. | Release qualification. Distinct disposable accounts and serial execution reduce ambiguity. | `pcloud-live-e2e` A/B tests; gated verification, invitation acceptance can require out-of-band action |

## Business and team features

| Feature | What and why it exists | Good for, and why | Entrypoint / maturity |
|---|---|---|---|
| List teams | Retrieves teams associated with the authenticated business account. It exists to resolve team targets before sharing. | Organization-aware clients. Team IDs remain distinct from user/receiver IDs. | `ListMyTeams`; implemented API path, business entitlement required |
| Account team share | Grants a folder to a team with explicit name/permissions. It exists because team membership should be managed centrally rather than as many user invitations. | Business accounts and departmental folders. A separate protocol method preserves organization semantics. | `AccountTeamShare`; implemented/reachable, complete live team lifecycle not yet proven |
| Account modify share | Applies batches of user/team permission changes. It exists for administrative policy changes at organization scale. | Offboarding and access reviews. Typed user/team modifications avoid mixing target classes. | `AccountModifyShare`; implemented API path |
| Account stop share | Revokes selected user/team access. It exists as the administrative counterpart to ordinary recipient-controlled share removal. | Incident response and team reorganization. Explicit target lists make the mutation auditable. | `AccountStopShare`; implemented API path |
| Crypto team/user sharing | Wraps compatible folder/file symmetric keys to the recipient RSA key and addresses user/team targets. It exists because encrypted content cannot be shared by permission metadata alone. | Business Crypto folders and interoperable recipients. Key wrapping keeps plaintext folder keys out of invitations. | crypto `share_rsa` + shares backend/protocol; implementation exists, real bilateral/team Crypto qualification remains required |

## Multi-account on one host

<span class="atlas-experimental">Experimental scaffold</span>

| Feature | Why it exists and current behavior | Good for, and why | Current limit |
|---|---|---|---|
| Account registry | Models account labels and per-account state. It exists so “personal” and “work” do not share credentials or databases. | Future multi-account desktop/server use. Explicit account labels make selection deterministic. | `pcloud-supervisor`; not the ordinary daemon's production composition root |
| Account-scoped bootstrap | Carries account-specific paths/identity into daemon construction. It exists to avoid global `~/.pcloud` collisions. | Isolated sub-daemons and tests. Store, vault, runtime, and IPC can be rooted per account. | `pcloud-daemon::account_scope`; internal seam, full routing still bounded |
| Sub-daemon spawner | Starts/stops a process per registered account. It exists because process isolation is stronger and simpler than merging many secret sessions into one RuntimeShell. | Fault and credential isolation. One failed/account-compromised daemon need not expose another account's in-memory state. | `pcloud-supervisor::spawner`; scaffold, service lifecycle/packaging not release-qualified |
| IPC routing model | Selects an account endpoint from a label/environment hint. It exists so clients target one authority explicitly. | Future `PCLOUD_ACCOUNT=work` workflows. Routing metadata stays outside the remote protocol. | Model exists; complete CLI/SDK routing and collision-free native endpoint evidence remain work |

The supported base topology remains one per-user daemon/account. Multi-account
source presence must not be interpreted as a shipped account switcher.

## Federated identity (`pcloud-idp`)

| Feature | Why it exists | Good for, and why | Maturity / caveat |
|---|---|---|---|
| `IdpBroker` trait | Models begin, authorization-code completion, and refresh without offering a password grant. | Okta, Entra ID, Keycloak, Ping, or custom broker implementations. Object safety lets the daemon select an implementation. | Internal enterprise contract |
| OIDC Authorization Code + PKCE | Builds state, nonce, S256 verifier/challenge, browser URL, and token exchange. It exists to keep user passwords at the IdP. | Interactive enterprise SSO. State/nonce/PKCE bind callback to the originating client. | Implemented broker path, requires operator IdP registration |
| Discovery/JWKS cache | Fetches OIDC metadata and RS256 keys, validates issuer/audience/expiry/nonce, and caches keys. It exists to authenticate ID tokens rather than merely decoding JWTs. | Real OIDC issuers and key rotation. RS256-only scope and typed validation narrow accepted algorithms. | Implemented modules; live interoperability depends on issuer behavior |
| HTTP token exchange | Exchanges the code at the IdP token endpoint over HTTPS. It exists to complete the standard flow. | Default secure broker integration. | `oidc-http-exchange` is enabled by default; TLS/redirect/client config needs live proof |
| Insecure plaintext exchange | Provides a deliberately unsafe local test transport. It exists only for deterministic fixtures. | Unit/integration tests. The explicit non-default Cargo flag makes downgrade visible. | Never production |
| pCloud token exchanger | Converts an IdP token through an operator-provided trusted-issuer bridge into a pCloud session. It exists because the public pCloud API does not document native arbitrary-OIDC federation. | Enterprises operating their own bridge. Null exchanger fails with `NotConfigured` rather than pretending SSO works. | Pluggable HTTP seam; no official pCloud exchange means end-to-end pCloud SSO is not a shipped claim |
| Future SAML/LDAP/device flows | Types mention broader enterprise identity patterns. They exist as design extension points. | Future adapters. | Not implemented/broker-ready; never infer support from enum variants |

## Policy enforcement (`pcloud-policy`)

| Feature | Why it exists and behavior | Good for, and why | Maturity / caveat |
|---|---|---|---|
| Policy input | Provides user, command, secret-stripped args, device ID, and time. It exists to evaluate context without exposing tokens/passwords to policy code. | Per-user/path/device/time rules. A stable input makes audit replay possible. | Implemented type; caller must continue stripping secrets |
| Null policy | Allows requests and relies on caller audit. It exists to preserve the simple single-user default and tests. | Development/personal deployments. | Not an enterprise enforcement engine |
| Rego engine | Loads sorted `.rego` files into pure-Rust Regorus and queries one decision document. It exists for organization-authored policy rather than hard-coded conditions. | Standalone policy evaluation and future integration work. Deterministic load/evaluation supports audit. | Implemented engine, but not a `pcloudd` dependency and therefore not enforcing daemon commands today |
| Fail closed | Returns denial/error for missing, malformed, or failed evaluation. It exists so an integrating caller can fail closed. | Regulated integrations built on the engine. | The caller must treat every `Err` as deny; no current daemon dispatch boundary invokes it |
| Secure bundle loading | Rejects group/world-writable policy files on Unix. It exists to stop lower-privileged local users changing authorization. | Server and fleet endpoints. | Non-Unix relies on platform ACL policy and needs native validation |
| Atomic reload | Builds a new engine and swaps only on success. It exists so a bad update cannot erase the last known-good policy. | Policy-engine hosts and future configuration management. | Engine reload implemented; not connected to daemon SIGHUP, and dynamic external data/group resolution remains absent |
| Decision audit | Records allow/deny and safe reason/context outside the dependency-light engine. It exists to make policy behavior explainable. | Investigations and compliance. | Audit chain is local tamper-evidence, not immutable remote SIEM by itself |

## Data loss prevention

| Feature | Why it exists and behavior | Good for, and why | Maturity / caveat |
|---|---|---|---|
| Pre-upload scan | Examines a bounded first-byte prefix before publication. It exists to catch obvious secrets before they leave the endpoint. | Testing DLP rules and future managed upload integration. Bounded input and no network capability limit exposure. | `pcloud-plugin-dlp` logic exists, but it is not wired into pcloudd uploads and currently enforces nothing in the default product |
| Built-in regex rules | Detects AWS access keys, nearby AWS secret material, private-key PEM, JWTs, and password literals. | Common high-impact secret classes. Stable rule IDs support config and audit. | Heuristic: false positives/negatives are expected |
| Entropy rule | Flags very high-entropy text while skipping known compressed/media/container magic. | Unknown token/key material. Binary magic suppression reduces obvious false positives. | Only the sampled prefix is seen; encrypted/compressed/custom formats can evade or trigger it |
| Audit-only mode | Allows but emits safe findings. It exists to tune policy before blocking users. | Initial deployment and false-positive measurement. | Default behavior, not prevention |
| Strict mode | Denies when enabled rules match. It exists for enforceable exfiltration policy. | Mature managed environments. | Scanner timeout/failure policy and daemon integration must be fail-closed to make a production claim |
| Privacy-preserving audit | Emits path SHA-256, rule IDs, and verdict, never raw path/content. | Central audit without copying suspected secrets into logs. | Stable hashes can still correlate repeated paths and may be dictionary-guessed |

## Data residency

| Feature | Why it exists and behavior | Good for, and why | Maturity / caveat |
|---|---|---|---|
| Allowed-region policy | Configures allowed pCloud regions and strict/warn behavior. It exists to stop accidental client routing or uploads outside an approved jurisdiction. | EU/US-regulated deployments. | Implemented config/evaluator; it trusts pCloud-reported region and is not cryptographic attestation |
| Region resolver/cache | Resolves account/folder region and caches for a bounded TTL. It exists to avoid a remote lookup before every sensitive operation. | Upload/sync/link policy. Mismatch invalidation prevents long-lived stale approval. | Backend implemented; service attributes and migration behavior need live qualification |
| Pre-operation enforcement | Evaluates high-value sync-root, upload/public-link paths before side effects. It exists so policy can refuse before bytes/exposure leave the client. | Strict compliance. Typed `PolicyViolation` makes refusal distinguishable. | Several runtime sites are wired; complete protocol-family adoption must be audited as code evolves |
| Residency audit | Records allow/warn/deny decisions in the audit chain. It exists to show attempted violations as well as successful operations. | Compliance evidence. | Local evidence cannot prove the cloud operator's physical location claim |

## Fleet management

| Feature | Why it exists and behavior | Good for, and why | Maturity / caveat |
|---|---|---|---|
| Device identity | Generates/persists an owner-only endpoint identity. It exists to distinguish managed installations without using the pCloud auth token as fleet identity. | Per-device enrollment and revocation. | Implemented source; controller enrollment policy and identity backup are external |
| Privacy-safe heartbeat | Reports device/version/OS/SLO summaries rather than user content or credentials. It exists for fleet health visibility. | Endpoint inventory and alerting. | Implemented agent payload; operators must assess hostname/device metadata privacy |
| Historically named `MtlsFleetAgent` transport | Uses rustls HTTPS with an explicit controller CA; `with_no_client_auth()` means there is **no TLS client certificate**. Device authentication is instead Ed25519-signed HTTP heartbeat headers, and returned commands are signed. | Standalone controller/agent interoperability experiments where server trust and application-level device identity are both explicit. | `mtls` feature/type name is retained, but this is not classic mutual TLS; crate is not wired into pcloudd and no production controller ships here |
| Signed/bounded commands | Models a narrow command set and verification rather than arbitrary shell execution. It exists to limit controller compromise blast radius. | Safe remote pause/config-like actions. | Source types/agent exist; every accepted command and replay policy needs controller interop proof |
| Null agent | Does nothing when fleet is absent. It exists to keep personal installations unmanaged by default. | Single-user deployments. | Implemented default |
| Reference/live tests | In-process/reference and optional live heartbeat prove framing/transport behavior. | Integration work. | A test server is not a supported management product |

## High availability, drain, and disaster recovery

| Feature | Why it exists and behavior | Good for, and why | Maturity / caveat |
|---|---|---|---|
| Active/passive HA lease | Uses a durable lease so only one daemon instance owns side effects. It exists to prevent two services from syncing/mounting the same state concurrently. | Managed server/NAS failover. | `pcloud-daemon::ha_lease` plus config/tests; shared-storage and fencing behavior need deployment qualification |
| Graceful drain | Stops admitting/starting work, waits for in-flight state, and reports drain status. It exists for upgrade/handoff without corrupting transfers. | Rolling upgrade and service maintenance. | Implemented daemon/CLI path; external supervisor timeout policy matters |
| Config handoff/reload | Reloads safe configuration and supports controlled daemon replacement. It exists to change operations without abrupt kill. | Fleet and packaging upgrades. | SIGHUP/native signal differences and rollback need per-platform tests |
| Snapshot/manifest | Captures state/database/archive and optional encryption. It exists for point-in-time disaster recovery independent of sync deletions. | Off-site backups and migration. | Implemented pipeline; actual restore drills and key recovery are release evidence |
| Restore/DR drill assets | Scripts/runbooks exercise recovery. They exist because an unread restored backup is not a backup. | Release and operator qualification. | Verification support, not an always-on runtime feature |
| Audit-chain verification | Scans persisted audit links/ranges on demand or schedule. It exists to detect historical tampering before trusting reports/snapshots. | Compliance and incident response. | Implemented verifier; detection does not prevent deletion of all local copies |
| Integrity sweeper | Periodically finds data/metadata divergence with power/schedule policy. | Latent-corruption detection. | Implemented service; remediation and scale need deployment policy |

## Plugin and extension system

```text
signed manifest
   │ verify key + requested capabilities
   ▼
plugin registry / ExtensionPolicy
   │ grant subset + audit
   ▼
plugin host message bus ── backend (no-op or Wasmtime)
   │ every operation checked again
   ▼
typed, secret-free host operation / response
```

| Feature | Why it exists | Good for, and why | Maturity / caveat |
|---|---|---|---|
| Manifest/signature | Gives a plugin stable ID/version/name/capabilities and optional Ed25519 provenance. | Reviewable deployment and trust roots. Canonical signing bytes avoid serializer ambiguity. | `pcloud-plugin-api`; trusted key distribution remains operator policy |
| Capability model | Separates observe status, sync control, Crypto-state query, and network egress. It exists to avoid handing extensions full RuntimeShell access. | Least-privilege plugins. Each typed operation maps to exactly one required capability. | Implemented API contract; per-resource policy is still host responsibility |
| Registry/lifecycle | Registers, initializes, dispatches, catches panics, and audits operations. It exists to contain extension failure and keep lifecycle explicit. | Built-in/in-process extensions. | In-process API explicitly is not a sandbox |
| Secret-free bus | Exposes coarse status, health, link summaries, ticks, integrity events, scan prefixes, and requests—not auth tokens or Crypto keys. | Safe extension workflows. | The host must continue redacting future operation variants |
| Plugin host | Adds a backend abstraction, resource/capability checks, and host-call routing. It exists to run non-linked extensions behind a message boundary. | Standalone sandbox experiments. | `pcloud-plugin-host` exists, but pcloudd has no host dependency or plugin owner; not wired |
| Wasmtime backend | Runs WebAssembly with fuel/memory/resource constraints in a separate dependency-heavy crate. | Standalone isolation experiments stronger than linked Rust plugins. | Experimental and not wired into daemon startup; WASI/host-call/denial-of-service qualification remains external |
| Auto-heal | Observes integrity events, requests quarantine, and escalates repeated corruption to sync pause. | Testing a managed repair workflow. | Plugin logic exists; without a daemon host it observes/remediates nothing in the default product |
| DLP | Performs bounded pre-upload audit/deny scanning when explicitly hosted. | Rule testing and future endpoint exfiltration controls. | Implemented logic but no default daemon host/integration; heuristic caveats above |
| Public-link expiry | Emits rate-limited desktop notifications and persists `0600` state. | Personal link hygiene. | Advisory only; headless notification support varies |
| Backup schedule | Parses cron/natural language and emits time-triggered backup operations. | User-level scheduling. | Plugin logic exists; missed-run/timezone/daemon host integration must be verified |

## Observability and enterprise control plane helpers

| Feature | Why it exists | Good for, and why | Entrypoint / maturity |
|---|---|---|---|
| Structured/JSON logging | Produces stable fields and optional JSON instead of scraping prose. | SIEM/log pipelines. Feature-gated format keeps default lean. | observability + daemon `json-logs`; implemented |
| Prometheus metrics | Exposes canonical counters/gauges/histograms and a small scraper endpoint. | Dashboards, alerts, capacity planning, SLOs. | `metrics` feature; bind/access policy required |
| W3C traceparent | Parses/creates child trace context across request boundaries. | Correlating CLI/IPC/daemon/external calls. Strict parser rejects malformed context. | config traceparent + IPC envelope; implemented internal |
| OTLP tracing | Exports spans through OpenTelemetry when compiled/configured. | Distributed enterprise tracing. | Feature-gated and optional; collector/TLS/redaction/performance qualification required |
| Per-category rate limits | Controls daemon/API operation rates. It exists to protect accounts/services and separate cheap status from expensive mutations. | Multi-client/fleet workloads. | config + daemon limiter/resilience; implemented internal |
| Circuit breaker/retry budget | Stops hammering a failing service and bounds retries across requests. | KMS, pCloud, IdP, and fleet dependencies. | resilience crate; implemented primitives, correct idempotency classification remains call-site critical |

The generated [API capability catalog](../generated/features/api-capabilities.md)
contains every share, business, public-link, Crypto, and account-level method.
The [package families](../generated/features/package-families.md) and [source
unit catalog](../generated/features/source-units.md) cover every enterprise and
plugin helper, including those not wired into the default daemon.
