# Request Lifecycle

This page is a teaching-grade walkthrough for a new contributor. We pick one
concrete command:

```
pcloudc sync add /local /remote
```

and trace every hop from the user's terminal down to `binapi.pcloud.com` and
back. The goal is that after reading this page you can open any of the files
mentioned and know why that file is on the request path, not just that it is.

All file paths are relative to ``. All line numbers reflect the tree
at the time of writing; use them as starting points, not as contracts.

## Sequence diagram

```
  user shell        pcloud-cli (client process)                 pcloud-daemon (server process)            pCloud API
  ----------        ---------------------------                 ----------------------------------        ----------
   |  argv                |                                            |                                    |
   |--------------------->| main()                                     |                                    |
   |                      | GlobalFlags::extract                       |                                    |
   |                      | app::parse_command                         |                                    |
   |                      | Command::SyncAdd + inputs                  |                                    |
   |                      | build Request::SyncRootAdd { local, remote}|                                    |
   |                      | IpcClient::send                            |                                    |
   |                      | protocol::encode_request (8B hdr + JSON)   |                                    |
   |                      |--- UnixStream ---------------------------->| accept()                           |
   |                      |                                            | peer_identity (SO_PEERCRED)        |
   |                      |                                            | read_framed_request (size-gated)   |
   |                      |                                            | decode_request -> Request enum     |
   |                      |                                            | Runtime::handle_request            |
   |                      |                                            |   catch_unwind { dispatch }        |
   |                      |                                            |     add_sync_root                  |
   |                      |                                            |       canonicalize local           |
   |                      |                                            |       sync_runtime.validate_remote |
   |                      |                                            |         listfolder (binary proto)  |
   |                      |                                            |-----------------------------TLS--->| binapi.pcloud.com:8398
   |                      |                                            |<------ result metadata ------------|
   |                      |                                            |       persist SyncRootRecord       |
   |                      |                                            | Response { Ok, "sync added ..." }  |
   |                      |<--- framed Response ------------------------| write_response                    |
   |                      | parse_response                             |                                    |
   |  exit code <---------| map Response -> ExitCode                   |                                    |
```

## 1. The CLI process starts

The binary entry point is `fn main` in
`crates/pcloud-cli/src/main.rs:20`. It collects `argv`, hands it to `run`, and
propagates the numeric exit code. `run` itself is the interesting function
(`crates/pcloud-cli/src/main.rs:78`), because it is the one exercised by unit
tests.

The first job is to split global flags (`-q`, `-v`, `--json`, `--output`) from
the subcommand. That happens in
`GlobalFlags::extract` at `crates/pcloud-cli/src/globals.rs:145`. `extract`
returns the flags plus a "reduced" argv with those global tokens removed.

## 2. Command parsing

The reduced argv is fed into `app::parse_command` at
`crates/pcloud-cli/src/app.rs:1009`. Parsing is a hand-written recursive-descent
mapper rather than a derive macro so we can support legacy aliases like
`s add`, `sync-add`, and abbreviations like `st` without a framework in the way.
For our input `sync add`, it takes the first-token branch in the "sync" group
(`app.rs:513`) and returns `Command::SyncAdd`.

`Command::SyncAdd` is a zero-payload variant. The two positional arguments
(`/local` and `/remote`) are captured separately by `parse_inputs_for_command`
(see the test at `app.rs:2091` for the canonical fixture).

## 3. Building the typed IPC request

Once the command and its inputs are known, `app.rs:1088` selects the `SyncAdd`
arm and constructs the IPC-level request. The concrete enum variant is
`Request::SyncRootAdd { local_path, remote_path }`, declared in
`crates/pcloud-ipc/src/methods.rs:150`.

Two things to notice here:

1. The variant carries plain paths, not secrets. Any secret fields across the
   whole `Request` enum (passwords, tokens, TFA codes) are wrapped in
   `SecretString` so they zeroize on drop. A sync-root add carries no secret
   payload.
2. The variant name (`SyncRootAdd`) intentionally diverges from the CLI name
   (`SyncAdd`). The CLI names the user gesture; the IPC names the daemon
   operation. Keep that distinction when you add new methods.

## 4. Encoding and sending over the local socket

