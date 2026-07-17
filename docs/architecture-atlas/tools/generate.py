#!/usr/bin/env python3
"""Generate the exhaustive pcloud-rs architecture atlas catalogs.

Generated files are intentionally source-derived. Do not hand-edit anything
under src/generated/.
"""

from __future__ import annotations

import datetime as dt
import csv
import json
import os
import re
import subprocess
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

ATLAS = Path(__file__).resolve().parents[1]
ROOT = ATLAS.parents[1]
SRC = ATLAS / "src"
GENERATED = SRC / "generated"
GITHUB = "https://github.com/ezechiel203/pcloud-rs/blob/main"

FUNCTION_ITEM = re.compile(
    r"^\s*(?P<visibility>pub(?:\([^)]*\))?\s+)?"
    r"(?:(?:async|unsafe|const|default)\s+)*"
    r"(?:extern\s+\"[^\"]+\"\s+)?"
    r"(?P<kind>fn)\s+"
    r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)"
)
OTHER_ITEM = re.compile(
    r"^\s*(?P<visibility>pub(?:\([^)]*\))?\s+)?"
    r"(?P<kind>struct|enum|trait|type|const|static|mod)\s+"
    r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)"
)

STABLE = {"pcloud-sdk"}
INTERNAL = {
    "pcloud-auth",
    "pcloud-backends",
    "pcloud-crypto",
    "pcloud-engine",
    "pcloud-error",
    "pcloud-ipc",
    "pcloud-model",
    "pcloud-observability",
    "pcloud-proto",
    "pcloud-resilience",
    "pcloud-secret",
    "pcloud-store",
}
EVOLVING = {
    "pcloud-cache",
    "pcloud-cli",
    "pcloud-config",
    "pcloud-daemon",
    "pcloud-embedded-sdk",
    "pcloud-fs",
    "pcloud-session",
    "pcloud-web",
}
VERIFY = {"pcloud-chaos", "pcloud-live-e2e", "pcloud-mockserver"}
TOOLING = {"xtask"}

REQUIRED_PLATFORMS = (
    "Linux",
    "macOS",
    "Windows",
    "FreeBSD",
    "NetBSD",
    "OpenBSD",
    "DragonFly BSD",
    "illumos/OmniOS",
    "Oracle Solaris",
    "Synology DSM",
    "QNAP QTS/QuTS hero",
    "ASUSTOR ADM",
)

VERIFICATION_CATEGORIES = (
    "unit tests",
    "integration tests",
    "mock server",
    "live E2E",
    "chaos",
    "fuzz",
    "benchmarks",
    "mutation testing",
    "coverage",
    "disaster recovery",
    "reproducibility",
)

# A source inventory says *what exists*.  These profiles add the architectural
# explanation the feature encyclopedia needs: why the package exists, where it
# is useful, and which design property makes it valuable.  The generated crate
# and source-unit catalogs use this map, so every package and internal helper is
# covered even when it has no standalone README.
CRATE_PROFILES: dict[str, tuple[str, str, str]] = {
    "pcloud-auth": (
        "Keep login, TFA, refresh, logout, and session transitions in one explicit state machine instead of scattering credential rules through callers.",
        "Interactive and unattended authentication flows, token refresh, idle expiry, and safe recovery from authentication challenges.",
        "Typed commands, states, and secret-free events make invalid transitions visible and independently testable.",
    ),
    "pcloud-backends": (
        "Turn low-level pCloud protocol methods into coherent filesystem, account, sharing, backup, transfer, residency, and snapshot behaviors.",
        "Daemon business logic and the canonical RemoteFs service used by CLI, SDK, sync, mount, and web adapters.",
        "ID-first operations, narrow subsystem backends, durable transfer state, and a single remote namespace prevent competing interpretations of cloud state.",
    ),
    "pcloud-cache": (
        "Reduce latency and repeated I/O without allowing cached data to become remote truth.",
        "Metadata/page reuse, checksum reuse, bounded staging, eviction, and optional encrypted local cache blobs.",
        "Bounded LRU-style structures and explicit staging/cipher boundaries make memory use and invalidation behavior controllable.",
    ),
    "pcloud-chaos": (
        "Exercise failure modes that ordinary success-path tests cannot prove.",
        "Disk-full, process-kill, slow-peer, clock-jump, and network-blackhole recovery validation.",
        "Scripted fault scenarios have predicted outcomes, allowing crash safety and resilience claims to be falsified repeatably.",
    ),
    "pcloud-cli": (
        "Expose daemon capabilities to humans and automation without duplicating remote or security logic in a short-lived process.",
        "Login, status, file operations, sync, backup, Crypto, account administration, diagnostics, migration, JSON output, and shell completion.",
        "Typed IPC commands, stable exit codes, field selection, redacted prompts, and machine-readable output serve both operators and scripts.",
    ),
    "pcloud-compat": (
        "Isolate the small legacy C-client ABI surfaces that are still useful during migration.",
        "Decoding legacy rpc_message_t frames and, when explicitly enabled, producing the old SysV shared-memory folder-list layout.",
        "Byte-exact codecs live outside the canonical daemon so compatibility cannot silently constrain modern internal design.",
    ),
    "pcloud-config": (
        "Give every process one validated, versioned, secure configuration model across platforms and deployment sizes.",
        "Profiles, paths, API/TLS settings, vault selection, limits, sync/mount policy, observability, HA, residency, KMS, bandwidth, and upgrades.",
        "Schema validation, migrations, permission checks, typed environment overrides, and cross-field invariants reject unsafe combinations early.",
    ),
    "pcloud-crypto": (
        "Provide client-side Crypto-folder confidentiality while supporting both official-client interoperability and a Rust-native hardened format.",
        "Content and filename encryption, key derivation/wrapping, unlock lifecycle, password rotation, crypto sharing, policy, and compatibility KATs.",
        "Domain-separated keys, zeroized secret material, authenticated sectors, nonce budgets, lockout, and explicit backend identity avoid silent format or key misuse.",
    ),
    "pcloud-daemon": (
        "Own long-lived credentials, validated request dispatch, state, background work, and native resources in one per-user composition root.",
        "Running pcloudd as the authoritative local service behind CLI, SDK, web, sync, mount, backup, health, and metrics.",
        "A single RuntimeShell, peer-authenticated IPC, durable stores, controlled shutdown, and narrowly composed backends centralize authority and cleanup.",
    ),
    "pcloud-daemon-win": (
        "Explore Windows Service Control Manager hosting without contaminating the portable per-user daemon.",
        "Experimental Windows service installation and SCM lifecycle integration.",
        "The wrapper is isolated and explicitly unshipped, so Windows-specific service semantics cannot be mistaken for the supported daemon contract.",
    ),
    "pcloud-embedded-sdk": (
        "Retain a broad first-party in-process compatibility API for tests, migration, and tightly controlled embeddings.",
        "Embedding daemon-like behavior, broad pCloud API coverage, upload sessions, Crypto workflows, and compatibility examples.",
        "It composes existing runtime/backends rather than reimplementing protocol semantics, but remains unpublished to avoid freezing internals.",
    ),
    "pcloud-engine": (
        "Separate synchronization decisions from transport and process lifecycle.",
        "Scanning, diff polling, selective sync, conflict policy, planning, scheduling, recovery, stall detection, and upload/download coordination.",
        "Pure planners and typed events make reconciliation deterministic, testable, priority-aware, and recoverable.",
    ),
    "pcloud-error": (
        "Give the workspace one stable failure language instead of ad-hoc strings and platform errno leakage.",
        "Mapping failures consistently across protocol, IPC, CLI, SDK, retry, policy, and operator diagnostics.",
        "Stable codes and structured variants preserve actionable context while keeping secret material out of messages.",
    ),
    "pcloud-fleet": (
        "Let centrally managed deployments report health and receive bounded commands without changing single-user defaults.",
        "Experimental standalone enrollment, device identity, CA-authenticated HTTPS heartbeat, Ed25519 device/command signatures, SLO reporting, and fleet command envelopes.",
        "Null-by-default behavior, owner-only identity files, pinned controller CA trust, and constrained signed commands limit the management trust boundary; the crate is not wired into pcloudd.",
    ),
    "pcloud-fs": (
        "Translate remote-drive semantics into native filesystem behavior while surviving caching, partial writes, crashes, and platform ABI differences.",
        "Linux/BSD FUSE, macOS fuse-t/macFUSE, Windows WinFSP, read caching, staged writeback, journals, orphan cleanup, and filesystem watching.",
        "A portable adapter core plus narrow native shims, stable inode mapping, bounded caches, and write-ahead staging protect correctness across kernels.",
    ),
    "pcloud-idp": (
        "Model enterprise federated-login flows separately from native pCloud authentication.",
        "OIDC Authorization Code with PKCE, discovery/JWKS validation, and future SAML/LDAP or trusted-issuer exchange adapters.",
        "S256-only PKCE, RS256 verification, issuer/audience checks, cached JWKS, and an explicit unimplemented exchange keep incomplete federation honest.",
    ),
    "pcloud-ipc": (
        "Provide one owner-authenticated local protocol between untrusted client processes and the credential-holding daemon.",
        "CLI/SDK/web requests over Unix sockets, Solaris credentials, or Windows named pipes with typed framing and trace propagation.",
        "Peer UID/SID checks, owner-only endpoints, request-size caps, redacted secret fields, and exhaustive typed methods harden the local boundary.",
    ),
    "pcloud-kms": (
        "Allow enterprise operators to wrap data-encryption keys under external KMS or HSM control.",
        "Null/default operation plus optional AWS KMS, HashiCorp Vault Transit, and PKCS#11 provider paths.",
        "A narrow KmsProvider contract, provider-specific feature gates, no silent downgrade, and zeroized plaintext DEKs isolate external key custody.",
    ),
    "pcloud-live-e2e": (
        "Prove selected behavior against the real pCloud service and native environments instead of relying only on mocks.",
        "Gated auth, account, transfer, sharing, backup, Crypto, sync, mount, snapshot, rate-limit, and fleet integration checks.",
        "Ignored-by-default destructive gates, disposable-account conventions, unique scratch objects, and serial execution reduce accidental production impact.",
    ),
    "pcloud-mockserver": (
        "Make protocol and backend integration deterministic without network access or production secrets.",
        "Local pCloud-like HTTP responses, error injection, request assertions, and repeatable integration flows.",
        "An in-process server gives tests realistic I/O boundaries while retaining exact control of timing and responses.",
    ),
    "pcloud-model": (
        "Keep shared domain data independent from transport, storage, UI, and platform code.",
        "Typed IDs, users, auth, files, folders, shares, links, sync state, transfers, conflicts, Crypto, and health payloads.",
        "Small serializable types and newtype IDs prevent cross-layer duplication and accidental identifier mixing.",
    ),
    "pcloud-observability": (
        "Make daemon behavior diagnosable and auditable without leaking credentials or binding the core to one telemetry vendor.",
        "Structured logs, audit events, health/build reports, metrics, SLOs, Prometheus export, poison-safe locks, and optional OTLP traces.",
        "A zero-dependency metrics core and feature-gated exporters keep the default lean while preserving consistent names and redaction.",
    ),
    "pcloud-p2p": (
        "Reserve typed policy and lifecycle seams for possible LAN-assisted transfer while preserving the cloud as authority.",
        "Design exploration and compile-time/API compatibility only: the current runtime opens no mDNS socket, advertises nothing, returns no peers, and transfers no bytes.",
        "The inert scaffold prevents an unfinished peer path from becoming a trusted metadata or data source and is not wired into pcloudd.",
    ),
    "pcloud-plugin-api": (
        "Define a capability-limited extension contract before loading third-party code.",
        "Signed manifests, registry/lifecycle metadata, operation/response messages, capabilities, and audit events.",
        "Ed25519 verification, explicit capability grants, size/version checks, and secret-free messages make extension authority reviewable.",
    ),
    "pcloud-plugin-autoheal": (
        "Detect local integrity drift and escalate corruption through a bounded plugin workflow.",
        "Checksum scans, quarantine requests, retry tracking, and full-sync pause escalation.",
        "A small state machine separates detection from privileged remediation and caps repeated failure behavior.",
    ),
    "pcloud-plugin-backup-schedule": (
        "Add user-controlled backup timing without embedding cron parsing into the daemon core.",
        "Cron and natural-language schedules that emit backup-cycle operations on time ticks.",
        "Pure schedule parsing plus deterministic tick handling makes time behavior independently testable.",
    ),
    "pcloud-plugin-dlp": (
        "Inspect outbound content before upload so obvious secrets can be blocked or reviewed.",
        "Built-in regex and Shannon-entropy scanning with findings and policy decisions.",
        "Bounded scanning, explicit rules, and pre-upload placement provide useful local detection without granting network access.",
    ),
    "pcloud-plugin-host": (
        "Run extensions behind a capability and message boundary instead of exposing daemon memory directly.",
        "Plugin lifecycle, operation dispatch, resource limits, audit hooks, and pluggable sandbox backends.",
        "The dependency-light host core separates policy from the runtime engine and defaults to denying ungranted capabilities.",
    ),
    "pcloud-plugin-publink-expiry": (
        "Warn users before expiring public links become an operational surprise.",
        "Single-user desktop notifications with persistent per-link rate limiting.",
        "A clock/notifier abstraction and atomic owner-only state make behavior testable and restart-safe without auto-mutating links.",
    ),
    "pcloud-plugin-wasmtime": (
        "Provide a concrete WebAssembly sandbox without forcing Wasmtime into the core plugin host dependency graph.",
        "Experimental execution of capability-bounded plugins with fuel and memory limits.",
        "A separate backend crate contains the large runtime dependency and translates only the narrow PluginBackend contract.",
    ),
    "pcloud-policy": (
        "Apply organization rules before sensitive operations rather than relying only on server-side administration.",
        "Default-deny Rego evaluation, policy bundle loading/hot reload, contextual allow/deny decisions, and null single-user policy.",
        "Fail-closed evaluation, owner-only policy files, deterministic inputs, and a null default preserve security and single-user simplicity.",
    ),
    "pcloud-proto": (
        "Represent pCloud wire operations as typed clients and builders rather than stringly-typed calls throughout the product.",
        "Auth, account, folders, transfers, diff, backup, notifications, shares, business/team, public links, Crypto, binary framing, TLS, and downloads.",
        "Typed DTOs, bounded parsers, shared TLS, redacted secrets, resilient wrappers, and synchronous/async transfer helpers isolate protocol risk.",
    ),
    "pcloud-resilience": (
        "Keep transient faults from becoming retry storms, indefinite hangs, or cascading outages.",
        "Retry/backoff, global retry budgets, circuit breakers, rate limits, pacing, metered-network awareness, and optional async timeouts.",
        "Injectable clocks, explicit state machines, token budgets, jitter, and metrics make failure policy observable and testable.",
    ),
    "pcloud-rsync": (
        "Avoid retransmitting unchanged blocks when a large file changes locally.",
        "Rolling weak hashes, strong block signatures, delta planning, and differential-upload strategy inputs.",
        "Rsync-style weak/strong matching finds reusable blocks in one pass while strong hashes protect against weak-hash collisions.",
    ),
    "pcloud-sdk": (
        "Give third-party Rust programs a small stable API without exposing daemon internals or credential handling.",
        "Blocking RemoteDrive operations for listing, metadata, directories, copy/move, upload/download, delete, public links, and sharing through pcloudd.",
        "SDK-owned SemVer types and authenticated local IPC keep the public contract narrow and reuse the daemon's policy, durability, and security.",
    ),
    "pcloud-secret": (
        "Make accidental logging, cloning, serialization, comparison leakage, and residual memory less likely for credentials and key material.",
        "Passwords, tokens, private keys, PINs, and binary secret buffers across all crates.",
        "Redacted Debug, no Serialize, zeroize-on-drop, explicit exposure, and constant-time equality force deliberate secret handling.",
    ),
    "pcloud-session": (
        "Keep daemon session refresh and vault synchronization separate from both protocol mechanics and process dispatch.",
        "Refresh loops, session lifecycle glue, persisted-token synchronization, and authentication expiry handling.",
        "A narrow lifecycle layer coordinates timers and vault actions while reusing the typed auth state machine.",
    ),
    "pcloud-store": (
        "Persist durable local state transactionally across crashes and upgrades.",
        "SQLite schema/migrations, settings, preferences, sync graph, diff cursors, metadata, upload resume, account state, audit chain, and typed values.",
        "Transactions, versioned migrations, busy retries, HMAC-indexed sensitive keys, integrity checks, and repository boundaries protect consistency.",
    ),
    "pcloud-supervisor": (
        "Model multiple isolated accounts without merging credentials, state directories, or daemon authority.",
        "Experimental account registry, account selection, IPC routing metadata, and per-account sub-daemon spawning.",
        "Process and path isolation make account boundaries explicit; the scaffold remains separate until routing and lifecycle are production-wired.",
    ),
    "pcloud-web": (
        "Offer a browser-readable local status/control surface without giving the browser direct access to pCloud credentials.",
        "Loopback-first health, status, sync, and simple UI routes backed by daemon IPC.",
        "Host validation, loopback defaults, limited routes, and IPC delegation keep the MVP surface small and authority in pcloudd.",
    ),
    "pcloud-webdav": (
        "Explore compatibility with applications that understand WebDAV but not the native SDK or CLI.",
        "Experimental local HTTP parsing, PROPFIND/multistatus, method dispatch, and daemon-IPC-backed file operations.",
        "A minimal dependency-light codec and canonical IPC backend avoid creating a second remote implementation; RFC completeness is intentionally not claimed.",
    ),
    "xtask": (
        "Keep CI/CD policy versioned and runnable locally instead of depending on opaque hosted-workflow behavior.",
        "Format, lint, test, coverage, audit, packaging, Docker, native mount, Windows remote, release, and cleanup orchestration.",
        "One Rust entrypoint pins command order, fail/skip policy, toolchain use, and cross-platform evidence so developer and release gates match.",
    ),
}

