# CLI, SDK, web, protocols, and automation

pcloud-rs exposes one remote behavior through several interfaces. The daemon
is the authority; interfaces should translate intent, not implement a private
cloud client.

## Reachability map

| User intent | CLI | Stable `pcloud-sdk` | Embedded SDK | Local IPC | Owner behind daemon |
|---|---|---|---|---|---|
| Health/status/pending/SLO | `health`, `status`, `pending`, `slo` | Not currently in focused RemoteDrive | Yes | health/status/pending/SLO methods | daemon runtime + observability |
| Login/TFA/logout/session | `login`, TFA channel/code/recovery, `logout`, `authsave`, session status | Daemon must already be authenticated | Broad auth helpers | secret-bearing auth requests/methods | auth/session backends and vault |
| Stat/list/mkdir/delete | filesystem commands | Yes | Yes | RemoteFs requests | canonical RemoteFs/folder backend |
| Move/recursive copy | filesystem commands | Yes | Yes | RemoteFs move/copy | RemoteFs + folder/transfer backends |
| Bounded read/upload/download | download/upload/filesystem commands | Yes | Yes, including more raw helpers | RemoteFs range/stream/durable requests | RemoteFs + transfer backend/journals |
| Upload-session control | upload-session command family | Focused high-level surface only where stable | Broad `UploadSession` facade | begin/chunk/status/commit/abort and registry controls | transfer/upload session backend |
| Sync roots/excludes/pause | `sync`, `pause`, `resume` | No | Helpers/raw dispatch | sync-root and control methods | sync backend/engine/store |
| Mount/unmount | `mount`, `unmount` | No | Mount helpers | mount requests | daemon mount runtime/pcloud-fs |
| Public/upload/tree links | `publink` family | File/folder share-oriented subset where exposed | Broad link helpers | typed public-link requests | public-link backend/protocol |
| Named folder share | `shares` family | Yes | Yes | share requests | shares backend/protocol |
| Business/team share | account/team command family | No | Broad/raw | business/team requests | shares backend/protocol |
| Crypto lifecycle | `crypto setup/start/status/lock/change-password` | No | Broad Crypto example/API | redacted Crypto requests | daemon crypto backend/CryptoShell |
| Backup/device/snapshots | `backup` and snapshot actions | No | Backup helpers | backup/snapshot requests | backup backend/snapshot service |
| Account utilities | account commands | No | Broad helpers | account requests | account backend/protocol |
| Audit/integrity/doctor | `audit`, `verify`, `doctor` | No | Raw/internal where needed | verifier/status requests | store/daemon/fs/CLI diagnostics |

This matrix explains intended surface ownership, not exact method signatures.
The generated [current Rust surface catalog](../generated/features/current-surfaces.md)
enumerates every live CLI `Command`, IPC `Method`, IPC `Request`, direct
stable/embedded SDK construction, daemon runtime arm, and Cargo binary. The
separate [API capability catalog](../generated/features/api-capabilities.md)
covers legacy C-parity decisions, and the generated crate pages give all
declarations.

## Command-line client (`pcloudc`)

