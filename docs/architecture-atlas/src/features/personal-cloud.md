# Personal cloud and account features

The personal-cloud path is the base product: one OS user, one authenticated
pCloud account, one owner-only daemon, and any number of short-lived local
clients. It is deliberately useful without enterprise configuration.

```text
person / script
   │
   ├── pcloudc / pcloud-sdk / local web
   │                  │ owner-authenticated IPC
   ▼                  ▼
login and account → pcloudd → RemoteFs → typed TLS protocol → pCloud
                         │
                         └── store, token vault, journals, cache
```

## Authentication and session lifecycle

| Feature | What and why it exists | Good for, and why | Entrypoint / maturity |
|---|---|---|---|
| Password login | Begins a typed login flow and submits the password as `SecretString`. It exists because credentials must cross the local client boundary without becoming ordinary printable strings. | Interactive first login and account recovery. The state machine separates username/password collection from server challenges and never emits secrets in events. | `pcloud-auth`, `pcloud-backends::auth_backend`, `pcloud-proto::auth_api`; implemented internal path |
| Auth-token login | Restores an existing pCloud session without replaying the password. It exists for daemon restart and unattended operation. | Headless systems and daily startup. Tokens remain secret-wrapped and persistence is opt-in rather than automatic. | `SubmitAuthToken`, auth backend, vault; implemented |
| Two-factor challenges | Supports TFA code submission, recovery codes, resend by SMS, and notification challenge. It exists because a password-only state machine cannot represent real pCloud account policy. | TFA-enabled personal accounts. Challenge type and next action are typed, so callers do not infer security state from error strings. | `pcloud-auth::{orchestrator,state}`, CLI login flow; implemented, live proof requires a dedicated TFA fixture |
| User information | Fetches the authenticated user's ID/profile and exposes authentication status. It exists so clients can display account identity and bind state to the correct account. | Status screens, scripts, diagnostics, and multi-account checks. Typed user IDs reduce accidental cross-account reuse. | `userinfo`, `GetUserInfo`; implemented |
| Proactive refresh | Tracks token expiry and refreshes before the session becomes unusable. It exists to keep long syncs and mounts alive without teaching every consumer about refresh timing. | Long-running daemon, sync, mount, backup, and fleet work. One refresh coordinator prevents concurrent refresh storms. | `pcloud-auth::refresh`, `pcloud-session::refresh_loop`, daemon session integration; internal implemented |
| Idle logout and expiry | Tracks last use and lifecycle deadlines. It exists to limit the lifetime of abandoned authenticated state. | Shared workstations and conservative credential policy. A typed lifecycle makes the decision observable and testable. | `pcloud-auth::lifecycle`, `pcloud-session`; internal implemented |
| Explicit logout | Invalidates/drops the active session and removes eligible persisted state. It exists so “stop using this account” has deterministic semantics. | Account switching, incident response, and clean shutdown. Central daemon ownership ensures clients cannot leave a hidden secondary session. | `pcloudc logout`, daemon dispatch; implemented |
| Durable token vault | Optionally saves auth tokens in an OS vault or owner-only encrypted/restricted file. It exists to balance unattended startup with the rule that tokens are not persisted by default. | Desktops, servers, and NAS appliances that must restart unattended. Platform vaults, `0600` enforcement, DPAPI/Keychain/Secret Service, and explicit `authsave` reduce exposure. | `pcloud-daemon::vault`, `pcloud-config::auth`; implemented with platform-specific qualification |
| Session status | Reports expiry, last use, refresh state, and authentication state. It exists so operators can diagnose session problems without inspecting credentials. | Health checks and automation. The payload contains metadata, never the token/password. | `SessionStatus`; implemented |

## Account lifecycle and preferences