SUBSYSTEM_PROFILES: dict[str, tuple[str, str, str]] = {
    "init": ("Own startup and shutdown explicitly.", "Embedding and daemon lifecycle.", "Central initialization and cleanup avoid hidden global state."),
    "notifications": ("Expose server-side events.", "User alerts and read-state workflows.", "Typed list/read operations preserve IDs and state."),
    "state": ("Report real runtime state.", "Status pages, automation, and diagnosis.", "Typed snapshots replace global errno and stub values."),
    "auth": ("Authenticate and manage sessions safely.", "Password, token, TFA, recovery, refresh, logout, and user-info flows.", "Secret wrappers and explicit state transitions contain credential risk."),
    "account": ("Cover account lifecycle utilities.", "Registration, verification, password recovery/change, language, promotions, and API-region selection.", "Typed protocol builders validate each distinct account operation."),
    "settings": ("Persist user/runtime preferences.", "Behavioral toggles and typed key/value settings.", "Validated typed storage avoids stringly configuration drift."),
    "sync": ("Reconcile local and remote trees.", "Adding, listing, removing, pausing, and inspecting sync roots.", "Canonical roots, planners, diff cursors, and durable state make convergence explicit."),
    "fs": ("Present and inspect a drive-like namespace.", "Stat, list, path resolution, mount, unmount, and local scan.", "RemoteFs provides one ID-first namespace independent of cache warmth."),
    "transfers": ("Move bytes reliably.", "Uploads, downloads, links, progress, pause/resume, and large-file sessions.", "Signed links, chunk state, integrity checks, and journals support recovery."),
    "backup": ("Model backup/device lifecycle separately from bidirectional sync.", "Creating, stopping, deleting, and snapshotting backup roots/devices.", "Explicit backup state prevents accidental sync semantics."),
    "updates": ("Make update behavior an explicit product decision.", "Version/reporting compatibility and downstream packaging.", "Rejected self-update paths avoid an untrusted second distribution channel."),
    "crypto": ("Protect content and Crypto-folder metadata client-side.", "Setup, unlock, lock, password change, encrypted folders, sectors, and crypto sharing.", "Explicit backends, key wrapping, authentication tags, and zeroization bind data to the intended key/profile."),
    "shares": ("Support person-to-person collaboration.", "Incoming/outgoing requests, accept/decline/cancel/remove, and share permissions.", "Typed request IDs and folder IDs keep each lifecycle transition auditable."),
    "business": ("Reach organization/team sharing operations.", "Business accounts, team shares, and account-scoped administration.", "Separate business methods prevent consumer-share assumptions from leaking into team workflows."),
    "links": ("Publish controlled URL access without account credentials.", "File/folder/tree/upload links, passwords, expiry, traffic, branding, and deletion.", "Explicit link IDs, options, and typed responses make public exposure manageable."),
    "bookmarks": ("Preserve lightweight link metadata where supported.", "Legacy migration and link organization.", "Compatibility is isolated from the canonical file namespace."),
    "cli": ("Map legacy and modern operator verbs onto typed daemon requests.", "Interactive administration and automation.", "Aliases can remain familiar while one IPC command model owns semantics."),
    "sdk": ("Expose programmatic composition.", "First-party embedding and third-party Rust applications.", "Facade types reuse daemon/backends instead of duplicating protocol behavior."),
}