The CLI hands the typed `Request` to `IpcClient::send` at
`crates/pcloud-ipc/src/transport.rs:143`. `send` delegates the framing to
`protocol::encode_request` at `crates/pcloud-ipc/src/protocol.rs:74`. The wire
format is an 8-byte little-endian header
(`u32 payload_len | u16 version | u16 kind`) followed by the JSON-serialized
request body.

Caps and invariants enforced on the way out:

- `MAX_IPC_PAYLOAD_LEN` = 1 MiB (`protocol.rs:20`). Oversized requests fail
  before the socket is even touched.
- `IPC_PROTOCOL_VERSION` is pinned at 1 (`protocol.rs:12`). An old client
  talking to a new daemon gets a clean `VersionMismatch`.

The socket path itself comes from the bootstrap/runtime layer; it lives under
a `0700` runtime directory and the socket is chmodded `0600`
(`transport.rs:132`). That is our first line of defence: only the same UID can
even try to connect.

## 5. Server accept and peer authentication

On the daemon side, `pcloud-ipc/src/transport.rs` (search `fn run` in
`IpcServer`) accepts the `UnixStream`, then immediately calls
`peer_identity` at `transport.rs:233`. That function uses `SO_PEERCRED`
(`transport.rs:248`) via `getsockopt` to read the peer's `uid/gid/pid` out of
the kernel.

If the UID does not match the daemon's own UID, the connection is dropped
before any bytes are decoded. This is a hard authentication boundary: the
daemon does not trust that "it's a Unix socket" is enough, even with the
`0600` permissions, because filesystem permissions can be surprising on
shared mounts.

After the peer check, `read_framed_request` (`transport.rs:159`) reads the
8-byte header, checks the declared length against `MAX_REQUEST_BYTES`, then
reads exactly that many body bytes. A too-large declaration aborts the
connection without sending a response (`transport.rs:192`), because at that
point the stream is not in a framed-recoverable state.

## 6. Dispatch with a panic guard

The decoded `Request` lands in `Runtime::handle_request` at
`crates/pcloud-daemon/src/runtime.rs:118`. This function wraps the actual
dispatch in `std::panic::catch_unwind` (`runtime.rs:130`). If the dispatch
panics, the guard converts it into a `Response { InternalError, ... }` instead
of letting the daemon process die. The `metrics` feature increments a panic
counter so the event is visible in Prometheus.

Inside the guard, `handle_request_dispatch` (`runtime.rs:160`) is a large
`match` on `Request`. Our variant takes the arm at `runtime.rs:342`:

```rust
Request::SyncRootAdd { local_path, remote_path }
    => self.add_sync_root(local_path, remote_path),
```

## 7. Backend work

`Runtime::add_sync_root` starts at `runtime.rs:2813`. The interesting parts in
order:

1. Reject empty paths (`runtime.rs:2814`). Invalid input never leaves this
   layer.
2. Canonicalize the local path (`runtime.rs:2820`). We store the canonical
   form so a user adding the same directory via a symlink does not register
   two roots.
3. Reject duplicate or nested local roots (`runtime.rs:2832`). This is one of
   the places the Rust path is stricter than the C reference: we refuse to
   register `/a/b` when `/a` is already a root.
4. Require an authenticated session (`runtime.rs:2845`). The auth token is
   cloned out of `SecretString` for the duration of the backend call.
5. Validate the remote folder (`runtime.rs:2860`) by calling
   `sync_runtime.validate_remote_folder`, defined at
   `crates/pcloud-daemon/src/sync_backend.rs:375`. This is the hop that
   actually leaves the daemon.

## 8. Protocol call: `listfolder`

`validate_remote_folder` asks the API whether the remote path exists and is a
folder. It builds a `ListFolderByPathRequest` and invokes the `listfolder`
method defined in `crates/pcloud-proto/src/methods/folder.rs:12`. The binary
transport layer that actually writes the command and its params on the wire
lives in `crates/pcloud-proto/src/binary_api.rs` (command assembly) plus
`crates/pcloud-proto/src/folder_api.rs:8` (typed request/response wrappers).

## 9. TLS to `binapi.pcloud.com`

