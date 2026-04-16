# ARCHITECTURE

Authoritative architecture document for the Rust rewrite under ``.
This document describes the system **as implemented today**. It does **not**
claim full parity with the legacy C client. See `STATUS.md` for the
current parity tally (regenerated from `C_FEATURE_PARITY_MATRIX.csv`)
and `C_FEATURE_PARITY_REVIEW.md` for the narrative. Outstanding work
is tracked under `bd-1du`, `bd-1du.4`, and `bd-1du.10`.

## Crate map

The workspace is split into the following crates (under
`crates/<name>`):

| Crate | Role |
|-------|------|
| `pcloud-secret` | `SecretString` / `SecretBytes` wrappers with `Drop`-zeroize and `Debug` redaction. Foundation for all secret handling. |
| `pcloud-model` | Pure data types: auth, crypto, ids, health, conflict. No I/O. |
| `pcloud-config` | Profiles, feature flags, environment selection, limits, extension policy. |
| `pcloud-observability` | Structured logging, audit events, metrics, health. |
| `pcloud-store` | SQLite persistence, schema migrations, repositories, transactions. |
| `pcloud-cache` | Page cache, checksum cache, staging area, eviction. |
| `pcloud-auth` | Auth state machine, TFA orchestrator, auth events. |
| `pcloud-proto` | Typed pCloud protocol clients (auth, transfer, folder, sync, public links, shares, account, backup, crypto) + transport. |
| `pcloud-ipc` | Local IPC: `Method`/`Request`/`Response` enums, transport, server, client, peer auth. |
| `pcloud-crypto` | Active crypto primitives: key derivation, `CryptoShell`, sector-level AES-GCM, share temppass. |
| `pcloud-fs` | Filesystem shell: inode, journal, mount service scaffolding, FUSE adapter (not yet fully live — `bd-1du.4`). |
| `pcloud-engine` | Diff poller, local scan, conflict resolver, planner, fs events. |
| `pcloud-plugin-api` | Plugin extension points. |
| `pcloud-p2p` | LAN peer discovery/policy/transfer scaffolding. |
| `pcloud-daemon` | Composition root. Bootstrap, runtime, dispatch, per-subsystem backends, auth vault, mount discovery, serve loop. |
| `pcloud-sdk` | Embeddable in-process SDK wrapping the daemon (`EmbeddedDaemon`). |
| `pcloud-cli` | CLI front-end speaking to the daemon over IPC. |
| `pcloud-live-e2e` | Live end-to-end test harness. |

## High-level data flow

```
+---------+     IPC (uds, 0600)     +----------+     HTTPS      +--------+
|  CLI    | <---------------------> |  Daemon  | <------------> | pCloud |
+---------+                         |          |                +--------+
                                    |  runtime |
+---------+   in-proc (SDK)         |   +----+ |     SQLite
|  App    | <---------------------> |   |repo| | <-----------> local store
+---------+                         |   +----+ |
                                    +----------+
                                         |
                                         v
                                    pcloud-fs (WIP: FUSE)
```

## Daemon runtime layout

```
bootstrap.rs
    |-- loads ConfigProfile (pcloud-config)
    |-- opens Store (pcloud-store, SQLite)
    |-- opens AuthVault (0600 file, 0700 dir)
    |-- initializes RuntimeShell
    |        |-- AuthRuntime        (pcloud-auth + pcloud-proto::auth_api)
    |        |-- TransferRuntime    (pcloud-proto::transfer_api, http_download)
    |        |-- SyncRuntime        (pcloud-engine, sync_backend)
    |        |-- PublicLinkRuntime  (pcloud-proto::public_links_api)
    |        |-- SharesRuntime      (pcloud-proto::shares_api)   [partial]
    |        |-- AccountRuntime     (pcloud-proto::account_api)
    |        |-- BackupRuntime      (pcloud-proto::backup_api)   [partial]
    |        |-- CryptoRuntime      (pcloud-crypto, gated)       [not fully active]
    |        '-- MountRuntime       (pcloud-fs)                  [not live]
    |-- starts IPC server (pcloud-ipc::server)
    '-- dispatch loop routes Request -> Backend -> Response
```

## Threading model

- Tokio multi-threaded runtime owned by `pcloud-daemon`.
- IPC server: one accept task; per-connection task reads framed requests
  and hands off to `dispatch::handle`.
- Backends are `Send + Sync` and primarily async; CPU-bound work (crypto
  sector ops, hashing) runs on `spawn_blocking`.
- `RuntimeShell` holds `Arc<…>` handles; no global mutable state.
- The engine (`pcloud-engine`) owns its own scheduler tasks for diff polling
  and local scan; task handles are stored on the runtime for shutdown.
- Shutdown is cooperative via a `CancellationToken` + `finalize()` in
  `bootstrap.rs`.