| Feature | Rationale and behavior | Good for, and why | Entrypoint / maturity |
|---|---|---|---|
| Account registration | Builds the pCloud registration request through a typed protocol method. It exists for standalone onboarding rather than requiring an external browser workflow. | New disposable/test or user accounts where pCloud permits API registration. Inputs are validated and secrets are wrapped. | account protocol/backend/CLI/embedded SDK; implemented API path, live behavior service-dependent |
| Email verification | Sends normal or restricted verification requests. It exists because unverified accounts cannot reliably use sharing and other lifecycle operations. | Completing onboarding and diagnosing account restrictions. Separate methods preserve the server's distinct flows. | `verify_email`, `verify_email_restricted`; implemented |
| Lost-password flow | Triggers pCloud's password recovery channel. It exists because a client cannot and must not recover the old password itself. | Locked-out users. The operation delegates recovery to the account's verified channel and never stores a replacement credential. | account backend/protocol; implemented, destructive/email side effect gated in E2E |
| Password change | Performs the authenticated account-password change path and supports supervised rollback testing. It exists for routine rotation and incident response. | Security operations. Old/new passwords stay secret-wrapped; destructive live tests use recovery markers because interruption is dangerous. | account backend, CLI/embedded SDK; implemented, supervised live qualification required |
| Language | Gets/sets account language where the pCloud API supports it. It exists to preserve account-level behavior and legacy parity. | Localized account experiences and migration. The setting is a typed account operation rather than a local UI-only guess. | account/settings protocol; implemented |
| API server and region | Lists/selects API servers and binds TLS host/SNI. It exists because EU and US accounts may use different endpoints and residency policy must know the chosen region. | Correct regional routing, migration, and residency enforcement. Configuration validates TLS mode/host and the residency evaluator can refuse disallowed regions. | `pcloud-config::api`, account API, residency backend; implemented with deployment policy |
| Promotions | Retrieves promotion/account offers exposed by pCloud. It exists for API parity, not core drive correctness. | UI or support tools that surface server promotions. It remains a narrow read-only account call. | `getpromo`; implemented |
| Typed settings and values | Reads/writes bool, integer, string, and binary-compatible settings through typed repositories. It exists to replace global C key/value state without losing migration compatibility. | Preferences, feature toggles, sync policy, and migration. SQLite transactions and type-specific helpers prevent silent value reinterpretation. | `pcloud-store::repositories::{settings,values,preferences}`; internal implemented |
| Configuration profiles | Loads versioned TOML/JSON-like profile data, environment overrides, paths, resource limits, and feature policy. It exists so behavior is reproducible instead of controlled by scattered globals. | Desktop, server, tests, packaging, and multiple deployment profiles. Schema/migration/permission and cross-field validation fail early. | `pcloud-config`; internal stable |

## Notifications and status

| Feature | Rationale and behavior | Good for, and why | Entrypoint / maturity |
|---|---|---|---|
| List notifications | Fetches pCloud notifications as typed items. It exists to expose sharing/account events without scraping UI state. | User inboxes, CLI status, plugins, and automation. IDs and read status are preserved from the service. | notifications protocol/backend, `pcloudc notifications list`; implemented |
| Mark notifications read | Advances read state through the dedicated pCloud method. It exists so clients can complete the notification lifecycle. | Inbox UIs and scripts. An explicit `upto_id` makes bulk acknowledgment deterministic. | `MarkNotificationsRead`; implemented |
| Daemon status and health | Reports runtime state, build information, auth/sync condition, and cheap liveness. It exists so supervisors do not infer health from “process exists.” | Desktop launchers, service managers, Docker/NAS health checks, and `doctor`. Payloads are small, typed, and secret-free. | `GetStatus`, `GetHealth`, health server; implemented |
| Pending work | Lists in-flight transfers/sync work. It exists to make shutdown, upgrades, and user expectations observable. | Progress views, drain decisions, and support. State comes from the authoritative runtime rather than filesystem heuristics. | `GetPending`; implemented |
| SLO report | Evaluates canonical service-level objectives such as latency/error behavior. It exists to turn internal metrics into an operator-facing contract. | Fleet/enterprise health and release soak checks. Canonical definitions prevent each dashboard from inventing thresholds. | `pcloud-observability::slo`, `pcloudc slo`; implemented instrumentation, production thresholds need operator validation |

## Remote namespace and everyday file operations

All drive-like behavior converges on [`RemoteFs`](../remote-fs.md). It accepts
paths at the edge, resolves live metadata, then uses typed file/folder IDs.
This is why a cold or empty metadata cache does not make remote files vanish.