The underlying socket is a TLS-wrapped TCP stream set up in
`crates/pcloud-proto/src/transport.rs`. `connect_socket` at `transport.rs:116`
opens the raw TCP connection with a bounded `connect_timeout`
(`transport.rs:27`); `StreamOwned::new` at `transport.rs:161` wraps it in
`rustls`. Production configs reject downgrade away from TLS, which is enforced
at config load time and is one of the non-negotiable security rules in
`CLAUDE.md`.

## 10. Response path

The listfolder reply flows back the same way in reverse:

- `pcloud-proto` decodes the binary response and returns a `Result<_, ProtoError>`.
- `sync_backend.rs` maps protocol errors into its own `ValidateRemoteError`.
- `runtime.rs:2865` converts that into a `Response { Conflict, "remote sync
  root validation failed: ..." }` on failure, or on success continues to
  allocate a new `SyncId` and persist a `SyncRootRecord` (`runtime.rs:2872`).
- `handle_request` frames the `Response` and `write_response`
  (`pcloud-ipc/src/transport.rs:221`) writes it back on the same stream.
- `IpcClient::send` (`transport.rs:153`) reads the framed response to end,
  calls `parse_response`, and returns.
- The CLI maps `ResponseStatus` to a process exit code using the table in
  `crates/pcloud-cli/src/exit_code.rs`; see the rendered table at
  `reference/exit-codes.md`.

Each hop has its own error type. That is deliberate: protocol errors don't
leak into IPC errors, IPC errors don't leak into CLI exit codes, and each
layer can log at the level where the information is useful. When you debug a
failure, pick the layer that matches your symptom and grep for that error
type; do not assume a single `Error` enum spans the whole stack.

## Where to add a new method

Adding a new daemon-mediated command is a fixed six-file change:

1. `crates/pcloud-ipc/src/methods.rs` - add the `Request` variant and any
   `Response` payload.
2. `crates/pcloud-cli/src/app.rs` - add a `Command` variant, parse it in
   `parse_command`, and build the `Request` in the `run_command` match.
3. `crates/pcloud-daemon/src/runtime.rs` - add the arm in
   `handle_request_dispatch` and a private method on `Runtime`.
4. `crates/pcloud-daemon/src/<area>_backend.rs` - implement the business logic
   against `pcloud-proto` and/or `pcloud-store`.
5. `crates/pcloud-proto/src/methods/<area>.rs` (+ `<area>_api.rs`) - if a new
   API call is needed, declare the request/response types and the binary
   command name.
6. `crates/pcloud-sdk-public/src/lib.rs` - expose a focused SDK helper with
   SDK-owned types when the operation belongs in the stable remote-drive
   contract. Broad first-party compatibility helpers belong in
   `crates/pcloud-sdk/src/lib.rs` (`pcloud-embedded-sdk`).

Also write an integration test under
`crates/pcloud-daemon/tests/` or a `#[cfg(test)]` block next to the backend,
and update `C_FEATURE_PARITY_MATRIX.csv` if the new method is parity-relevant.

## Gotchas

**Peer authentication is platform-specific.** Linux uses `SO_PEERCRED`, BSD
and macOS use native peer-credential APIs, Solaris-family targets use
`getpeerucred(3)`, and Windows verifies the named-pipe client's token SID.
Never replace these checks with socket/file permissions alone.

**The panic guard is unconditional.** The `catch_unwind` wrapper in
`runtime.rs:130` runs in release builds as well as test builds. It exists so a
buggy dispatch arm cannot take the whole daemon down, which means you must
not rely on a panic to tear down shared state: assume the process keeps
running after a panic, and make sure any partially-updated state is either
rolled back or documented.

**`catch_unwind` does not catch all aborts.** It catches panics. It does not
catch stack overflow via `abort`, it does not catch `abort()` from FFI, and it
does not catch a panic that hits `panic = "abort"` if we ever switch. The
panic hook in the metrics path (`runtime.rs:152`) folds background-thread
panics into a gauge so you still see them in Prometheus; never silence that
hook.

**JSON is the current IPC body encoding.** The doc comment on the framing
mentions CBOR in passing, but the shipping implementation uses
`serde_json::to_vec` / `serde_json::from_slice` (`protocol.rs:75`). If you
switch encodings, bump `IPC_PROTOCOL_VERSION` so old clients fail loudly.