| Feature | Why it exists and what it is good for | Why it is effective | Entry / maturity |
|---|---|---|---|
| Typed command enum | Gives every subcommand/alias one parsed variant and one lowering point to IPC. | Prevents parsing, secret collection, and wire mapping from drifting across handlers. | `pcloud-cli::commands`; evolving product surface |
| Human help and aliases | Preserves discoverable modern commands and selected legacy shortcuts. | Migration users keep familiar verbs while aliases reach the same canonical request. | CLI app/parser; implemented |
| Global flags | Centralizes config path, JSON, selectors, and shared behavior. | Scripts get uniform options instead of command-specific surprises. | `globals`; implemented |
| JSON envelope | Returns a stable success/error shape with command, status, message, exit code, and details. | Automation can parse output without scraping prose. | `json_output`; implemented |
| Field selector | Extracts dotted native fields from structured output. | Shell scripts can ask for one value without `jq`; syntax validation rejects ambiguous selectors. | `field_selector`; implemented |
| Stable exit codes | Maps usage, auth, network, conflict, locked, unavailable, and other families consistently. | Supervisors and scripts can distinguish retry/user-action failures. | `exit_code`; implemented enterprise discipline |
| Secret prompts | Reads passwords/tokens/Crypto secrets without echo and stores them in wrappers separate from ordinary args. | Interactive login/unlock/change flows. | `prompt`; implemented; terminal/TTY behavior needs platform qualification |
| Progress | Wraps long-running operation feedback without affecting transfer authority. | Human uploads/downloads/verification. | `progress`; implemented presentation helper |
| Shell completion | Generates command completions. | Discoverability and fewer scripting mistakes. | `completion`; implemented |
| Internationalization | Provides a small in-process message catalog/runtime. | Localized CLI messages without pulling localization into core crates. | `i18n`; internal T1.5, translation coverage evolves |
| CLI config | Loads user-facing CLI preferences separately from daemon state. | Per-client output/endpoint behavior. | `config`; implemented |
| Doctor | Checks daemon reachability, config/paths, platform prerequisites, and common faults. | First-response troubleshooting. It reports causes without reading secret values. | `doctor`; implemented |
| Migration from C | Imports selected legacy config/state into the Rust layout. | Controlled transition from the C client. Versioned parsing and dry/specific behavior are safer than reusing legacy files in place. | `migrate`; implemented helper, fixture-tested |
| Verify | Walks synced data and reports SHA-256/integrity mismatch. | Migration and corruption diagnosis. It is read-only and separates detection from repair. | `verify`; implemented |
| Start/drain/shutdown | Starts a per-user daemon, requests graceful drain, or orderly shutdown. | Desktop/service lifecycle and upgrades. The daemon remains authoritative for in-flight work and cleanup. | CLI main + daemon control methods; implemented |

## Stable public Rust SDK (`pcloud-sdk`)

| Feature | Rationale and use | Why it is effective | Limit/status |
|---|---|---|---|
| Blocking `Client` | Connects to the owner-authenticated daemon endpoint without constructing a remote runtime. | Familiar synchronous API, no required async runtime, and each call opens its own local connection. | Stable source contract; daemon must be running/authenticated; registry publication is separately unqualified |
| SDK-owned types | Defines `RemoteEntryId`, metadata/list/read/copy/upload/download/share receipts and options. | External SemVer is insulated from internal IPC/backend model changes. | Non-exhaustive types permit compatible growth |
| `RemoteDrive` facade | Groups filesystem-focused operations and maps typed responses/errors. | Every call reaches canonical RemoteFs, inheriting policy, live resolution, durability, and platform-neutral behavior. | Intentionally narrower than embedded SDK |
| Path and ID operations | Supports human paths while preserving typed file/folder identity in results. | Programs can browse naturally but avoid numeric ID kind confusion. | Live remote resolution may cost calls; cache is not authority |
| Bounded reads and durable transfers | Exposes memory-bounded random reads and crash-safe local-file upload/download receipts. | Applications get integrity/resume without owning journals. | Generic async streaming and every account API are not promised by the stable surface |
| Sharing | Uses typed recipients/permissions/options through RemoteFs. | Common folder sharing does not require raw permission bit handling. | Business/team administration remains outside focused SDK |
| Error surface | Separates transport, daemon status, protocol/shape, validation, and operation errors into SDK-owned variants. | Callers can handle failures without matching daemon prose. | Exact variants are non-exhaustive for evolution |
| Threading model | Clones share immutable endpoint/sender configuration, not a mutable response connection. | Simple multi-threaded callers avoid hidden connection state. | Operation-level daemon serialization/concurrency still applies |

## Embedded first-party SDK (`pcloud-embedded-sdk`)

| Feature | Why it exists / good for | Why it is effective | Caveat |
|---|---|---|---|
| In-process runtime facade | Preserves broad C-parity and internal test access not appropriate for a small public crate. | First-party integration, examples, and migration can exercise backends directly while reusing composition. | Unpublished and evolving; not a third-party SemVer promise |
| Broad auth/account surface | Exposes login/session/TFA/notifications/account utilities. | Tests and controlled embeddings can run a full account lifecycle. | Embedding owns more secret/runtime responsibility than public SDK use |
| Raw/broad protocol helpers | Covers public links, backup, mount, stat/list, local scan, uploads/downloads, and raw dispatch. | Useful for parity proof and future facade design. | Callers can reach internal behavior that may change |
| Upload-session handle | Provides chunked session lifecycle and benchmark/example. | Large-upload experiments and reachability proof. | Public route/maturity must be evaluated separately |
| Examples | Login/list, upload/download, public links, tree links, and Crypto lifecycle. | Executable call sequences reveal prerequisites and ownership. | Examples do not prove live service behavior without credentials |