# Current Rust surfaces are broader than the legacy C parity matrix. These
# profiles explain the destination and side effects for every current CLI and
# IPC enum variant in the generated cross-surface catalog.
SURFACE_FAMILY_PROFILES: dict[str, tuple[str, str, str, str, str]] = {
    "control": (
        "daemon runtime and observability",
        "reads process/queue/health state or changes local lifecycle state",
        "Make process health, lifecycle, and operator control explicit instead of inferring them from a PID.",
        "supervision, diagnosis, graceful maintenance, and automation",
        "implemented local product surface",
    ),
    "auth": (
        "pcloud-auth, session, auth backend, and token vault",
        "changes or reports the authenticated session and may call pCloud auth APIs or the local vault",
        "Keep secret submission and session transitions inside one typed state machine.",
        "login, TFA, refresh, persistence opt-in, logout, and session diagnosis",
        "implemented; real-account/TFA paths require live evidence",
    ),
    "crypto": (
        "daemon CryptoShell, crypto backend, pcloud-crypto, and optional KMS",
        "changes crypto lifecycle/key state or processes encrypted metadata/content",
        "Require explicit locked/unlocked/key-profile transitions for client-side encrypted data.",
        "Crypto setup, unlock, compatibility, rotation, key lookup, and safe lock/reset",
        "implemented code; interoperability/provider/native evidence is separate",
    ),
    "sync": (
        "sync backend, pcloud-engine, and pcloud-store",
        "reads or mutates sync roots, plans, conflicts, excludes, cursors, and queued work",
        "Centralize reconciliation so CLI, mount, and background work cannot invent competing sync semantics.",
        "continuous synchronization, selective sync, pause/resume, conflict handling, and recovery",
        "evolving implemented product surface",
    ),
    "links": (
        "public-link backend and typed pCloud public-link protocol",
        "reads or mutates public/upload/tree links and bookmarks on the remote account",
        "Represent public exposure as typed, revocable objects with explicit expiry/access policy.",
        "publishing files/folders, upload drop boxes, link hygiene, and controlled recipients",
        "implemented API paths; live account qualification required",
    ),
    "shares": (
        "shares/business backend and typed pCloud sharing protocol",
        "reads or mutates user, contact, request, team, and business share state",
        "Preserve each collaboration lifecycle and permission transition as an auditable typed operation.",
        "person-to-person sharing, team administration, invitations, and access reviews",
        "implemented API paths; bilateral/business live fixtures required",
    ),
    "notifications": (
        "notifications backend and pCloud notification protocol",
        "reads notification state or advances its remote read cursor",
        "Expose server-side events without coupling notification state to UI code.",
        "alerts, inbox workflows, plugins, and automation",
        "implemented API path",
    ),
    "remote-fs": (
        "canonical RemoteFs and folder/transfer backends",
        "resolves live remote IDs and reads or mutates the pCloud namespace",
        "Give every drive-like consumer one cache-independent, ID-first namespace contract.",
        "stat/list/read/mkdir/copy/move/delete and CLI/SDK drive access",
        "implemented canonical product path",
    ),
    "transfer": (
        "transfer backend, upload session registry/journal, and pCloud transfer protocol",
        "moves bytes or controls durable upload/download session state",
        "Separate byte movement from namespace intent and persist resumable state before acknowledging progress.",
        "large files, signed downloads, range reads, resumable uploads, and progress control",
        "implemented; real-service integrity/resume qualification required",
    ),
    "mount": (
        "daemon mount runtime and pcloud-fs adapters",
        "changes native mount state or inspects local filesystem integration",
        "Adapt the canonical remote namespace to kernels without creating a second source of truth.",
        "desktop drive letters/mountpoints, filesystem applications, cleanup, and diagnostics",
        "platform-specific; native qualification required",
    ),
    "backup": (
        "backup backend, snapshot helpers, store, and transfer path",
        "creates, reports, stops, verifies, restores, or removes backup/device/snapshot state",
        "Keep archival/device lifecycle distinct from bidirectional sync and make recovery evidence executable.",
        "device backup, portable snapshots, retention, verification, and disaster recovery",
        "implemented components; destructive/live/restore drills required",
    ),
    "account": (
        "account backend, configuration/settings store, and typed pCloud account protocol",
        "reads or mutates account/profile/API-region/language/value state",
        "Keep account administration separate from authentication and file operations.",
        "registration, verification, password recovery/change, preferences, region selection, and promotion data",
        "implemented API paths; destructive operations require isolated live accounts",
    ),
    "audit": (
        "daemon integrity/audit/HA services and durable store",
        "reads verification state or launches bounded local integrity/audit work",
        "Make tamper, corruption, availability, and handoff state observable and verifiable.",
        "audit verification, integrity sweeps, HA status, diagnosis, and release evidence",
        "implemented local controls; deployment/native evidence remains separate",
    ),
}

FEATURE_FLAG_GUIDANCE: dict[tuple[str, str], tuple[str, str, str]] = {
    ("pcloud-compat", "default"): ("Keep legacy ABI adapters out of ordinary builds.", "The modern canonical daemon path.", "The empty default prevents accidental SysV compatibility coupling."),
    ("pcloud-compat", "legacy-shm"): ("Opt into the legacy SysV shared-memory producer.", "Migration tools that must feed old folder-list consumers on supported Unix hosts.", "It is isolated, non-default, and ABI-tested; it is not a Windows/macOS portability surface."),
    ("pcloud-config", "default"): ("Keep configuration parsing free of external KMS SDKs.", "Single-user and ordinary daemon deployments.", "The default has no provider factory dependencies and therefore a smaller build/runtime surface."),
    ("pcloud-config", "kms-factory"): ("Enable construction of configured KMS providers.", "Enterprise builds that select key wrapping from typed configuration.", "The feature introduces the KMS and secret abstractions without selecting a vendor."),
    ("pcloud-config", "aws-kms"): ("Wire AWS KMS configuration to the AWS provider factory.", "AWS-managed wrapping keys and IAM-controlled deployments.", "It composes kms-factory with pcloud-kms/aws; AWS credentials still come from the normal provider chain."),
    ("pcloud-config", "pkcs11-kms"): ("Wire PKCS#11 configuration to an HSM provider.", "Experimental on-premises HSM or smart-card-backed wrapping.", "The provider is separately gated because native module/PIN handling and hardware qualification are deployment-specific."),
    ("pcloud-config", "vault-kms"): ("Wire HashiCorp Vault Transit configuration to the provider factory.", "Vault-operated envelope encryption.", "It enables only the Vault HTTP/provider dependencies and keeps tokens out of the config model."),
    ("pcloud-crypto", "default"): ("Select official-client compatibility plus the current RustCrypto implementation.", "Most builds, especially accounts also used by official pCloud clients.", "The default names both wire format and primitive provider so neither can change silently."),
    ("pcloud-crypto", "crypto-provider-rustcrypto"): ("Declare the implemented RustCrypto primitive provider.", "Normal non-FIPS builds.", "The marker participates in compile-time exactly-one-provider checks."),
    ("pcloud-crypto", "crypto-provider-aws-lc-fips"): ("Reserve a stable provider-selection seam for a future validated module.", "Downstream FIPS integration work and CI misuse detection.", "It intentionally compile-fails today; the name is not a FIPS claim and no validated provider ships."),
    ("pcloud-crypto", "legacy-c-compat"): ("Preserve an explicit compatibility marker for downstream legacy migration builds.", "Controlled C-client transition work.", "It is an empty marker; actual wire compatibility is provided by pclsync-v2 and must be proven by KATs."),
    ("pcloud-crypto", "pclsync-v2"): ("Compile the official-client-compatible Crypto primitives and profile codec.", "Existing pCloud Crypto content and cross-client sharing.", "RSA/OAEP, PBKDF2, AES modes, sectors, filenames, and auth tree are kept behind one auditable compatibility boundary."),
    ("pcloud-crypto", "test-helpers"): ("Expose controlled helper seams needed by tests.", "Property, KAT, and integration verification.", "Non-default visibility keeps production API pressure from test-only construction hooks."),
    ("pcloud-daemon", "default"): ("Run the daemon without optional telemetry exporters.", "Lean local single-user service.", "Core logging/health remain available while exporter dependency trees stay absent."),
    ("pcloud-daemon", "json-logs"): ("Emit structured JSON logs through pcloud-observability.", "Log aggregation and machine parsing.", "The feature changes formatting, not business behavior, and retains redaction rules."),
    ("pcloud-daemon", "metrics"): ("Enable the Prometheus-format metrics endpoint.", "Operations dashboards, alerts, and capacity observation.", "It reuses canonical metric families and a small exporter rather than instrumenting a second metrics path."),
    ("pcloud-daemon", "tracing-otlp"): ("Enable distributed trace export and trace-span integration.", "Enterprise request correlation across IPC, daemon, and external services.", "OTLP dependencies are feature-gated; trace context is typed and secrets remain excluded."),
    ("pcloud-embedded-sdk", "default"): ("Build the embedded facade with its ordinary in-process surface.", "First-party tests and controlled embedding.", "The marker provides a future compatibility point without enabling external integrations."),
    ("pcloud-fleet", "default"): ("Keep fleet code inert and null-by-default.", "Single-user or unmanaged installations.", "No management authority is introduced unless the operator configures it."),
    ("pcloud-fleet", "mtls"): ("Compile the historically named `MtlsFleetAgent` HTTPS transport.", "Experimental managed endpoints with a pinned controller CA and signed heartbeat/command envelopes.", "Despite the feature/type name, rustls uses no TLS client certificate: the server is CA-authenticated and the device authenticates at HTTP level with Ed25519 headers; the crate is not wired into pcloudd."),
    ("pcloud-idp", "default"): ("Enable the secure OIDC HTTP token-exchange path.", "Federated identity experiments and enterprise broker integration.", "The default avoids the plaintext test transport and keeps PKCE/discovery validation active."),
    ("pcloud-idp", "oidc-http-exchange"): ("Compile the real HTTPS OIDC token endpoint exchange.", "Authorization Code plus PKCE against an actual IdP.", "It provides the production-shaped exchange while pCloud trusted-issuer conversion remains an honest integration gap."),
    ("pcloud-idp", "insecure-plaintext-exchange"): ("Permit a plaintext exchange adapter for controlled tests.", "Local test fixtures only.", "Its alarming explicit name and non-default status prevent accidental production downgrade."),
    ("pcloud-kms", "default"): ("Provide the provider trait and NullKms without vendor SDKs.", "Single-user builds and enterprise code that does not select external custody.", "Null-by-default preserves behavior and keeps large/native dependencies out."),
    ("pcloud-kms", "aws"): ("Compile AWS SDK-based KMS wrapping.", "AWS IAM and managed/HSM-backed keys.", "Vendor dependencies and async runtime are isolated behind the feature; production routing needs live qualification."),
    ("pcloud-kms", "pkcs11"): ("Compile PKCS#11 wrapping support.", "On-premises HSM modules.", "Native module loading is opt-in and must be hardware-tested; no generic build can qualify an operator's device."),
    ("pcloud-kms", "serde"): ("Serialize non-secret KMS metadata and provider descriptors.", "Persisted envelopes or configuration adapters.", "Secret values remain in pcloud-secret and are not made serializable by this flag."),
    ("pcloud-kms", "vault"): ("Compile HashiCorp Vault Transit support.", "Self-hosted or managed Vault key custody.", "HTTPS, base64, and JSON dependencies are isolated and the provider refuses silent fallback."),
    ("pcloud-live-e2e", "default"): ("Compile the harness without implying that real-service tests ran.", "Normal workspace builds and offline CI.", "All real operations remain ignored and runtime-gated."),
    ("pcloud-live-e2e", "live"): ("Mark builds intended to include live-service qualification.", "Explicit credentialed release/soak jobs.", "The marker is not sufficient alone: runtime gates, disposable accounts, and serial execution are still required."),
    ("pcloud-observability", "default"): ("Provide logs, audit, health, SLO, and in-process metrics without exporters.", "Lean portable runtime instrumentation.", "The core stays dependency-light and can be consumed on every target."),
    ("pcloud-observability", "json-logs"): ("Enable JSON serialization for structured events.", "Central log collectors and deterministic parsing.", "It changes the sink representation while retaining canonical fields and redaction."),
    ("pcloud-observability", "prometheus-exporter"): ("Expose Prometheus text metrics over HTTP.", "Scraping and alerting.", "A small owned responder avoids requiring a full web framework in the daemon."),
    ("pcloud-observability", "tracing-otlp"): ("Compile OpenTelemetry/OTLP tracing integration.", "Cross-service latency and causality analysis.", "Heavy tracing dependencies and exporters remain optional while W3C context stays interoperable."),
    ("pcloud-resilience", "default"): ("Include transport metrics with the ordinary resilience stack.", "Operationally visible retries, circuit breaks, and request outcomes.", "Metrics make automated failure policy observable by default."),
    ("pcloud-resilience", "tokio-timeout"): ("Enable the cancellation-safe async timeout helper.", "Callers already operating inside Tokio.", "Tokio remains optional so blocking/core users do not inherit an async runtime."),
    ("pcloud-resilience", "transport-metrics"): ("Instrument resilient transport decisions.", "Dashboards and SLO enforcement around retries and breakers.", "It binds transport outcomes to canonical pcloud-observability metric families."),
}