| Feature | Rationale and behavior | Good for, and why | Entrypoint / maturity |
|---|---|---|---|
| Live path resolution | Walks each absolute path component through live folder listings and returns typed metadata. It exists to prevent cache-only false negatives and file/folder ID confusion. | CLI, SDK, sync, mount, and gateways. Zero/multiple matches become `NotFound`/`Ambiguous` instead of guesses. | `pcloud_backends::remote_fs::resolve`; canonical implemented path |
| Stat/metadata | Returns ID, kind, parent, size, timestamps, ownership, share, encryption, and permissions where supplied. It exists as the common information model for all consumers. | File browsers, scripts, mount getattr, and decisions before mutation. SDK-owned types shield external callers from IPC evolution. | `RemoteFs::stat`, SDK `RemoteDrive::stat`; implemented |
| Folder listing | Returns authoritative folder metadata and immediate children in server order. It exists for browsing and path traversal. | File explorers, recursive tools, mounts, and sync scanning. It is live and kind-aware rather than a raw cached row dump. | `RemoteFs::list`; implemented |
| Create directory | Creates a remote folder after resolving the live parent. It exists for ordinary drive management and as a primitive for copy/sync. | CLI/SDK `mkdir`, recursive copy, backups, and mount mkdir. ID-first mutation avoids path races after resolution. | folder backend/RemoteFs/SDK; implemented |
| Delete | Deletes a resolved file or folder by ID and treats confirmed absence idempotently. It exists so retries and cleanup can distinguish “already gone” from transport failure. | Scripts, E2E cleanup, sync reconciliation, and mount unlink/rmdir. Typed `DeleteOutcome` supports safe idempotency. | `RemoteFs::delete`; implemented |
| Move/rename | Resolves source/destination and performs the correct file/folder ID operation. It exists to keep rename semantics consistent across every interface. | Organization, sync rename propagation, and mounted-drive rename. Live resolution and kind-specific methods avoid stale-cache moves. | `move_path`, folder protocol; implemented |
| Recursive copy | Copies files/folders, creating destination structure and reporting file/folder/byte totals. It exists because server APIs and user paths need one safe recursive policy. | Drive duplication, migration, and SDK automation. Self/descendant copy is rejected, and byte movement reuses canonical transfer handling. | `copy_path`, SDK `RemoteCopyResult`; implemented |
| Bounded range reads | Reads a file slice with size/EOF metadata and caps one allocation at 16 MiB. It exists for mount/page consumers that need random access without loading whole files. | Media readers, mounted files, WebDAV ranges, and custom SDK loops. Explicit bounds resist memory exhaustion. | `read_range[_by_id]`; implemented |
| Checksums and verification | Calculates/compares local and remote integrity metadata where available. It exists because successful I/O alone does not prove correct bytes. | Migration, post-transfer validation, `pcloudc verify`, integrity sweeps. Strong hashes and explicit mismatch reports make corruption actionable. | CLI verify, transfer backend, integrity sweeper; implemented, remote hash availability varies |
| Local scan | Walks configured local roots and feeds typed scan output into reconciliation. It exists to discover local changes after downtime or watcher loss. | Initial sync, repair, and manual rescan. It complements event watching rather than trusting event delivery as complete history. | engine local scan, `run_localscan`; implemented |

## Public access and lightweight organization

| Feature | Rationale and behavior | Good for, and why | Entrypoint / maturity |
|---|---|---|---|
| File/folder public links | Creates, lists, shows, and deletes public links for selected content. It exists to share without provisioning a pCloud account. | Read-only external distribution. Typed IDs and link summaries make exposure inventory and revocation possible. | public-link protocol/backend/CLI; implemented |
| Tree/selection links | Builds a link from an explicit mixed set of files/folders or resolved paths. It exists when a whole folder is too broad. | Curated deliveries and temporary collections. The path resolver validates every item and chooses an explicit root rather than inferring from cache. | tree public-link helpers; implemented |
| Upload links | Creates upload-only endpoints and updates upload policy. It exists to receive files from people who should not browse the destination. | Intake portals and external submissions. Upload permission is separate from read access and can be deleted independently. | public-links backend/CLI; implemented |
| Link expiry/password | Sets or clears expiry and password controls. It exists to reduce indefinite unauthenticated exposure. | Time-bounded deliveries and low-friction access control. Secret passwords use redacted wire types; expiry is explicit server state. | `change-link-expire/password`; implemented |
| Link access list | Lists/adds/removes named recipients where supported. It exists for narrower access than a fully public URL. | Controlled external collaboration. Recipient and receiver IDs make revocation auditable. | public-link access methods; implemented API path |
| Link traffic/branding/options | Preserves extended public-link settings exposed by pCloud. It exists for parity and operator visibility beyond basic URL creation. | Campaign/support workflows and business accounts. Options remain typed instead of being packed into opaque query strings. | generated [API capability catalog](../generated/features/api-capabilities.md); implemented rows, account entitlements vary |
| Public bookmarks | Lists/changes/removes legacy public bookmarks. It exists for migration and compatibility rather than the canonical filesystem. | Users importing old client state. The feature stays isolated from RemoteFs so bookmark metadata cannot become drive truth. | bookmark IPC/CLI methods; implemented compatibility surface |
| Expiry notification plugin | Watches link expiry and rate-limits desktop warnings. It exists because server expiry can otherwise surprise the owner. | Desktop personal accounts. Persistent `0600` state prevents notification storms; it advises but never silently renews/revokes. | `pcloud-plugin-publink-expiry`; optional experimental plugin |

The generated [API capability catalog](../generated/features/api-capabilities.md)
lists every individual account, setting, filesystem, notification, public-link,
bookmark, CLI, and SDK operation, including the deliberate rejection of legacy
global callbacks and self-update hooks.