## Local IPC (`pcloud-ipc`)

| Feature | Why it exists | Good for, and why | Platform/security detail |
|---|---|---|---|
| Typed Request/Response | Gives local clients an exhaustive method/payload schema. | CLI, SDK, web, WebDAV, tests, and future client languages. Versioned typed envelopes reject unknown/malformed states. | Business policy stays in daemon, not codec |
| Length-delimited framing | Defines message boundaries and request-size caps. | Stream sockets/pipes where read boundaries are arbitrary. | Caps mitigate memory exhaustion; decoder property/fuzz tests cover malformed frames |
| Peer authentication | Confirms the caller is the daemon owner's UID/SID. | Keeping another local user from controlling an authenticated daemon. | Linux `SO_PEERCRED`; BSD/macOS `getpeereid`; Solaris-family `getpeerucred`; Windows named-pipe TokenUser SID |
| Endpoint permissions | Creates owner-only socket/runtime directories or named-pipe DACL. | Defense before/alongside accepted-peer checks. | Filesystem/ACL behavior must be native-tested |
| Secret wire wrappers | Redact Debug/serialization diagnostics for intentionally transmitted credentials. | Login/Crypto/link-password requests. | Redaction is not encryption; peer auth and local OS protection are required |
| Path validation | Validates local sync-root paths before privileged use. | Preventing malformed/non-absolute/unsafe root requests. | Remote paths are separately validated by RemoteFs |
| Trace context | Carries W3C traceparent metadata around typed requests. | End-to-end latency debugging. | Sensitive payload fields are excluded from spans |
| Server/client abstraction | Provides portable accept/connect/send loops over native transports. | One CLI/SDK behavior on all targets. | Current accept-loop concurrency/serialization is an architectural choice documented by ADR 0019 |
| Protocol/property/stress tests | Exercises roundtrip, size caps, peer rules, concurrent clients, and cross-platform selectors. | Regression safety at the trust boundary. | Synthetic peer tests do not replace native hostile-user tests |

## Remote pCloud protocol stack (`pcloud-proto`)

Local IPC and remote pCloud protocol are different. The daemon decodes local
IPC, applies its currently composed validation/rate/drain/state rules, then
calls typed pCloud clients over TLS. The standalone Rego, plugin, fleet, IdP,
and multi-account crates are not silently part of this path.

| Protocol family/helper | Why it exists / operations | Strengths and use | Maturity |
|---|---|---|---|
| Auth | Password/token login, TFA code/recovery, resend, user info, logout-like state. | Secret-bearing builders and typed challenge responses isolate auth wire details. | Implemented |
| Account | Register, verify, lost/change password, language, API servers, promotions. | Separate builders preserve side-effect/account semantics. | Implemented; service entitlement/side effects vary |
| Folder | List/stat/create/rename/move/delete/copy and metadata helpers. | File/folder-specific DTOs feed canonical RemoteFs. | Implemented |
| Transfer/upload/download | Signed file links, upload create/write/save/status, chunk/session helpers, thumbnails and download execution. | Control and bulk HTTP paths are separated; integrity/error classification supports resume. | Implemented core |
| Sync/diff | Syncability/root methods and remote change cursors/long polling. | Typed cursors/events support restartable reconciliation. | Implemented |
| Backup/device | Backup root/device create/stop/delete operations. | Dedicated API prevents accidental sync semantics. | Implemented |
| Notifications | List and mark-read methods. | IDs and read cursors remain typed. | Implemented |
| Shares/business/teams | User invitations, active shares, permission changes, contacts, teams, and account administration. | Target-specific DTOs avoid user/team ID mixing. | Implemented API paths; business live proof requires entitled accounts |
| Public links | File/folder/tree/upload link CRUD and access/password/expiry/options. | Link IDs/options are structured and secret passwords redacted. | Implemented |
| Crypto account transport | User-key and password-change protocol families. | Keeps server-side Crypto OTP/blob operations outside primitive code. | Implemented path; complete live OTP/interop qualification external |
| Binary API framer | Encodes typed parameters/commands and parses framed responses. | Bounds and property tests make legacy binary wire handling reviewable. | Internal implemented |
| Response/value parser | Normalizes success/error/result values used by every API family. | One error/result boundary prevents inconsistent interpretation. | Internal implemented |
| TLS | Builds shared rustls configuration with host/SNI/trust. | One security posture across APIs and downloads. | Implemented; not a FIPS claim |
| HTTP download executor | Follows signed download location and checks expected byte behavior. | Bulk data path does not burden the binary control protocol. | Implemented |
| Resilient transport | Wraps protocol requests with retry/circuit/rate policy and metrics. | Transient failure handling without duplicating in each API. | Implemented internal; idempotency classification is critical |
| Async transfer | Supplies typed async envelopes/helpers. | Future/internal async consumers. | Internal support; public SDK remains blocking |
| Parallel download planner | Builds non-overlapping ranges. | High-throughput large files. | Experimental until live performance/integrity qualification |
| Redacted types | Prevents tokens/passwords from appearing in Debug/error prose. | Every secret-bearing method. | Internal safety layer |