def run(*args: str) -> str:
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def cargo_metadata() -> dict[str, Any]:
    return json.loads(
        run("cargo", "metadata", "--format-version", "1", "--no-deps")
    )


def git_files() -> list[str]:
    raw = run(
        "git",
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
    )
    return sorted(path for path in raw.split("\0") if path)


def sensitive_runtime_path(path_text: str) -> bool:
    path = Path(path_text)
    return (
        ".pcloud-rust-dev" in path.parts
        or path.name in {"auth_token", "credentials", "secrets"}
        or (path.name.startswith(".env") and path.name != ".env.example")
    )


def first_meaningful_line(path: Path) -> str:
    try:
        relative = path.relative_to(ROOT).as_posix()
    except ValueError:
        relative = path.as_posix()
    sensitive_or_runtime = sensitive_runtime_path(relative) or path.suffix.lower() in {
        ".blob",
        ".db",
        ".sqlite",
        ".sqlite3",
    }
    if sensitive_or_runtime:
        return ""
    try:
        if b"\x00" in path.read_bytes()[:8192]:
            return ""
    except OSError:
        return ""
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return ""
    suffix = path.suffix.lower()
    if suffix == ".rs":
        for line in lines[:120]:
            stripped = line.strip()
            if stripped.startswith("//!"):
                text = stripped[3:].strip().lstrip("#").strip()
                if text:
                    return text
        for line in lines[:120]:
            stripped = line.strip()
            if stripped.startswith("///"):
                text = stripped[3:].strip().lstrip("#").strip()
                if text:
                    return text
    if suffix in {".md", ".markdown"}:
        for line in lines[:80]:
            if line.startswith("#"):
                return line.lstrip("#").strip()
    for line in lines[:80]:
        stripped = line.strip()
        if not stripped or stripped.startswith(("#!", "<?xml", "<!DOCTYPE")):
            continue
        if stripped.startswith(("#", "//", ";", "<!--")):
            text = stripped.lstrip("#/;<!- ").rstrip("-> ").strip()
            if text:
                return text
    return ""


def compact(text: str, limit: int = 130) -> str:
    # Generated Markdown must never inherit NUL/control bytes from a binary,
    # journal, or other unignored working-tree artifact.
    text = re.sub(r"[\x00-\x1f\x7f]", " ", text)
    text = re.sub(r"\s+", " ", text).strip()
    if len(text) <= limit:
        return text
    return text[: limit - 1].rstrip() + "…"


def file_kind(path: str) -> str:
    p = Path(path)
    name = p.name
    suffix = p.suffix.lower()
    if name == "Cargo.toml":
        return "Cargo manifest"
    if name == "Cargo.lock":
        return "dependency lock"
    if name in {"main.rs", "lib.rs", "build.rs"}:
        return {"main.rs": "binary root", "lib.rs": "library root", "build.rs": "build script"}[name]
    if "/tests/" in f"/{path}" or p.parent.name == "tests":
        return "test"
    if "/benches/" in f"/{path}" or p.parent.name == "benches":
        return "benchmark"
    if "/examples/" in f"/{path}" or p.parent.name == "examples":
        return "example"
    if suffix == ".rs":
        return "Rust module"
    if suffix in {".md", ".markdown", ".rst"}:
        return "documentation"
    if suffix in {".yml", ".yaml"}:
        return "YAML/config"
    if suffix in {".toml", ".json", ".json5", ".ini", ".cfg", ".conf"}:
        return "configuration"
    if suffix in {".sh", ".zsh", ".bash", ".ps1", ".bat", ".cmd"}:
        return "script"
    if suffix in {".wxs", ".nuspec", ".desktop", ".plist", ".service", ".socket"}:
        return "packaging/service"
    if suffix in {".png", ".jpg", ".jpeg", ".gif", ".ico", ".svg"}:
        return "asset"
    if suffix in {".csv"}:
        return "data matrix"
    if name.startswith("Dockerfile"):
        return "container build"
    if name.startswith("."):
        return "project configuration"
    return suffix.lstrip(".") or "file"


def area_for(path: str) -> str:
    top = path.split("/", 1)[0]
    if top == "crates":
        return "crates"
    if top == "vendor":
        return "vendor"
    if top == "packaging":
        return "packaging"
    if top in {".github", ".cargo", "scripts", "tools", "fuzz"}:
        return "automation"
    if top in {"tests", "ops", "deploy"}:
        return "operations-tests"
    if top in {
        "docs",
    }:
        return "documentation"
    if top in {
        ".audits",
        ".audit-fragments",
        "CLAUDEREV",
        "GPTREV",
        ".plans",
    }:
        return "historical"
    if top.startswith(".") and top not in {".env.example", ".envrc"}:
        return "project-meta"
    return "root"


def describe_file(path_text: str) -> str:
    path = ROOT / path_text
    p = Path(path_text)
    kind = file_kind(path_text)
    if sensitive_runtime_path(path_text):
        return "Local development runtime state; contents are intentionally neither inspected nor published."
    overrides = {
        "crates/pcloud-fleet/Cargo.toml": "Historically named fleet transport: controller-authenticated HTTPS plus Ed25519 device/command signatures, not TLS client-certificate mTLS.",
        "crates/pcloud-p2p/src/discovery.rs": "Inert discovery contract: opens no socket, advertises nothing, and always reports an empty peer list.",
        "crates/pcloud-p2p/src/lib.rs": "Experimental P2P shell around an inert discovery runtime; no current peer networking, planning, or transfer path.",
        "crates/pcloud-fleet/src/lib.rs": "Standalone fleet contract using controller-authenticated HTTPS plus Ed25519 device/command signatures; not wired into pcloudd.",
    }
    if path_text in overrides:
        return overrides[path_text]
    source = first_meaningful_line(path)
    if source:
        return compact(source)
    if p.name == "Cargo.toml":
        return "Defines package/workspace metadata, features, targets, and dependencies."
    if p.name == "Cargo.lock":
        return "Pins the resolved dependency graph for reproducible workspace builds."
    if p.name == "main.rs":
        return "Executable process entrypoint and top-level lifecycle."
    if p.name == "lib.rs":
        return "Crate root, public exports, and crate-level contract."
    if p.name == "build.rs":
        return "Cargo build-time platform or generated-code integration."
    if kind == "test":
        return "Executable verification for the behavior named by this file."
    if kind == "benchmark":
        return "Performance benchmark for the behavior named by this file."
    if kind == "example":
        return "Runnable usage example."
    if path_text.startswith(".github/workflows/"):
        return "GitHub Actions workflow for the named build, test, release, or qualification gate."
    if path_text.startswith("packaging/"):
        return "Packaging, service lifecycle, installer, or platform-distribution asset."
    if path_text.startswith("vendor/"):
        return "Vendored upstream dependency file; not a pcloud-rs architectural owner."
    if p.suffix == ".rs":
        return f"Rust {p.stem.replace('_', ' ')} module."
    return f"{kind.capitalize()} used by the {area_for(path_text).replace('-', ' ')} area."