**Socket mode vs directory mode.** Both must stay restrictive. The runtime
directory is `0700`, the socket is `0600`. Weakening either one invalidates
the `SO_PEERCRED` argument above, because a world-writable parent directory
lets an attacker stage a replacement socket.

## Runnable example

For a full end-to-end demo you can run against a real account, see
`crates/pcloud-sdk/examples/login_and_list.rs` from the internal
`pcloud-embedded-sdk`. It exercises the same
CLI -> Request -> daemon -> `pcloud-proto` -> TLS path end-to-end against
`binapi.pcloud.com`, minus the CLI frontend, and is the fastest way to see
each layer produce a real log line.

## The `pcloud-backends` split (P6.1)

The dispatch arms of `pcloud-daemon` used to own both the routing logic
and the per-feature backend implementations. As the feature surface grew
(auth, transfers, sync, public links, shares, backups, crypto, account,
folder) that coupling made it hard to unit-test a backend without pulling
the full daemon binary crate, and it made reuse across native compositions
and the internal embedded compatibility API unnecessarily difficult.

The `pcloud-backends` crate exists to break that coupling. It contains:

- the `Backend` trait that every per-feature backend implements;
- request routing types (`BackendRequest`, `BackendResponse`, dispatch
  keys) that are independent of the IPC wire format;
- backend composition utilities used by both `pcloud-daemon` (for the
  daemon dispatch path) and `pcloud-embedded-sdk` (for the embedded in-process
  path);
- shared test doubles and fixtures so each backend can be exercised in
  isolation.

The practical consequence is that a backend no longer needs to know
whether it is being driven by a real IPC socket, a mock IPC harness, or
an in-process compatibility-SDK call. `pcloud-daemon::runtime::dispatch` becomes a
pure translator between `pcloud-ipc::Request` and
`pcloud-backends::BackendRequest`; the embedded SDK does the same translation
on behalf of in-process callers. Neither path can short-circuit the other
because they both go through the same trait surface.

Invariants to preserve when touching this split:

- a backend must not depend on `pcloud-daemon` directly. If it needs a
  runtime service (store, crypto, observability), it takes it as a trait
  bound or a handle passed at construction time.
- the daemon dispatch arm stays **thin**. Any non-trivial logic belongs
  in the backend, not in the translation layer.
- the `catch_unwind` boundary lives on the daemon side of the
  translation. Backends assume panics propagate; the daemon catches them.
- backend tests live in `pcloud-backends` or the backend's own crate,
  not in `pcloud-daemon`. If you find yourself writing a backend test in
  the daemon crate, you have probably skipped an abstraction.

## Windows and macOS peer-check variations

The lifecycle above describes the Linux path. The other supported
platforms use different peer-authentication primitives for the same
trust boundary. The `PlatformIpc` trait hides these behind one interface,
but the details matter when you are debugging a permission denial or
writing a new platform backend.

```
                +---------------------------------------+
                |  client connects to local IPC endpoint|
                +------------------+--------------------+
                                   |
            +----------------------+----------------------+
            |                      |                      |
            v                      v                      v
    +---------------+      +---------------+      +-----------------+
    |    Linux      |      |    macOS      |      |    Windows      |
    |               |      |               |      |                 |
    | Unix socket   |      | Unix socket   |      | Named pipe      |
    | 0600 on 0700  |      | 0600 on 0700  |      | \\.\pipe\pcloudd|
    | dir           |      | dir           |      | with explicit   |
    |               |      |               |      | DACL            |
    |      |        |      |      |        |      |        |        |
    |      v        |      |      v        |      |        v        |
    | getsockopt    |      | getsockopt    |      | GetNamedPipe-   |
    | SO_PEERCRED   |      | LOCAL_PEERCRED|      | ClientProcessId |
    | -> ucred      |      | -> xucred     |      | -> PID          |
    |      |        |      |      |        |      |        |        |
    |      v        |      |      v        |      |        v        |
    | compare uid   |      | compare uid   |      | OpenProcessToken|
    | to geteuid()  |      | to geteuid()  |      | + compare SID   |
    |               |      |               |      | to daemon SID   |
    +-------+-------+      +-------+-------+      +--------+--------+
            |                      |                       |
            +----------------------+-----------------------+
                                   |
                                   v
                   PlatformIpc::accept() returns a
                   peer-verified framed channel
                   to runtime::dispatch
```