## Local web UI (`pcloud-web`)

| Feature | Why it exists / good for | Why it is effective | Maturity/caveat |
|---|---|---|---|
| Axum status/control scaffold | Provides browser-readable local health/status/sync views. | Desktop operators who prefer a browser. | MVP/evolving, not the final application |
| Loopback-first bind | Defaults to localhost and requires deliberate external bind/allowed host. | Reduces accidental LAN exposure. Host validation rejects unexpected Host headers. | Reverse proxies/TLS/auth must be explicitly designed before remote use |
| IPC-backed routes | Delegates state/control to pcloudd. | Browser process never owns pCloud credentials or a second runtime. | Route set is intentionally small |
| Inline templates | Keeps the MVP dependency/deployment simple. | Diagnostics and early usability. | Not a full component framework, accessibility/localization maturity varies |

## Experimental WebDAV gateway (`pcloud-webdav`)

| Feature | Why it exists / good for | Why it is effective | Explicit limit |
|---|---|---|---|
| Minimal HTTP/1.1 codec | Reads bounded local requests and writes responses without a large web stack. | Controlled local compatibility experiments. | Not a general hardened internet HTTP server |
| PROPFIND/multistatus | Implements the central WebDAV listing shape. | Applications that browse WebDAV resources. | Only a subset; no RFC 4918 compliance claim |
| Verb dispatcher | Maps supported methods to a backend trait. | Testable separation of parsing and file behavior. | Unsupported verbs return explicit responses |
| IPC RemoteFs backend | Routes implemented file operations through daemon IPC. | Avoids a second pCloud namespace/credential path. | Listener/bootstrap/packaging remain experimental and unshipped |

## Legacy compatibility (`pcloud-compat`)

| Feature | Why it exists | Good for | Limit |
|---|---|---|---|
| `rpc_message_t` codec | Reads/writes exact legacy C control frames. | Migration, interop tests, and forensic tooling. | Not canonical IPC; modern clients use pcloud-ipc |
| Folder-list ABI | Mirrors the old shared-memory folder-list structure. | Legacy consumers that cannot migrate immediately. | ABI/platform-specific and isolated |
| SysV SHM producer | Publishes the legacy payload when `legacy-shm` is enabled. | Controlled Unix migration/debug. | Non-default; not portable to Windows and not wired into ordinary daemon |
| SHM peek binary | Inspects the payload across processes. | Integration tests and diagnostics. | Debug helper, not end-user drive functionality |

## Inert LAN P2P scaffold (`pcloud-p2p`)

P2P policy/lifecycle types reserve an API shape for a possible future LAN
accelerator. Current truth is deliberately smaller than stale crate prose:
`DiscoveryRuntime::start` opens no network or mDNS socket, advertises nothing,
and `peers()` always returns an empty vector. There is no peer inventory,
transfer planner, authentication protocol, or peer byte transfer, and the
crate is not wired into `pcloudd`. The cloud and RemoteFs remain authoritative;
this is design scaffolding only and provides no LAN acceleration today.