def file_rationale(path_text: str) -> str:
    """Explain why a Git-visible unit belongs in the project."""
    kind = file_kind(path_text)
    area = area_for(path_text)
    if sensitive_runtime_path(path_text):
        return "Make the development-state boundary visible without ingesting credentials, journals, databases, or staged content into the site."
    if path_text.startswith("vendor/"):
        return "Keep the exact reviewed upstream source available for reproducible, inspectable builds."
    if kind == "Cargo manifest":
        return "Make package ownership, targets, dependencies, and compile-time choices explicit to Cargo and reviewers."
    if kind == "dependency lock":
        return "Freeze dependency resolution so the same source does not silently select a different graph."
    if kind in {"Rust module", "library root", "binary root"}:
        return "Give this responsibility a named Rust ownership boundary that can be reviewed, tested, and changed deliberately."
    if kind == "build script":
        return "Keep deterministic build-time preparation separate from runtime behavior."
    if kind == "test":
        return "Turn the named behavior or invariant into executable regression evidence."
    if kind == "benchmark":
        return "Make performance and capacity claims measurable instead of anecdotal."
    if kind == "example":
        return "Preserve a runnable intended-use path for adopters and maintainers."
    if kind == "script":
        return "Automate a repeatable developer, qualification, packaging, or operations procedure."
    if kind in {"configuration", "YAML/config", "project configuration"}:
        return "Version a machine-readable policy or tool input instead of relying on undocumented local state."
    if kind in {"packaging/service", "container build"} or area == "packaging":
        return "Encode install, service, lifecycle, or distribution behavior as a reviewable artifact."
    if kind == "documentation":
        return "Preserve the contract, rationale, procedure, or evidence that source alone cannot communicate safely."
    if kind == "data matrix":
        return "Keep structured product or compatibility decisions complete and mechanically consumable."
    if kind == "asset":
        return "Provide a user-visible or package-visible resource required by the surrounding product surface."
    if area == "historical":
        return "Retain dated review evidence and decision history without treating it as current implementation truth."
    return f"Support the repository's {area.replace('-', ' ')} concern with an explicit, versioned artifact."


def file_good_for(path_text: str) -> str:
    """Explain where a Git-visible unit is useful and its main strength."""
    kind = file_kind(path_text)
    area = area_for(path_text)
    if sensitive_runtime_path(path_text):
        return "Understanding which local paths are runtime output; never a release input, source reference, or publishable content source."
    if path_text.startswith("vendor/"):
        return "Offline/reproducible builds and dependency audit; it is not a pcloud-rs public entrypoint."
    if kind in {"Rust module", "library root", "binary root", "build script"}:
        return "Implementation navigation and ownership: the source link is the authority for its exact contract."
    if kind == "test":
        return "Regression detection in its named scope; fixture or mock success does not substitute for live/native proof."
    if kind == "benchmark":
        return "Before/after comparison under the recorded harness and corpus, not universal performance promises."
    if kind == "example":
        return "Learning and smoke testing the intended API sequence; production policy still belongs to the caller."
    if kind == "script":
        return "Repeatable local/remote automation; inspect prerequisites and side effects before operator use."
    if kind in {"packaging/service", "container build"} or area == "packaging":
        return "Building or operating the named platform artifact; native install/signing qualification remains separate."
    if kind in {"configuration", "YAML/config", "project configuration", "Cargo manifest", "dependency lock"}:
        return "Reproducible builds and explicit tool/runtime behavior because relevant choices are version-controlled."
    if kind == "documentation":
        return "Onboarding, operation, design review, or evidence lookup; current executable source wins on drift."
    if area == "historical":
        return "Understanding prior findings and decisions; use dated status and do not infer present maturity."
    return f"Navigation and review of the {area.replace('-', ' ')} surface; its surrounding owner defines operational use."


def source_link(path: str, line: int | None = None) -> str:
    url = f"{GITHUB}/{path}"
    if line:
        url += f"#L{line}"
    return url


def md_escape(text: str) -> str:
    return (
        text.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace("|", "\\|")
        .replace("[", "\\[")
        .replace("]", "\\]")
        .replace("\n", " ")
    )


def maturity(name: str) -> str:
    if name in STABLE:
        return "Stable public contract"
    if name in INTERNAL:
        return "Internal stable"
    if name in EVOLVING:
        return "Evolving product surface"
    if name in VERIFY:
        return "Verification support"
    if name in TOOLING:
        return "Repository infrastructure"
    return "Experimental / bounded"


def crate_profile(name: str) -> tuple[str, str, str]:
    return CRATE_PROFILES.get(
        name,
        (
            "Keep this package's concern behind an explicit workspace boundary.",
            "The behavior described by its Cargo targets and source modules.",
            "A separate crate makes dependencies, ownership, testing, and maturity visible.",
        ),
    )


def crate_files(package: dict[str, Any], files: list[str]) -> list[str]:
    manifest = Path(package["manifest_path"])
    directory = manifest.parent.relative_to(ROOT).as_posix()
    prefix = directory + "/"
    return [path for path in files if path == f"{directory}/Cargo.toml" or path.startswith(prefix)]


def rust_items(path_text: str) -> list[tuple[str, str, str, str, int]]:
    path = ROOT / path_text
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    result: list[tuple[str, str, str, str, int]] = []
    docs: list[str] = []
    for number, line in enumerate(lines, 1):
        stripped = line.strip()
        if stripped.startswith("///"):
            docs.append(stripped[3:].strip())
            continue
        if stripped.startswith("#[") or not stripped:
            continue
        match = FUNCTION_ITEM.match(line) or OTHER_ITEM.match(line)
        if match:
            summary = compact(" ".join(docs), 110) if docs else ""
            p2p_overrides = {
                ("crates/pcloud-p2p/src/lib.rs", "discovery"): "Configuration plus an inert discovery handle; no LAN scan or peer inventory is implemented.",
                ("crates/pcloud-p2p/src/lib.rs", "SERVICE_TYPE"): "Reserved future mDNS service string; current code neither advertises nor browses it.",
                ("crates/pcloud-p2p/src/lib.rs", "start"): "Creates an inert local handle only; opens no socket and performs no mDNS work.",
                ("crates/pcloud-p2p/src/lib.rs", "stop"): "Drops the inert local handle; there is no network responder to stop.",
                ("crates/pcloud-p2p/src/lib.rs", "is_running"): "Reports whether the inert handle exists, not discovery/network health.",
            }
            summary = p2p_overrides.get((path_text, match.group("name")), summary)
            visibility = (match.group("visibility") or "private").strip()
            result.append(
                (
                    visibility,
                    match.group("kind"),
                    match.group("name"),
                    summary,
                    number,
                )
            )
        docs = []
    return result


def package_slug(package: dict[str, Any]) -> str:
    return package["name"]


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.tmp-{os.getpid()}")
    temporary.write_text(text.rstrip() + "\n", encoding="utf-8")
    os.replace(temporary, path)


def package_page(package: dict[str, Any], files: list[str]) -> str:
    name = package["name"]
    manifest_rel = Path(package["manifest_path"]).relative_to(ROOT).as_posix()
    directory = str(Path(manifest_rel).parent)
    targets = package.get("targets", [])
    dependencies = sorted(
        {
            dependency.get("rename") or dependency["name"]
            for dependency in package.get("dependencies", [])
        }
    )
    features = package.get("features", {})
    package_files = crate_files(package, files)
    readme_path = ROOT / directory / "README.md"
    description = package.get("description") or (
        first_meaningful_line(readme_path) if readme_path.exists() else ""
    )
    if not description:
        description = first_meaningful_line(ROOT / directory / "src/lib.rs")
    rationale, good_for, strength = crate_profile(name)
    lines = [
        f"# `{name}`",
        "",
        f"**Maturity:** {maturity(name)}",
        "",
        f"**Version:** `{package['version']}`",
        "",
        f"**Directory:** `{directory}`",
        "",
        f"**Manifest:** [`{manifest_rel}`]({source_link(manifest_rel)})",
        "",
        md_escape(description or "Cargo workspace package.") ,
        "",
        "## Feature-family profile",
        "",
        f"**Why it exists.** {rationale}",
        "",
        f"**What it is good for.** {good_for}",
        "",
        f"**Why it is good at that job.** {strength}",
        "",
        "## Targets",
        "",
        "| Cargo target | Kinds | Source |",
        "|---|---|---|",
    ]
    for target in targets:
        src = Path(target["src_path"]).relative_to(ROOT).as_posix()
        kinds = ", ".join(target.get("kind", []))
        lines.append(
            f"| `{target['name']}` | {kinds} | [`{src}`]({source_link(src)}) |"
        )
    lines += [
        "",
        "## Direct dependencies",
        "",
        ", ".join(f"`{name}`" for name in dependencies) if dependencies else "None.",
        "",
        "## Cargo features",
        "",
    ]
    if features:
        lines += [
            "| Feature | Enables |",
            "|---|---|",
        ]
        for feature, values in sorted(features.items()):
            value_text = ", ".join(f"`{value}`" for value in values) or "empty marker"
            lines.append(f"| `{feature}` | {value_text} |")
    else:
        lines.append("No declared package features.")
    lines += [
        "",
        f"## File inventory ({len(package_files)})",
        "",
        "| File | Kind | Role |",
        "|---|---|---|",
    ]
    for path in package_files:
        source = (
            f"`{path}` (source link withheld)"
            if sensitive_runtime_path(path)
            else f"[`{path}`]({source_link(path)})"
        )
        lines.append(
            f"| {source} | {file_kind(path)} | "
            f"{md_escape(describe_file(path))} |"
        )
    symbols: list[tuple[str, str, str, str, int, str]] = []
    for path in package_files:
        if path.endswith(".rs"):
            for visibility, kind, symbol, doc, line in rust_items(path):
                symbols.append((path, visibility, kind, symbol, line, doc))
    public_count = sum(
        1 for _, visibility, _, _, _, _ in symbols if visibility.startswith("pub")
    )
    lines += [
        "",
        f"## Rust declaration index ({len(symbols)} total; {public_count} visible)",
        "",
    ]
    if symbols:
        lines += [
            "| Item | Visibility | Kind | Source | Documentation hint |",
            "|---|---|---|---|---|",
        ]
        for path, visibility, kind, symbol, line, doc in symbols:
            lines.append(
                f"| `{symbol}` | `{visibility}` | {kind} | "
                f"[`{path}:{line}`]({source_link(path, line)}) | "
                f"{md_escape(doc or 'Read the source/rustdoc for the exact contract.')} |"
            )
    else:
        lines.append(
            "No named Rust declarations were found. The package may be manifest-only "
            "or rely on generated source."
        )
    lines += [
        "",
        "## Usage guidance",
        "",
    ]
    label = maturity(name)
    if label == "Stable public contract":
        lines.append(
            "This is the intended third-party SemVer boundary. The daemon must be "
            "running and authenticated; registry release qualification is tracked separately."
        )
    elif label == "Internal stable":
        lines.append(
            "Core workspace code may depend on this contract. External applications should "
            "prefer `pcloud-sdk` unless they intentionally own the lower-level runtime."
        )
    elif label == "Verification support":
        lines.append(
            "This package proves behavior and is not a shipped end-user runtime surface."
        )
    elif label == "Evolving product surface":
        lines.append(
            "This is product code but not a frozen external library contract. Check current "
            "status and native qualification before deployment claims."
        )
    elif label == "Repository infrastructure":
        lines.append(
            "This package is the authoritative local build, test, coverage, packaging, "
            "qualification, and release orchestration surface; it is tooling rather than "
            "a shipped pCloud runtime library."
        )
    else:
        lines.append(
            "Treat this package as experimental, optional, enterprise-bounded, or unshipped "
            "until its feature and release evidence says otherwise."
        )
    return "\n".join(lines)


