# IPC Protocol

> Authoritative sources:
> `crates/pcloud-ipc/src/{lib,protocol,methods,server,transport,auth}.rs`,
> the platform backends under `crates/pcloud-ipc/src/platform/`,
> and `crates/pcloud-daemon/src/dispatch.rs`. Anything on this page that
> contradicts those files is wrong; the code wins.

## Who this page is for

- **Beginner**: start with
  [One-minute overview](#one-minute-overview) and
  [Speaking IPC by hand with socat](#speaking-ipc-by-hand-with-socat).
- **Integrator (SDK / CLI author)**: focus on
  [Request catalogue](#request-catalogue),
  [Response envelope](#response-envelope-response), and
  [RequestEnvelope and distributed tracing](#requestenvelope-and-distributed-tracing).
- **FAANG-grade (platform / security)**: read the
  [Wire format](#wire-format) spec, the full
  [Security invariants](#security-invariants) section, and
  [Versioning & evolution](#versioning--evolution).

## One-minute overview

The pCloud daemon exposes a **local-only** request/response protocol
over:

- **Linux / *BSD / macOS**: an `AF_UNIX` SOCK_STREAM socket at
  `$runtime_dir/pcloud.sock`, file mode `0600`, parent directory
  `0700`.
- **Windows**: a named pipe with a SID-restricted DACL.

Every message is an **8-byte little-endian header followed by JSON**.
Per-request payloads are capped at **1 MiB** (checked *before* any
allocation). The caller's UID/SID is checked against the daemon's
owner on `accept()`; mismatches are refused with `Unauthorized`. There
is no TCP, HTTP, or remotely routable surface.

## Wire format

Every frame:

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                    payload_len (u32, LE)                      |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|     version (u16, LE)         |   message_type (u16, LE)      |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|               payload (payload_len bytes, JSON)               |
+---------------------------------------------------------------+
```

Constants (from `pcloud_ipc::protocol`):

| Constant | Value | Purpose |
|---|---|---|
| `IPC_PROTOCOL_VERSION` | `1` | Wire schema version. Bumped only on incompatible changes. |
| `MAX_IPC_PAYLOAD_LEN` | `1 * 1024 * 1024` (1 MiB) | Hard cap on `payload_len`. |
| `MessageKind::Request` | `1` | Client → daemon. |
| `MessageKind::Response` | `2` | Daemon → client. |
| `MessageKind::Event` | `3` | Reserved for future daemon-push notifications; decoders coerce unknown tags into `Event`. |

> **Not newline-delimited.** Previous drafts described the transport as
> NDJSON or CBOR — neither is accurate. The payload has always been
> `serde_json::{to_vec, from_slice}` wrapped in the fixed 8-byte
> length-prefixed header above.

## Size caps

`pcloud_ipc::server::MAX_REQUEST_BYTES = 1 * 1024 * 1024` mirrors
`MAX_IPC_PAYLOAD_LEN`.

**Why it matters** (audit P0.8 OOM cap). The 8-byte header carries an
attacker-controlled `u32 payload_len`. Without the cap, a malicious
peer could declare 4 GiB and force pre-allocation of a 4 GiB buffer
before a single body byte was read. The server:

1. Reads the header.
2. Validates `payload_len <= MAX_REQUEST_BYTES`, `version ==
   IPC_PROTOCOL_VERSION`.
3. **Only then** calls `Vec::with_capacity(payload_len)` and reads.
4. On cap violation, returns `IpcError::RequestTooLarge { declared,
   max }` and closes the connection **without writing a reply** — a
   reply would itself be a DoS amplification vector.

## Decoder errors (`ProtocolError`)

```rust
pub enum ProtocolError {
    TruncatedHeader,                                   // < 8 bytes for the header
    PayloadTooLarge,                                   // len > cap OR len != slice.len()
    VersionMismatch { expected: u16, actual: u16 },    // client speaks wrong wire schema
    Codec(serde_json::Error),                          // JSON malformed / wrong shape
}
```

| Variant | Fatal for connection? | Surface to peer? | Retryable? |
|---|---|---|---|
| `TruncatedHeader` | yes | no (stream is unrecoverable) | no |
| `PayloadTooLarge` | yes | no (amplification) | no |
| `VersionMismatch` | request-fatal, listener keeps serving | yes — `ResponseStatus::InvalidRequest` | only after client upgrade |
| `Codec` | request-fatal, listener keeps serving | yes — `ResponseStatus::InvalidRequest` | only after fixing the request body |

## `RequestEnvelope` and distributed tracing

Every client-to-daemon request is (by preference) wrapped in a typed
envelope that carries an **optional W3C `traceparent`** alongside the
inner `Request`:

```rust
#[non_exhaustive]
pub struct RequestEnvelope {
    pub request: Request,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traceparent: Option<String>,
}
```

Wire shape (JSON, omitting `traceparent` when `None`):

```json
{ "request": { "Plain": { "method": "GetUserInfo" } },
  "traceparent": "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01" }
```

**Why a wrapper and not a per-variant field.** The daemon already has
roughly 485 `Request::*` construction sites. Adding a `traceparent`
field to every variant would either require a 485-site patch or a
serde-skipped field everywhere; the wrapper buys us the same wire
semantics with one call-site change at the transport boundary. It also
keeps `Request` `#[non_exhaustive]` and lets us add new context fields
(baggage, sampling hints) later without a schema break.

**Backward compatibility.** `RequestEnvelope::try_from_wire(bytes)`
tries the envelope shape first; on a serde mismatch it falls back to
decoding a bare `Request` and wraps the result with `traceparent:
None`. Old clients that still emit bare requests keep working.

`traceparent` format follows RFC-9. The envelope layer treats it as an
opaque string and does **not** validate — downstream observability in
`pcloud_observability::tracing` parses and filters malformed values.

The daemon dispatch loop
(`crates/pcloud-daemon/src/dispatch.rs`) reads the envelope's
traceparent, opens a `pcloudd.dispatch` server span parented to it (or
a fresh root when `None`), and nests a `pcloudd.backend.<name>` span
around each handler. Every recorded attribute goes through the
`attr_redact` allow-list so secret-shaped keys never reach the
exporter.

## Request catalogue

`Request` is `#[non_exhaustive]`. The enumeration below groups
variants by responsibility; field shapes mirror `methods.rs`.

**`Plain { method: Method }`** — wrapper for every argumentless
method. `Method` is `#[non_exhaustive]` and includes (selected):

- Status / health: `GetStatus`, `GetHealth`, `Health`, `GetPending`,
  `GetSyncRoots`, `ListPublicLinks`, `ListUploadLinks`.
- Auth / session: `GetUserInfo`, `LoginBegin`, `Logout`,
  `SendTwoFactorSms`, `SendTwoFactorNotification`, `SubmitPassword`,
  `SubmitTwoFactorCode`, `SessionStatus`, `SetAuthPersistence`.
- Sync: `PauseSync`, `ResumeSync`.
- Crypto: `UnlockCrypto`, `LockCrypto`, `GetCryptoStatus`,
  `CryptoReset`, `GetCryptoPrivKeyFlags`,
  `SendCryptoChangeUserPrivate`.
- Shares / contacts: `ListIncomingShares`, `ListOutgoingShares`,
  `ListIncomingShareRequests`, `ListOutgoingShareRequests`,
  `ListContacts`, `ListMyTeams`.
- Notifications: `ListNotifications`.
- Introspection / diagnostics: `FileHistory`, `IntegrityStatus`.
- Lifecycle: `Shutdown`.

**Auth data-bearing** — transit-only secrets; see the audit H1 note at
the top of `methods.rs` for the rationale for using `String` over
`SecretString` on the wire:

- `PasswordSubmission { username, value }`
- `AuthTokenSubmission { value }`
- `TwoFactorCodeSubmission { value, trust_device, recovery_code }`
- `AuthPersistence { enabled }`

**Crypto**:

- `CryptoUnlock { password }`
- `CryptoSetup { password, hint }`
- `CryptoMkdir { name, parent_folder_id, local_folder_id }`
- `CryptoChangePassword { old_password, new_password, hint, code, flags }`
- `CryptoChangePasswordUnlocked { new_password, hint, code, flags }`

**Sync root lifecycle**:

- `SyncRootAdd { local_path, remote_path }`
- `SyncRootRemove { sync_id }`
- `SyncRootPause { sync_id }` / `SyncRootResume { sync_id }`
- `SyncRootChangeType { sync_id, ... }`
- `GetSyncSuggestions { ... }`
- `IsFolderSyncable { ... }`
- `RunLocalScan`

**Public links**:

- `ShowPublicLink { ... }`, `DeletePublicLink { ... }`,
  `DeletePublicLinkByCode { ... }`
- `CreateFilePublicLink { ... }`, `CreateFolderPublicLink { ... }`
- `ChangePublicLinkExpire`, `ChangePublicLinkPassword`,
  `ChangePublicLinkUpload`
- `CreateUploadLink`, `DeleteUploadLink`, `CreateTreePublicLink`
- `ListPublicLinkAccess`, `AddPublicLinkAccess`,
  `RemovePublicLinkAccess`
- `ListBookmarks`, `RemoveBookmark`, `ChangeBookmark`
- `SendPublink { code, mails, message }`

**Shares / teams / account**:

- `ShareFolder { ... }`, `CancelShareRequest`, `DeclineShareRequest`,
  `AcceptShareRequest`, `RemoveShare`, `ModifyShare`
- `AccountStopShare`, `AccountModifyShare`, `AccountTeamShare`

**Typed key/value store** (mirrors the legacy `setting` SQLite table):

- `ValueGet { key, kind: ValueKvKind }`
- `ValueSet { key, payload: ValueKvPayload }`
- `ValueHas { key, kind: ValueKvKind }`

  `ValueKvKind` = `Bool | Int | Uint | String`;
  `ValueKvPayload` mirrors with a concrete value.

**Mount / filesystem**:

- `Mount { path }`, `Unmount`, `MountForceUnmount { ... }`
- `CreateRemoteFolder { ... }`
- `GetFolderIdByPath { ... }`, `GetFolderFlags { ... }`,
  `GetFolderOwnerId { ... }`
- `FilesystemStatus { ... }`, `VerifyPath { ... }`
- `FileHistory { ... }`

**Audit / integrity**:

- `SessionStatus`, `MarkNotificationsRead { ... }`
- `AuditVerifyChain { ... }` with `AuditVerifyRange`
- `IntegrityRunOnce`, `IntegritySkip { ... }`
- `BackupSnapshot { ... }` using `SnapshotAction::{Create, Restore,
  Verify, Prune}`.

The complete, authoritative list lives in `methods.rs` — treat the
above as a hand-curated table of contents, not a closed set.

## Response envelope (`Response`)

```rust
#[derive(Serialize, Deserialize)]
pub struct Response {
    pub status:  ResponseStatus,
    pub message: String,
}
```

`message` is free-form text **and sometimes a JSON-serialized payload**
(for example, `Method::Health` returns Prometheus text;
`Method::SessionStatus` returns a JSON `SessionStatusPayload`;
`Request::GetFolderFlags` returns a `key=value` string). Callers MUST
branch on `status` before attempting to parse `message`.

**Security invariant**: `message` never carries secret material. Error
messages are pre-screened on the daemon side so they are safe to
surface verbatim to operators (see the H1 audit note in `methods.rs`).

### `ResponseStatus`

`#[non_exhaustive]`. HTTP-style semantics in the rightmost column are
an analogy, not a guarantee — there is no HTTP transport.

| Variant | Wire shape | Meaning | Recovery | Closest HTTP analogue |
|---|---|---|---|---|
| `Ok` | `"Ok"` | Success. Payload (if any) is in `message`. | None — callers proceed. | 200 |
| `InvalidRequest` | `"InvalidRequest"` | Malformed request, unknown variant, missing argument, version mismatch, JSON decode failure. | Not retryable without changing the body. Surface `message` to operator. | 400 |
| `Unauthorized` | `"Unauthorized"` | Peer UID/SID mismatch, no authenticated session, or missing capability. | Not retryable without re-authenticating or running as the daemon owner. | 401 / 403 |
| `Conflict` | `"Conflict"` | State conflict (already mounted, duplicate sync root, already logged in, crypto already unlocked). | Not retryable without reconciling the conflicting state. | 409 |
| `Unavailable` | `"Unavailable"` | Subsystem not available (crypto not set up, network unreachable, feature compiled out, FUSE runtime not started). | Transient (retry with backoff) or permanent (fatal). | 503 |
| `InternalError` | `"InternalError"` | Daemon-side failure that does not fit a stricter class — DB error, panic-guard path, unexpected I/O. | Opaque; may be transient or persistent. | 500 |
| `PolicyViolation { kind }` | `{"PolicyViolation":{"kind":"<name>"}}` | Operation refused by a declarative policy. Stable `kind` discriminators today: `"data_residency"`. | Not retryable as-is; adjust policy or target a permitted resource. | 451 / 403 |

Unit variants serialize as bare JSON strings; the data-bearing
`PolicyViolation` as an object. Clients MUST treat unknown `kind`
values as "generic policy refusal" rather than erroring — new kinds
may be introduced in minor releases.

## Security invariants

These are load-bearing and enforced by the IPC layer rather than each
call site. Regressing any of them is a security bug.

### Owner-only local socket (Unix)

- Socket file created with mode `0600` by
  `pcloud_ipc::transport::BoundIpcServer`.
- Parent runtime directory is `0700` (enforced by
  `runtime.socket_dir_mode` and the daemon's on-boot
  `chmod`).
- Stale socket files are unlinked on bind so a crashed daemon cannot
  leave an unbound handle.

### Peer identity check on every accept

- **Linux**: `getsockopt(SO_PEERCRED)` returning
  `libc::ucred { pid, uid, gid }`.
- **FreeBSD / OpenBSD / NetBSD / macOS**: `getpeereid(3)`.
- **Windows**: named pipe SID DACL + `GetNamedPipeClientProcessId`
  followed by TokenUser SID comparison against the daemon's SID. The
  Windows platform backend is scaffolded; production Windows support
  is not yet claimed.

The daemon's owner UID/SID is fixed at boot. `IpcServer::authorize_peer`
rejects every other caller before any dispatch happens. Rejection does
not write a reply — the FD is closed immediately.

### 1 MiB cap before allocation

Already covered under [Size caps](#size-caps). The check precedes any
`Vec::with_capacity` or `vec![0u8; ...]` sized by client-controlled
bytes.

### No secret leakage in `Response.message`

Audit H1. Secret-bearing `Request` fields (passwords, tokens, TFA
codes) are serialized as `String` for transit and destructured into
`SecretString` on the daemon side before any long-lived storage. The
corresponding `Response.message` contents are pre-screened so an
operator can log them verbatim without leaking credentials.

### No remote surface

There is no TCP listener, no HTTP endpoint, no mDNS. The IPC path is
the only wire the daemon speaks outside of the outbound API transport,
and it is always a local filesystem handle gated by OS-level
permissions.

## Connection lifecycle

1. Client opens the local socket/pipe.
2. Server `accept`s and immediately recovers `PeerIdentity { uid, pid }`
   (or the Windows SID equivalent).
3. Owner check. Reject → close the FD, no reply.
4. Server reads exactly 8 bytes (the header).
5. Server validates `version == 1`, computes `message_type`, and
   checks `payload_len <= MAX_REQUEST_BYTES`.
6. Server allocates + reads the JSON payload.
7. Server calls `RequestEnvelope::try_from_wire(bytes)` — envelope
   shape first, bare-`Request` fallback for pre-envelope clients.
8. Dispatch via `pcloud_daemon::dispatch::dispatch(...)`. A
   `pcloudd.dispatch` span is opened, parented to the envelope's
   `traceparent` when present.
9. Handler produces a typed `Response`. `encode_response` frames it
   with the same 8-byte header (kind = `Response`).
10. Connection is one-shot per request in the current retained surface.
    A future streaming extension will use `MessageKind::Event`.

## Versioning & evolution

- `IPC_PROTOCOL_VERSION` is bumped **only on incompatible wire
  changes**. Additive changes (new `Request` variants, new `Method`
  values, new `RequestEnvelope` fields, new `ResponseStatus` variants)
  are released at the same version.
- `Request`, `Method`, `ResponseStatus`, and `RequestEnvelope` are all
  `#[non_exhaustive]`. Clients MUST treat every enum as open and fall
  back gracefully for unknown discriminants.
- `traceparent` is already treated as opaque at the envelope layer,
  and the `RequestEnvelope` struct is marked `#[non_exhaustive]` so
  future context fields (baggage, sampling hints, backend identity)
  don't break existing clients.
- The wire JSON for unit variants stays bare-string; data-bearing
  variants stay single-key objects. This is the contract
  `ResponseStatus::PolicyViolation` was added under and is the
  template for any future data-bearing status.

## Speaking IPC by hand with `socat`

Useful for operator debugging and smoke tests — the daemon speaks the
exact same wire regardless of caller. Frame `{ "Plain": { "method":
"GetUserInfo" } }` and pipe it in:

```bash
# 1. Serialize the body.
body='{"request":{"Plain":{"method":"GetUserInfo"}}}'

# 2. Compute payload_len (u32 LE), version (1, u16 LE), message_type
#    (Request = 1, u16 LE).
len=$(printf '%s' "$body" | wc -c)
python3 -c "
import struct, sys
body = b'''$body'''
hdr = struct.pack('<IHH', len(body), 1, 1)
sys.stdout.buffer.write(hdr + body)
" | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/pcloud/pcloud-rs/pcloud.sock \
  | python3 -c "
import struct, sys
raw = sys.stdin.buffer.read()
payload_len, version, kind = struct.unpack('<IHH', raw[:8])
print('header:', payload_len, version, kind)
print('body:', raw[8:8+payload_len].decode('utf-8'))
"
```

Expected output (assuming an authenticated session, `kind == 2` for
`Response`):

```
header: 123 1 2
body: {"status":"Ok","message":"{\"userid\":...,\"email\":\"...\"}"}
```

Common failure modes:

- `Connection refused` → daemon is not running, or you targeted the
  wrong `runtime_dir`. Check `pcloudc status` or inspect
  `$runtime_dir/pcloud.sock`.
- `permission denied` → you are not the daemon owner; the socket is
  `0600`, and the peer UID check rejects unrelated UIDs without
  writing a reply.
- Reading 0 bytes back after a valid send → you hit
  `PayloadTooLarge` or `TruncatedHeader`; the daemon closes the FD
  without responding by design.

## Testability hooks

- `PCLOUD_LIVE_E2E=1` opts into live-API end-to-end tests that
  exercise real frames against a staging account.
- `PCLOUD_FUSE_TEST=1` gates FUSE integration tests that drive mount
  lifecycle IPC.
- `PCLOUD_CHAOS=1` injects framed read/write faults in test/debug
  builds; never honoured in release.

## See also

- [CLI Reference](./cli.md) — which CLI subcommands map to which
  `Request`.
- [Exit Codes](./exit-codes.md) — `ResponseStatus` → CLI exit-code
  ABI.
- [enterprise/tracing](../enterprise/tracing.md) — span hierarchy,
  sampling policy, and `attr_redact` allow-list.
- `crates/pcloud-ipc/src/methods.rs` — authoritative `Method`,
  `Request`, `Response`, `RequestEnvelope`, `ResponseStatus`
  definitions.
- `crates/pcloud-ipc/src/protocol.rs` — framing, constants, encoders /
  decoders.
- `crates/pcloud-ipc/src/server.rs`,
  `crates/pcloud-ipc/src/transport.rs`,
  `crates/pcloud-ipc/src/platform/{linux,unix,windows}.rs` — transport
  and peer-identity plumbing.