Linux uses `SO_PEERCRED` on the accepted Unix socket, which returns a
`struct ucred` (uid, gid, pid) captured at connect time. The daemon
compares the returned uid to `geteuid()` and rejects any mismatch. The
socket is `0600` on a `0700` parent directory so the kernel's DAC check
already filters obvious abuse; `SO_PEERCRED` is the defence-in-depth
layer on top.

macOS and the BSD family use `getpeereid(3)`. The comparison is the same
(peer euid versus daemon euid) and the file permissions are the same.
illumos/Solaris use `getpeerucred(3)`, which also yields the peer PID.

Windows uses a named pipe at `\\.\pipe\pcloudd-<session>` created with
an explicit DACL that grants `FILE_ALL_ACCESS` only to the daemon's SID
and the owning user's SID. On accept, the daemon calls
`GetNamedPipeClientProcessId`, opens the client process token with
`OpenProcessToken`, and compares the token's user SID against the
expected SID. Mismatch or failure to open the token is treated the same
as a uid mismatch on Unix: the connection is rejected before the first
byte is read.

All three branches converge on the same post-condition: the channel
returned to `runtime::dispatch` has been peer-authenticated, and any
request body read from it is known to come from the expected local user.
The rest of the lifecycle — `catch_unwind`, backend dispatch, proto
call, response framing — is platform-independent from that point on.

## Upload chunk lifecycle and daemon-restart resume

The lifecycle above covers synchronous control-plane commands. File
uploads have a second lifecycle overlaid on it, because a single user
intent (“upload this file”) maps to a *sequence* of protocol calls that
must either all commit or be resumable across a daemon restart.

The three-phase upload conversation is:

1. `upload_create` — the daemon allocates a server-side upload id.
   This is the only phase that mints new server state; its response
   carries the id that every subsequent call quotes.
2. `upload_write` — issued one or more times, each carrying a
   contiguous byte range at a specific offset. The server acknowledges
   each write after it is durably buffered.
3. `upload_save` — atomically materialises the accumulated bytes as a
   file inside the target folder. Only this phase makes the upload
   visible to the rest of the account.

Between phases 1 and 3 the daemon maintains a per-inode NDJSON sidecar
(`ino-<inode>.upload-progress`) under the staging directory. Each
`upload_write` acknowledgement updates the sidecar’s `acked_offset`
and `heartbeat_unix_secs` before the next write is issued; the update
uses a write-temp + `fsync(file)` + rename + `fsync(dir)` sequence so
that a crash never leaves a half-written sidecar.

On daemon restart, `bootstrap.rs` runs `replay_upload_sidecars` (in
`pcloud-fs::write_path`) before the mount is brought up. For each
sidecar it calls the `upload_status` protocol method and classifies
the outcome into one of seven variants — `Resumed`, `ServerAhead`,
`SidecarTrimmed`, `Expired`, `Stalled`, `Unparseable`, `BackendError`
— and either continues the upload from the agreed offset, re-sends
trimmed bytes, retires the upload id, or leaves the sidecar for
operator inspection. The mount runtime re-runs the same replay when a
mount is (re)activated after startup.

The full taxonomy, the 10-minute stall timeout, the
`upload_status` semantics, and the companion download-side
`fetch_download_resumable` design (with its `.part` prefix re-hash and
206→200 fallback) are documented in
[Operations → Partial Transfers](../operations/partial-transfers.md).
Readers adding a new transfer call site should consult that page
before choosing between `fetch_download_verified` and
`fetch_download_resumable`.

## Variations across platforms

The same IPC → daemon → backend → proto pipeline runs on Linux, macOS,
and Windows, but three stages of that pipeline have platform-specific
branches. This section consolidates them so a reader can trace a single
`pcloudc sync add /local /remote` end-to-end on any target and know
exactly which module is in play.

### The macOS branch