AREA_TITLES = {
    "root": "Root product and policy files",
    "crates": "Workspace crate files",
    "documentation": "Documentation files",
    "packaging": "Packaging and service files",
    "automation": "Automation, workflows, scripts, and fuzz files",
    "operations-tests": "Operations, deployment, tools, and cross-crate tests",
    "historical": "Historical audits, plans, and review evidence",
    "project-meta": "Project metadata and local development definitions",
    "vendor": "Vendored upstream files",
}


def inventory_page(area: str, paths: list[str]) -> str:
    kinds = Counter(file_kind(path) for path in paths)
    lines = [
        f"# {AREA_TITLES[area]}",
        "",
        f"This generated page covers **{len(paths)}** Git-visible files.",
        "",
        "Kind summary: "
        + ", ".join(f"{kind}: {count}" for kind, count in kinds.most_common()),
        "",
    ]
    if area == "vendor":
        lines += [
            "> Vendored files are upstream implementation details. They are listed for "
            "exhaustiveness but are not pcloud-rs-owned entrypoints.",
            "",
        ]
    if area == "historical":
        lines += [
            "> Historical reports describe past snapshots. Prefer current source, tests, "
            "`STATUS.md`, and release evidence for present-tense claims.",
            "",
        ]
    lines += [
        "| File | Kind | Source-derived role | Why it exists | Good at / for, and why |",
        "|---|---|---|---|---|",
    ]
    for path in paths:
        source = (
            f"`{path}` (source link withheld)"
            if sensitive_runtime_path(path)
            else f"[`{path}`]({source_link(path)})"
        )
        lines.append(
            f"| {source} | {file_kind(path)} | "
            f"{md_escape(describe_file(path))} | {md_escape(file_rationale(path))} | "
            f"{md_escape(file_good_for(path))} |"
        )
    return "\n".join(lines)


def crate_index(packages: list[dict[str, Any]], files: list[str]) -> str:
    lines = [
        "# Workspace crate catalog",
        "",
        f"Cargo currently reports **{len(packages)} packages**.",
        "",
        "| Package | Version | Maturity | Targets | Files | Directory |",
        "|---|---:|---|---|---:|---|",
    ]
    for package in sorted(packages, key=lambda item: item["name"]):
        directory = Path(package["manifest_path"]).parent.relative_to(ROOT).as_posix()
        kinds = sorted(
            {
                kind
                for target in package.get("targets", [])
                for kind in target.get("kind", [])
            }
        )
        count = len(crate_files(package, files))
        slug = package_slug(package)
        lines.append(
            f"| [`{package['name']}`](./{slug}.md) | `{package['version']}` | "
            f"{maturity(package['name'])} | {', '.join(kinds)} | {count} | "
            f"`{directory}` |"
        )
    lines += [
        "",
        "## Dependency overview",
        "",
        "Each package page lists its direct Cargo dependencies. Read arrows as "
        "“uses”; this schematic highlights the primary product path rather than "
        "every optional/test edge.",
        "",
        "```text",
        "pcloud-cli ─────────────► pcloud-ipc ─────► pcloud-model",
        "pcloud-sdk ─────────────► pcloud-ipc",
        "pcloud-web / webdav* ───► pcloud-ipc",
        "                                  ▲",
        "                                  │",
        "pcloud-daemon ─► pcloud-backends ─┼─► pcloud-proto ─► TLS/pCloud",
        "       │              │           │",
        "       │              ├───────────┴─► pcloud-store / cache",
        "       ├──────────────► pcloud-engine",
        "       ├──────────────► pcloud-fs",
        "       ├──────────────► pcloud-crypto / auth / secret",
        "       └──────────────► observability / resilience / policy",
        "",
        "* experimental/unshipped where documented",
        "```",
    ]
    return "\n".join(lines)


def workspace_snapshot(files: list[str], packages: list[dict[str, Any]]) -> str:
    head = run("git", "rev-parse", "HEAD")
    dirty = bool(run("git", "status", "--porcelain"))
    timestamp = dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat()
    counts = Counter(area_for(path) for path in files)
    lines = [
        "# Generated workspace snapshot",
        "",
        f"- Generated: `{timestamp}`",
        f"- Git HEAD: `{head}`",
        f"- Worktree: `{'dirty' if dirty else 'clean'}`",
        f"- Cargo packages: **{len(packages)}**",
        f"- Git-visible files inventoried: **{len(files)}**",
        "",
        "## File coverage",
        "",
        "| Area | Files | Inventory |",
        "|---|---:|---|",
    ]
    for area in AREA_TITLES:
        lines.append(
            f"| {AREA_TITLES[area]} | {counts.get(area, 0)} | "
            f"[open](inventory/{area}.md) |"
        )
    lines += [
        "",
        "> A dirty snapshot is useful for development navigation but is not a "
        "reproducible release baseline.",
    ]
    return "\n".join(lines)


def package_feature_catalog(packages: list[dict[str, Any]]) -> str:
    lines = [
        "# Complete package feature-family catalog",
        "",
        "Every Cargo package is an architectural feature family. This table explains",
        "why each boundary exists, what it is best used for, and why its design is a",
        "good fit. Follow the package link for all targets, files, flags, declarations,",
        "and exact source entrypoints.",
        "",
        f"**Coverage: {len(packages)} of {len(packages)} Cargo packages.**",
        "",
        "| Feature family | Maturity | Why it exists | Good for | Why it is effective |",
        "|---|---|---|---|---|",
    ]
    for package in sorted(packages, key=lambda item: item["name"]):
        name = package["name"]
        rationale, good_for, strength = crate_profile(name)
        lines.append(
            f"| [`{name}`](../crates/{name}.md) | {maturity(name)} | "
            f"{md_escape(rationale)} | {md_escape(good_for)} | {md_escape(strength)} |"
        )
    return "\n".join(lines)


def cargo_feature_catalog(packages: list[dict[str, Any]]) -> str:
    rows: list[tuple[str, str, list[str]]] = []
    for package in sorted(packages, key=lambda item: item["name"]):
        for feature, enables in sorted(package.get("features", {}).items()):
            rows.append((package["name"], feature, enables))
    lines = [
        "# Complete Cargo feature-flag catalog",
        "",
        "Cargo features are compile-time product switches, dependency boundaries, or",
        "explicit forward-compatibility seams. An empty marker still has meaning; it",
        "does not prove that the named integration is production-ready.",
        "",
        f"**Coverage: {len(rows)} of {len(rows)} declared package feature flags.**",
        "",
        "| Package / flag | Enables | Why it exists | Good for | Why / caveat |",
        "|---|---|---|---|---|",
    ]
    for package, feature, enables in rows:
        rationale, good_for, strength = FEATURE_FLAG_GUIDANCE.get(
            (package, feature),
            (
                f"Provide an explicit compile-time switch named {feature}.",
                crate_profile(package)[1],
                "The flag keeps optional behavior and dependencies visible in Cargo metadata; inspect its enabled edges before relying on it.",
            ),
        )
        enabled = ", ".join(f"`{value}`" for value in enables) or "empty marker"
        lines.append(
            f"| [`{package}`](../crates/{package}.md) / `{feature}` | {enabled} | "
            f"{md_escape(rationale)} | {md_escape(good_for)} | {md_escape(strength)} |"
        )
    return "\n".join(lines)


def rust_enum_variants(path_text: str, enum_name: str) -> list[tuple[str, str, int]]:
    """Return top-level Rust enum variants with adjacent rustdoc and line."""
    lines = (ROOT / path_text).read_text(encoding="utf-8").splitlines()
    start = re.compile(rf"\b(?:pub\s+)?enum\s+{re.escape(enum_name)}\s*\{{")
    variant = re.compile(r"^\s*([A-Z][A-Za-z0-9_]*)\s*(?:,|\{|\()");
    active = False
    depth = 0
    docs: list[str] = []
    result: list[tuple[str, str, int]] = []
    for line_number, line in enumerate(lines, 1):
        if not active:
            if start.search(line):
                active = True
                depth = line.count("{") - line.count("}")
            continue
        if depth == 1:
            stripped = line.strip()
            if stripped.startswith("///"):
                docs.append(stripped[3:].strip())
            elif stripped.startswith("#[") or not stripped or stripped.startswith("//"):
                pass
            else:
                match = variant.match(line)
                if match:
                    result.append((match.group(1), compact(" ".join(docs), 280), line_number))
                docs = []
        depth += line.count("{") - line.count("}")
        if depth == 0:
            break
    return result


def surface_family(name: str) -> str:
    lowered = name.lower()
    if any(token in lowered for token in ("crypto", "folderkey", "filekey")):
        return "crypto"
    if any(token in lowered for token in ("login", "logout", "auth", "twofactor", "passwordsubmission", "session")):
        return "auth"
    if any(token in lowered for token in ("notification",)):
        return "notifications"
    if any(token in lowered for token in ("sync", "conflict", "localscan", "issyncable", "suggestion")):
        return "sync"
    if any(token in lowered for token in ("publiclink", "publink", "uploadlink", "treelink", "bookmark", "linkaccess")):
        return "links"
    if any(token in lowered for token in ("share", "contact", "team")):
        return "shares"
    if any(token in lowered for token in ("mount", "unmount", "filesystem")):
        return "mount"
    if any(token in lowered for token in ("backup", "snapshot", "device")):
        return "backup"
    if any(token in lowered for token in ("audit", "integrity", "hastatus", "verifychain", "verifypath")):
        return "audit"
    if any(token in lowered for token in ("status", "health", "pending", "slo", "shutdown", "drain", "reload", "start", "help")):
        return "control"
    if any(token in lowered for token in ("upload", "download", "filelink", "readfilerange", "transfer")):
        return "transfer"
    if any(token in lowered for token in ("userinfo", "account", "apiserver", "language", "promo", "value", "verifyemail", "lostpassword", "register")):
        return "account"
    if any(token in lowered for token in ("folder", "path", "remote")) or lowered.startswith("stat"):
        return "remote-fs"
    if any(token in lowered for token in ("pause", "resume")):
        return "control"
    return "control"