## IPC protocol reference

Transport: Unix domain socket under the owner-only runtime dir. File mode
`0600`, parent dir `0700`. Peer UID check enforced on accept.

Framing: length-prefixed CBOR (see `pcloud-ipc/src/transport.rs`).

Core types (`pcloud-ipc/src/methods.rs`):

- `Method` — enum of callable operations (auth, sync, transfer, public
  links, crypto, values, settings, shares, backup, shutdown).
- `Request` — tagged envelope carrying method-specific payloads.
- `Response` — `{ status: ResponseStatus, payload }` where
  `ResponseStatus` is `Ok | Error(kind) | Unauthorized | …`.
- `ValueKvKind` / `ValueKvPayload` — typed key/value persistence surface
  (stricter than the legacy C `get_*_value` APIs — cross-kind reads return
  `SettingTypeMismatch` instead of a zero sentinel).

See `API-REFERENCE.md` for per-method mapping to pCloud protocol calls and
parity matrix rows.

## Auth lifecycle

```
Client ──Login(user, SecretString pw)──► Daemon
                                          │
                                          ├─ AuthApi::userinfo(getauth=1)
                                          │    └─ HTTPS POST /userinfo
                                          │
              ◄──TfaRequired(token, hints)┤   (if TFA)
  SubmitTwoFactorCode | Recovery | SMS/Push
                                          │
              ◄─────────Authenticated─────┤
                                          │
                           AuthVault.store(token)   [opt-in, 0600]
                           RuntimeShell.set_user(uid, token-as-SecretString)
```

- Password never persisted (intentional divergence from C — see
  SECURITY-MODEL.md).
- Token persistence is explicit opt-in; vault is owner-only.
- `logout()` clears in-memory state and persisted vault entry.
- Live-verified flows: password, token, TFA code, recovery code, TFA SMS
  resend, TFA notification resend, userinfo.

## Crypto lifecycle (partial — `bd-1du.5`)

```
Setup:  CryptoShell::setup(password) ──► derives master key (Argon2 KEK,
                                        AES-256-GCM wrap, HMAC-SHA256
                                        fingerprint). Master key never
                                        persisted in plaintext.

Unlock: CryptoShell::start(password) ──► constant-time fingerprint check
                                        (subtle::ConstantTimeEq); master
                                        key held as SecretBytes.

File ops: per-file key = HMAC-SHA256(master, random seed);
          sector seal = AES-256-GCM(file_key, nonce=12B, aad=sector_idx,
                                    tag=16B). Master never seals content.

Lock:   CryptoShell::stop() ──► zeroize-on-drop SecretBytes.
```

Daemon currently gates the active crypto path behind config; FUSE
integration is pending. See `bd-1du.5`.

## Sync lifecycle (partial — `bd-1du.3`)

```
Add:    sync-add <local> <remote>
          ├─ canonicalize local path
          ├─ reject duplicates / nested roots / ignored mounts
          ├─ AuthApi.listfolder(remote) validation
          ├─ persist to store (schema v6 includes SyncType, paused flag)
          └─ enqueue engine scheduler

Run:    diff_poller --► planner --► conflict_resolver --► transfer_backend
                         ▲
                local_scan (fs_events)

Pause:  SyncRootPause — stops per-root scheduling, persists paused flag.
Remove: evicts scheduler, uploads/downloads; drops staged cache prefix.
```

Not yet at full C parity: suggestions helpers, is-folder-syncable helper
family, deeper backend-coupled lifecycle. See `bd-1du.3`.

## Transfer lifecycle

```
Upload:  upload_create(folderid, name) ─► uploadid
         upload_write(uploadid, offset, bytes) × N
         upload_save(uploadid, …)           ─► file metadata
         (SDK helpers: upload_data / upload_data_as / upload_file /
          upload_file_as)

Download: getfilelink(fileid)  ─► signed host + path
          http_download execute (streaming, checksum verify)
          cache staging (pcloud-cache::staging)
```

All protocol calls in `pcloud-proto/src/transfer_api.rs`.
Execution in `pcloud-daemon/src/transfer_backend.rs`.

## Public link lifecycle

```
Create:  getfilepublink | getfolderpublink | gettreepublink (id-based)
Modify:  changepublink (expire, password, maxdownloads, maxtraffic,
                        upload policy)
List:    listpublinks / listuploadlinks
Delete:  deletepublink / deleteuploadlink
```

Still missing: C path-resolver shape for some tree/public-tree helpers
(`bd-1du.9`).

## Unimplemented / staged subsystems

- Mounted-drive / FUSE runtime: scaffolded, not live (`bd-1du.4`).
- Shares / business / teams: only partial (`bd-1du.7`).
- Backup / device surface: account slice done, backup lifecycle partial
  (`bd-1du.8`).