On macOS the IPC transport is still a Unix domain socket, but peer
identification does *not* use `SO_PEERCRED`, which is Linux-only. The
`PlatformIpc` implementation in
`crates/pcloud-daemon/src/platform/macos.rs` calls
[`getpeereid(3)`](https://developer.apple.com/documentation/) on the
accepted `UnixStream`'s raw fd. `getpeereid` returns the effective uid
and gid of the peer process at the moment of `accept`, which is the
correct instant (later `setuid` calls by the peer do not affect the
already-authorized channel). The returned uid is compared against
`geteuid()`; mismatch closes the socket before any body bytes are read,
identical to the Linux path.

The rest of the IPC frame handling — 8-byte length prefix, bounded body
allocation, JSON decode — is shared code. From `runtime::dispatch`
onward everything is literally the same module the Linux daemon uses,
because the backends, the proto client, and the store are
platform-neutral.

The macOS branch diverges again only when the request touches the
virtual drive. `PlatformMount` on macOS is backed by `fuse-t`, a
user-space FUSE compatibility layer that does not require a kernel
extension. `crates/pcloud-fs/src/platform/macos.rs` spawns the
`fuse-t`-provided helper, hands it a unix socket, and exposes the same
`mount`/`unmount`/`is_mounted` surface as the Linux FUSE backend. To a
sync-backend or transfer-backend call, `fuse-t` and Linux FUSE are
indistinguishable: both appear as a `PlatformMount` whose journal and
staging directory are owned by the daemon.

The `PlatformVault` on macOS is the Keychain. Instead of writing an
owner-only file under `$XDG_DATA_HOME`, the daemon stores the auth
envelope as a generic password item scoped to the daemon's bundle id
and the current login keychain. Keychain ACLs ensure only the daemon's
signed binary can read it back without a user prompt. The envelope
format, the opt-in persistence rule, and the no-cleartext-password
rule are identical to the Linux file-vault path.

### The Windows branch

On Windows the IPC transport is a *named pipe* rather than a Unix
socket. The acceptor lives in
`crates/pcloud-ipc/src/platform/windows.rs` and is consumed by the shared
`pcloud-daemon` runtime.
The pipe name is `\\.\pipe\pcloud-<sid>`, where `<sid>` is the current
user's SID formatted via `ConvertSidToStringSidW`. The pipe is created
with an explicit security descriptor: a DACL built from
`InitializeSecurityDescriptor` + `SetSecurityDescriptorDacl` that
grants `FILE_ALL_ACCESS` to exactly two trustees — the daemon's own
SID and the user's SID — and denies inheritance. There is no
`Everyone` ACE. There is no fallback to `\\.\pipe\pcloud` without a
SID qualifier.

On `ConnectNamedPipe`, the daemon calls
`GetNamedPipeClientProcessId` to learn the peer's PID, opens that
process with `PROCESS_QUERY_LIMITED_INFORMATION`, opens its token with
`OpenProcessToken(TOKEN_QUERY)`, extracts the token's user SID via
`GetTokenInformation(TokenUser)`, and compares that SID against the
expected user SID using `EqualSid`. Any failure along that path —
missing PID, closed process, denied token query, mismatched SID — is
treated exactly like a `SO_PEERCRED` uid mismatch on Linux: the pipe
instance is disconnected before a frame is read. Once the SID check
passes, the 8-byte length prefix, bounded body allocation, and JSON
decode are the same code the Unix daemons run, because that layer lives
in `pcloud-ipc`, not in the acceptor.

`PlatformMount` on Windows is WinFSP. `crates/pcloud-fs/src/platform/windows.rs`
registers a WinFSP service for the session, binds it to a drive letter
or a mount-point directory, and exposes the same trait the Linux and
macOS backends do. WinFSP's callback model is wrapped so that readdir,
open, read, and write dispatch into the same backend code the FUSE
path uses. Journal and staging live under `%LOCALAPPDATA%\pcloud`,
resolved by `PcloudDirs`.

`PlatformVault` on Windows is DPAPI. The auth envelope is wrapped with
`CryptProtectData` using the `CRYPTPROTECT_UI_FORBIDDEN` flag and
stored under `%APPDATA%\pcloud\auth_token`. DPAPI ties the ciphertext
to the user's logon credential, so a file copied to another account
cannot be decrypted even if it is world-readable on the source disk.
The opt-in persistence rule and the no-cleartext-password rule hold on
Windows exactly as they do on Linux and macOS.

After peer authentication and mount/vault plumbing, the three
platforms re-converge. `runtime::dispatch` reads the same `Request`
enum, the same backend functions run the same validation and the same
proto call, the same `Response` is written back through the same
framing code. The net effect is that the bulk of the request lifecycle
— every box in the sequence diagram except the first hop and the
final filesystem side-effect — is single-source. The platform
variations live in exactly the stages the `PlatformIpc`,
`PlatformVault`, and `PlatformMount` traits name, and nowhere else.

## `RequestEnvelope` and traceparent wrapping

Every `Request` that the CLI builds is wrapped in a `RequestEnvelope`
before framing. The envelope carries:

```rust
struct RequestEnvelope {
    request: Request,
    trace_id: Option<String>,        // W3C traceparent: trace-id
    span_id: Option<String>,         // W3C traceparent: span-id
    origin: ClientOrigin,            // cli | web | sdk
    correlation_id: Uuid,            // unique per envelope
    ipc_protocol_version: u16,       // must match daemon
}
```

The envelope is what the daemon decodes and what `Runtime::handle_request`
consumes; the inner `Request` is never decoded in isolation. This gives us
four properties at no per-call cost:

1. A single `correlation_id` threads through CLI logs, daemon logs,
   proto logs, and backend logs — grep-once, find the whole story.
2. Cross-process tracing works whether the upstream was another
   traced service (W3C `traceparent` propagates) or a user shell (a new
   trace is minted at the CLI).
3. The daemon can reject an envelope with an unsupported IPC version
   cleanly without attempting a partial decode.
4. `origin` is observable at the audit-chain writer, so an operator can
   distinguish CLI, web UI, and SDK-originated privileged operations
   after the fact.

The envelope-decoding contract is: if the outer envelope decode fails, the
daemon returns `Response::VersionMismatch` and closes the connection. If
the outer envelope decodes but the inner `Request` fails, the daemon
returns `Response::BadRequest` and keeps the connection open so the client
can retry with a corrected payload. This split prevents the common case of
"malformed Request JSON" from tearing down an otherwise healthy session.

Details on the observability tie-in, including how correlation ids and
trace parents flow into the audit hash-chain, live in
[Security Model](./security-model.md) and in
`crates/pcloud-observability/src/audit.rs`.

## State-machine view of the lifecycle

Five named states span the lifetime of a single daemon-mediated request.
The state machine is implicit in the code but explicit in the audit log.

| State         | Entered on                       | Guard                                  | Exits to                     |
|---------------|----------------------------------|----------------------------------------|------------------------------|
| `Accepted`    | `accept()` returns               | none                                   | `PeerChecked`, `Closed`      |
| `PeerChecked` | peer uid/SID matches daemon      | peer-cred returns ok                   | `Framed`, `Closed`           |
| `Framed`      | 8-byte header + body read ok     | `payload_len <= MAX_IPC_PAYLOAD_LEN`   | `Decoded`, `Closed`          |
| `Decoded`     | envelope + inner request parsed  | `ipc_protocol_version == IPC_PROTOCOL_VERSION` | `Dispatched`, `Closed` |
| `Dispatched`  | `handle_request_dispatch` entered | `catch_unwind` boundary                | `Responded`, `Responded(panic)` |
| `Responded`   | framed `Response` written        | writer not disconnected                 | (terminal)                   |

A panic during `Dispatched` transitions into `Responded(panic)` via the
`catch_unwind` guard, which synthesises `Response::InternalError` and
increments a Prometheus counter. The transition is *observable*: audit
logs emit one record per state exit, so reconstructing the lifecycle of a
single correlation id is a straightforward log filter.

## Tradeoffs and design decisions

- **Why decode envelope and inner request separately?** Because IPC
  version skew is a recoverable condition — the client should be told to
  upgrade — while a malformed body is a bug in the specific request. One
  decode step would conflate them.
- **Why not pipeline requests on a single connection?** Because the
  `catch_unwind` boundary is per-request and cleanup is simpler with
  one-request-per-connection. IPC connections are cheap; the daemon is
  not a web server.
- **Why synthesise a full `Response::InternalError` on panic rather than
  closing the socket?** Because the CLI has a canonical exit-code table
  (see `reference/exit-codes.md`) and a clean response-based failure is
  easier to test against than a closed socket.
- **Why carry a correlation id in the envelope rather than mint one at
  the daemon?** So that CLI logs, web-UI logs, and daemon logs share the
  same id — the id must exist *before* the request hits the socket.

## Concurrency model (request-path)

- One OS thread per accepted connection. The `IpcServer::run` loop
  calls `accept()` on the listener, spawns a thread, and resumes. No
  tokio.
- Shared state touched by dispatch:
  `Runtime::state` (`parking_lot::RwLock`), vault handle
  (`parking_lot::Mutex`), engine queue (`crossbeam_channel`), proto
  client (stateless, cloned from `Arc`).
- The panic guard (`catch_unwind`) runs in the accepted thread; a
  panic inside the guard cannot poison the runtime lock because the
  lock is released before the guard boundary.
- Protocol HTTPS calls are blocking inside the dispatch thread; the TLS
  stack (`rustls` + `ureq`) is synchronous. This is the entire reason
  the daemon is thread-per-connection rather than tokio-first.

## Security invariants (request-path)

- Peer identity is verified before the first body byte is read (tests:
  `crates/pcloud-ipc/tests/peer_cred_linux.rs`,
  `crates/pcloud-ipc/tests/peer_cred_macos.rs`,
  `crates/pcloud-ipc/tests/platform_ipc_crossplat.rs`).
- Body size is capped at `MAX_IPC_PAYLOAD_LEN` = 1 MiB before any
  allocation (test:
  `crates/pcloud-ipc/tests/frame_size_cap.rs`).
- `catch_unwind` wraps every dispatch arm; a panic converts into
  `Response::InternalError` (test:
  `crates/pcloud-daemon/tests/panic_guard.rs`).
- Production configs refuse plaintext downgrade from TLS (test:
  `crates/pcloud-proto/tests/transport_tls_required.rs`).
- Secret-bearing request fields (`Request::Login { password, ... }`,
  `Request::TfaCode { code, ... }`) use `SecretString` (test:
  `crates/pcloud-ipc/tests/secret_redaction.rs`).

## Performance notes

- The envelope decode is ~1 µs for a typical request (JSON, bounded by
  1 MiB cap); this is the *first* allocation the daemon makes per
  request.
- The panic guard (`catch_unwind`) costs a setjmp-equivalent on entry;
  measured < 50 ns on Linux x86_64, lost in the noise against any real
  backend call.
- The dispatch `match` arm selection is O(#variants); the compiler
  generates a jump table for us.

## Extension points

- **New method**: six-file change enumerated in "Where to add a new
  method" above.
- **New IPC protocol version**: bump `IPC_PROTOCOL_VERSION` in
  `crates/pcloud-ipc/src/protocol.rs:12`; the daemon's
  `VersionMismatch` path is the forward/backward compatibility story.
- **Alternate transport** (e.g. vsock for VM-hosted clients): implement
  `PlatformIpc::accept` for the new socket family; the request pipeline
  above is unchanged from `peer_identity` onward.

## Open `bd` trackers

- **`bd-1du`** — parity epic.
- **`bd-1du.4`** — mount lifecycle interaction with the request path on
  non-Linux.
- **`bd-1du.4.6.1`** — write-path daemon wiring (ADR-0010).
- **`bd-1du.10`** — final parity proof gating docs.

## Cross-references

- [Overview](./overview.md) — the five platform abstractions and
  processes at play.
- [Crate Map](./crate-map.md) — where each step lives.
- [Performance](./performance.md) — hotpaths that live inside this
  lifecycle.
- [Platform Support](./platform-support.md) — per-platform variations
  in peer-check and mount.
- [Security Model](./security-model.md) — enumerated invariants and
  their test citations.
- [Operations → Partial Transfers](../operations/partial-transfers.md) —
  the upload-resume lifecycle overlay.
- [ADR-0002](../adr/0002.md), [ADR-0004](../adr/0004.md),
  [ADR-0010](../adr/0010.md) — related decisions.