def command_routes(command_names: set[str]) -> dict[str, tuple[list[str], list[str], bool]]:
    """Extract each CLI arm's Request/Method route from the canonical lowering match."""
    source = (ROOT / "crates/pcloud-cli/src/commands.rs").read_text(encoding="utf-8")
    start = source.index("pub fn into_request")
    end = source.index("/// Read the `PCLOUD_FORCE_UMOUNT`", start)
    block = source[start:end]
    result: dict[str, tuple[list[str], list[str], bool]] = {}
    local_only = {"Help", "Start", "Drain", "Reload", "Doctor", "MigrateFromC", "Verify"}
    next_arm = re.compile(r"\n\s{12}Self::[A-Z]")
    for name in command_names:
        match = re.search(rf"\bSelf::{re.escape(name)}\b", block)
        if not match:
            result[name] = ([], [], name in local_only)
            continue
        arrow = block.find("=>", match.end())
        if arrow < 0:
            result[name] = ([], [], name in local_only)
            continue
        following = next_arm.search(block, arrow + 2)
        segment = block[arrow + 2 : following.start() if following else len(block)]
        requests = list(dict.fromkeys(re.findall(r"\bRequest::([A-Z][A-Za-z0-9_]*)", segment)))
        methods = list(dict.fromkeys(re.findall(r"\bMethod::([A-Z][A-Za-z0-9_]*)", segment)))
        result[name] = (requests, methods, name in local_only)
    return result


def enum_references(package: dict[str, Any]) -> tuple[set[str], set[str]]:
    directory = Path(package["manifest_path"]).parent / "src"
    request_names: set[str] = set()
    method_names: set[str] = set()
    for path in directory.rglob("*.rs"):
        text = path.read_text(encoding="utf-8", errors="replace")
        request_names.update(re.findall(r"\bRequest::([A-Z][A-Za-z0-9_]*)", text))
        method_names.update(re.findall(r"\bMethod::([A-Z][A-Za-z0-9_]*)", text))
    return request_names, method_names


def direct_exposure(
    name: str,
    kind: str,
    usage: dict[str, tuple[set[str], set[str]]],
) -> str:
    index = 0 if kind == "Request" else 1
    surfaces: list[str] = []
    if name in usage["pcloud-cli"][index]:
        surfaces.append("CLI")
    if name in usage["pcloud-sdk"][index]:
        surfaces.append("stable SDK")
    if name in usage["pcloud-embedded-sdk"][index]:
        surfaces.append("embedded SDK")
    return ", ".join(surfaces) if surfaces else "no direct construction in CLI/SDK crates"


def current_surface_catalog(packages: list[dict[str, Any]]) -> str:
    command_path = "crates/pcloud-cli/src/commands.rs"
    ipc_path = "crates/pcloud-ipc/src/methods.rs"
    commands = rust_enum_variants(command_path, "Command")
    methods = rust_enum_variants(ipc_path, "Method")
    requests = rust_enum_variants(ipc_path, "Request")
    routes = command_routes({name for name, _, _ in commands})

    package_by_name = {package["name"]: package for package in packages}
    usage = {
        name: enum_references(package_by_name[name])
        for name in ("pcloud-cli", "pcloud-sdk", "pcloud-embedded-sdk")
    }
    runtime_path = ROOT / "crates/pcloud-daemon/src/runtime.rs"
    runtime = runtime_path.read_text(encoding="utf-8")
    runtime_lines = runtime.splitlines()

    def handler(kind: str, name: str) -> str:
        needle = f"{kind}::{name}"
        for line_number, line in enumerate(runtime_lines, 1):
            if needle in line:
                return f"[wired runtime arm]({source_link('crates/pcloud-daemon/src/runtime.rs', line_number)})"
        return '<span class="atlas-experimental">no direct pcloudd runtime arm found</span>'

    binaries: list[tuple[str, str, str, str]] = []
    for package in packages:
        rationale, good_for, _ = crate_profile(package["name"])
        for target in package.get("targets", []):
            if "bin" not in target.get("kind", []):
                continue
            source = Path(target["src_path"]).relative_to(ROOT).as_posix()
            binaries.append((target["name"], package["name"], source, f"{rationale} Good for {good_for}"))

    lines = [
        "# Complete current CLI, IPC, SDK, and binary surface",
        "",
        "This catalog is generated from the current Rust enums and Cargo metadata. It",
        "complements the legacy C-parity catalog: the latter answers compatibility",
        "decisions; this page answers what today's pcloud-rs clients can name and route.",
        "",
        f"**Coverage: {len(commands)} of {len(commands)} CLI commands, "
        f"{len(methods)} of {len(methods)} argumentless IPC methods, "
        f"{len(requests)} of {len(requests)} argument-bearing IPC requests, and "
        f"{len(binaries)} of {len(binaries)} Cargo binaries.**",
        "",
        "> “Wired runtime arm” means `pcloudd::RuntimeShell::handle_request` has a direct",
        "> match arm. It does not prove live pCloud success, native qualification, stable-SDK",
        "> exposure, or that optional standalone enterprise crates are composed into pcloudd.",
        "",
        "## Executable entrypoints",
        "",
        "| Binary | Owning package | Why it exists / good for | Source |",
        "|---|---|---|---|",
    ]
    for name, package, source, explanation in sorted(binaries):
        lines.append(
            f"| `{name}` | [`{package}`](../crates/{package}.md) | {md_escape(explanation)} | "
            f"[`{source}`]({source_link(source)}) |"
        )

    lines += [
        "",
        f"## CLI `Command` ({len(commands)})",
        "",
        "Each command is parsed once and lowered by `Command::into_request`. A CLI-local",
        "command may show a harmless fallback route that is not its real execution path.",
        "",
        "| Command | What it does and why it exists | IPC route | Runtime owner / side effect | Good at / for | Platform and maturity |",
        "|---|---|---|---|---|---|",
    ]
    for name, doc, line_number in commands:
        request_names, method_names, local = routes[name]
        route_parts = [f"`Request::{value}`" for value in request_names]
        route_parts += [f"`Method::{value}`" for value in method_names]
        route = " + ".join(route_parts) or "no enum route extracted"
        if local:
            route = f"CLI-local; defensive fallback {route}"
        family_name = next((value for value in request_names if value != "Plain"), None)
        family = surface_family(family_name or (method_names[0] if method_names else name))
        owner, effect, rationale, good_for, maturity_label = SURFACE_FAMILY_PROFILES[family]
        explanation = doc or f"Parsed command for the `{name}` operation."
        platform = "native mount qualification required" if family == "mount" else "portable client surface"
        lines.append(
            f"| [`{name}`]({source_link(command_path, line_number)}) | {md_escape(explanation)} "
            f"**Rationale:** {md_escape(rationale)} | {route} | {md_escape(owner)}; {md_escape(effect)} | "
            f"{md_escape(good_for)} | {platform}; {maturity_label} |"
        )

    lines += [
        "",
        f"## IPC `Method` ({len(methods)})",
        "",
        "Argumentless operations travel as `Request::Plain { method }`.",
        "",
        "| Method | Full behavior | Why it exists / good for | Owner and side effect | Direct client exposure | pcloudd reachability |",
        "|---|---|---|---|---|---|",
    ]
    for name, doc, line_number in methods:
        family = surface_family(name)
        owner, effect, rationale, good_for, _ = SURFACE_FAMILY_PROFILES[family]
        lines.append(
            f"| [`{name}`]({source_link(ipc_path, line_number)}) | {md_escape(doc or 'Argumentless typed daemon operation.')} | "
            f"{md_escape(rationale)} Good for {md_escape(good_for)}. | {md_escape(owner)}; {md_escape(effect)} | "
            f"{direct_exposure(name, 'Method', usage)} | {handler('Method', name)} |"
        )

    lines += [
        "",
        f"## IPC `Request` ({len(requests)})",
        "",
        "Argument-bearing requests are the complete current local wire vocabulary.",
        "Secret-bearing variants use redacted transit wrappers and the whole enum has a",
        "manual redacted `Debug`; local IPC serialization is still a real trust boundary.",
        "",
        "| Request | Full behavior | Why it exists / good for | Owner and side effect | Direct client exposure | pcloudd reachability |",
        "|---|---|---|---|---|---|",
    ]
    for name, doc, line_number in requests:
        family = surface_family(name)
        owner, effect, rationale, good_for, _ = SURFACE_FAMILY_PROFILES[family]
        lines.append(
            f"| [`{name}`]({source_link(ipc_path, line_number)}) | {md_escape(doc or 'Typed argument-bearing daemon request.')} | "
            f"{md_escape(rationale)} Good for {md_escape(good_for)}. | {md_escape(owner)}; {md_escape(effect)} | "
            f"{direct_exposure(name, 'Request', usage)} | {handler('Request', name)} |"
        )
    return "\n".join(lines)


def source_unit_kind(path: str) -> str:
    if path.endswith("/build.rs"):
        return "build helper"
    if "/tests/" in f"/{path}":
        return "integration test"
    if "/benches/" in f"/{path}":
        return "benchmark"
    if "/examples/" in f"/{path}":
        return "example"
    if "/fuzz/" in f"/{path}" or "fuzz_targets" in path:
        return "fuzz target/helper"
    if "/src/bin/" in f"/{path}" or path.endswith("/src/main.rs"):
        return "binary entrypoint/helper"
    return "runtime/library unit"


def source_unit_why(path: str, package: str, kind: str, role: str) -> str:
    label = Path(path).stem.replace("_", " ")
    if kind == "integration test":
        return f"Prove the {label} behavior at a crate or process boundary instead of relying only on unit tests."
    if kind == "benchmark":
        return f"Measure the {label} path so performance and capacity decisions have executable evidence."
    if kind == "example":
        return f"Show a runnable {label} workflow and keep the intended call sequence discoverable."
    if kind == "fuzz target/helper":
        return f"Drive malformed or adversarial inputs through the {label} boundary to find parser and invariant failures."
    if kind == "build helper":
        return f"Perform the {label} package's deterministic build-time preparation outside runtime code."
    if kind == "binary entrypoint/helper":
        return f"Own the {label} executable lifecycle without mixing process concerns into reusable library code."
    if Path(path).name == "lib.rs":
        return f"Declare and compose the `{package}` crate boundary. {crate_profile(package)[0]}"
    return (
        f"Keep this responsibility explicit: {role.rstrip('.')}. A dedicated {label} "
        f"unit localizes dependencies, review, and regression testing inside `{package}`."
    )


def source_unit_good(path: str, package: str, kind: str, role: str) -> str:
    label = Path(path).stem.replace("_", " ")
    strength = crate_profile(package)[2]
    if kind == "integration test":
        return f"Protecting {label} boundary behavior from regressions. {strength}"
    if kind == "benchmark":
        return f"Measuring {label} changes under a stable harness. {strength}"
    if kind == "example":
        return f"Teaching and smoke-testing the {label} call path. {strength}"
    if kind == "fuzz target/helper":
        return f"Finding malformed-input and invariant failures around {label}. {strength}"
    if kind == "build helper":
        return f"Reproducible preparation for the package build. {strength}"
    if kind == "binary entrypoint/helper":
        return f"Process startup, argument/lifecycle ownership, and operator integration. {strength}"
    return f"The focused `{package}` responsibility described as: {role.rstrip('.')}. {strength}"


def source_unit_catalog(packages: list[dict[str, Any]], files: list[str]) -> str:
    package_units: dict[str, list[str]] = defaultdict(list)
    for package in packages:
        for path in crate_files(package, files):
            if path.endswith(".rs"):
                package_units[package["name"]].append(path)
    total = sum(len(paths) for paths in package_units.values())
    lines = [
        "# Complete internal module and helper catalog",
        "",
        "This is the exhaustive implementation-unit layer of the feature encyclopedia.",
        "It includes runtime modules, private helpers, binaries, build scripts, examples,",
        "benchmarks, fuzz targets, and tests. The adjacent generated crate pages index",
        "every named Rust declaration inside these units.",
        "",
        f"**Coverage: {total} of {total} Rust source/test/helper files owned by Cargo packages.**",
        "",
        "> A file being present means an implementation or verification surface exists; it",
        "> does not automatically make that surface public, enabled, wired, or release-qualified.",
        "",
    ]
    for package in sorted(packages, key=lambda item: item["name"]):
        name = package["name"]
        paths = sorted(package_units.get(name, []))
        if not paths:
            continue
        _, package_good_for, _ = crate_profile(name)
        lines += [
            f"## `{name}` ({len(paths)} units)",
            "",
            f"Package context: {package_good_for}",
            "",
            "| Unit / feature | Kind | What it does | Why it exists | Good at / for, and why |",
            "|---|---|---|---|---|",
        ]
        for path in paths:
            kind = source_unit_kind(path)
            role = describe_file(path)
            why = source_unit_why(path, name, kind, role)
            item_count = len(rust_items(path))
            good = (
                f"{source_unit_good(path, name, kind, role)} This unit contains "
                f"{item_count} indexed Rust declaration{'s' if item_count != 1 else ''}."
            )
            lines.append(
                f"| [`{path}`]({source_link(path)}) | {kind} | {md_escape(role)} | "
                f"{md_escape(why)} | {md_escape(good)} |"
            )
        lines += [
            "",
            f"All declarations: [`{name}` generated crate page](../crates/{name}.md).",
            "",
        ]
    return "\n".join(lines)


def api_capability_catalog() -> str:
    matrix = ROOT / "C_FEATURE_PARITY_MATRIX.csv"
    with matrix.open(newline="", encoding="utf-8-sig") as handle:
        rows = list(csv.DictReader(handle))
    status_counts = Counter(row["status"] for row in rows)
    grouped: dict[str, list[dict[str, str]]] = defaultdict(list)
    for row in rows:
        grouped[row["subsystem"]].append(row)
    lines = [
        "# Complete API and compatibility capability catalog",
        "",
        "This page projects every row of the repository's canonical C-to-Rust parity",
        "matrix into the feature encyclopedia. It covers both implemented operations and",
        "deliberately rejected legacy surfaces; a rejected C hook is a design decision,",
        "not a hidden missing feature.",
        "",
        f"**Coverage: {len(rows)} of {len(rows)} matrix rows** — "
        + ", ".join(f"{status}: {count}" for status, count in sorted(status_counts.items()))
        + ".",
        "",
        "> The exact current implementation and tests remain authoritative. The matrix",
        "> supplies operation-level intent and source citations; native/live qualification",
        "> is tracked separately.",
        "",
    ]
    for subsystem, subsystem_rows in grouped.items():
        rationale, good_for, strength = SUBSYSTEM_PROFILES.get(
            subsystem,
            ("Keep this capability family explicit.", "The named API operations.", "Typed Rust ownership makes the behavior reviewable."),
        )
        lines += [
            f"## {subsystem.replace('_', ' ').title()} ({len(subsystem_rows)})",
            "",
            f"**Rationale.** {rationale} **Good for.** {good_for} **Why.** {strength}",
            "",
            "| Capability | Status | Full behavior / design decision | Rust entrypoint | Legacy reference |",
            "|---|---|---|---|---|",
        ]
        for row in subsystem_rows:
            status = row["status"]
            badge = (
                '<span class="atlas-supported">Implemented</span>'
                if status == "Implemented"
                else '<span class="atlas-experimental">Rejected by design</span>'
            )
            rust_ref = f"`{md_escape(row['rust_reference'])}`" if row["rust_reference"] else "Not carried forward"
            c_ref = f"`{md_escape(row['c_reference'])}`" if row["c_reference"] else "—"
            lines.append(
                f"| `{md_escape(row['feature'])}` | {badge} | {md_escape(row['notes'])} | "
                f"{rust_ref} | {c_ref} |"
            )
        lines.append("")
    return "\n".join(lines)


def summary(packages: list[dict[str, Any]]) -> str:
    lines = [
        "# Summary",
        "",
        "- [Architecture Atlas](./index.md)",
        "- [Truth, maturity, and scope](./truth-and-scope.md)",
        "",
        "# Complete feature encyclopedia",
        "",
        "- [Feature encyclopedia: how to read it](./features/index.md)",
        "- [Personal cloud and account features](./features/personal-cloud.md)",
        "- [Transfers, sync, backup, and mounted drives](./features/sync-mount-transfer.md)",
        "- [Cryptography, secrets, and key custody](./features/crypto.md)",
        "- [Sharing, multi-user, and enterprise](./features/collaboration-enterprise.md)",
        "- [CLI, SDK, web, protocols, and automation](./features/interfaces-automation.md)",
        "- [Runtime, storage, resilience, and internal helpers](./features/runtime-internals.md)",
        "- [Platforms, packaging, and operations](./features/platform-operations.md)",
        "- [Verification, test, fuzz, and developer features](./features/verification-helpers.md)",
        "- [All package feature families](./generated/features/package-families.md)",
        "- [All API and compatibility capabilities](./generated/features/api-capabilities.md)",
        "- [All current CLI, IPC, SDK, and binary surfaces](./generated/features/current-surfaces.md)",
        "- [All Cargo feature flags](./generated/features/cargo-flags.md)",
        "- [All internal modules and helpers](./generated/features/source-units.md)",
        "",
        "# Architecture",
        "",
        "- [System overview](./system-overview.md)",
        "- [RemoteFs canonical boundary](./remote-fs.md)",
        "- [Entrypoints and public surfaces](./entrypoints.md)",
        "- [Request and data paths](./request-paths.md)",
        "- [State, transfers, and durability](./storage-durability.md)",
        "- [Security and trust boundaries](./security-boundaries.md)",
        "",
        "# Use and operations",
        "",
        "- [Standalone and library use](./standalone-library.md)",
        "- [Operations and platforms](./operations-platforms.md)",
        "- [Developer and extension guide](./developer-guide.md)",
        "- [Verification and evidence](./verification.md)",
        "",
        "# Generated source reference",
        "",
        "- [Workspace snapshot](./generated/snapshot.md)",
        "- [Workspace crate catalog](./generated/crates/index.md)",
    ]
    for package in sorted(packages, key=lambda item: item["name"]):
        lines.append(
            f"  - [`{package['name']}`](./generated/crates/{package_slug(package)}.md)"
        )
    lines += [
        "- [File inventory methodology](./inventory-methodology.md)",
        "- [Complete file inventory](./generated/inventory/index.md)",
    ]
    for area, title in AREA_TITLES.items():
        lines.append(f"  - [{title}](./generated/inventory/{area}.md)")
    return "\n".join(lines)


def inventory_index(files: list[str]) -> str:
    by_area: dict[str, list[str]] = defaultdict(list)
    for path in files:
        by_area[area_for(path)].append(path)
    lines = [
        "# Complete project file inventory",
        "",
        f"**{len(files)} tracked or unignored working-tree files** are covered.",
        "",
        "| Area | Files | Page |",
        "|---|---:|---|",
    ]
    for area, title in AREA_TITLES.items():
        lines.append(
            f"| {title} | {len(by_area.get(area, []))} | "
            f"[open](./{area}.md) |"
        )
    lines += [
        "",
        "The inventory includes vendored upstream files for exhaustiveness and "
        "labels them separately. Ignored build/runtime output is excluded.",
    ]
    return "\n".join(lines)


def main() -> int:
    metadata = cargo_metadata()
    packages = metadata["packages"]
    # Read the prior generated tree so the inventory is self-describing.
    files = git_files()
    GENERATED.mkdir(parents=True, exist_ok=True)

    write(GENERATED / "snapshot.md", workspace_snapshot(files, packages))
    write(
        GENERATED / "features/package-families.md",
        package_feature_catalog(packages),
    )
    write(
        GENERATED / "features/api-capabilities.md",
        api_capability_catalog(),
    )
    write(
        GENERATED / "features/current-surfaces.md",
        current_surface_catalog(packages),
    )
    write(
        GENERATED / "features/cargo-flags.md",
        cargo_feature_catalog(packages),
    )
    write(
        GENERATED / "features/source-units.md",
        source_unit_catalog(packages, files),
    )
    write(GENERATED / "crates/index.md", crate_index(packages, files))
    for package in packages:
        write(
            GENERATED / "crates" / f"{package_slug(package)}.md",
            package_page(package, files),
        )

    by_area: dict[str, list[str]] = defaultdict(list)
    for path in files:
        by_area[area_for(path)].append(path)
    write(GENERATED / "inventory/index.md", inventory_index(files))
    for area in AREA_TITLES:
        write(
            GENERATED / "inventory" / f"{area}.md",
            inventory_page(area, by_area.get(area, [])),
        )
    write(SRC / "SUMMARY.md", summary(packages))

    print(
        f"architecture atlas: generated {len(packages)} package pages and "
        f"inventoried {len(files)} files"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
