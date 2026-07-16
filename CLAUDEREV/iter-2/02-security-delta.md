# Security Audit — Iteration 2 Delta

Date: 2026-04-29 (delta)
Scope: re-walk security-critical surface for findings missed by iter 1
(`CLAUDEREV/02-security.md`, totals 0 CRITICAL / 4 HIGH / 7 MEDIUM / 4 LOW).

## Convergence: YES (with one minor tightening recommendation)

After a focused delta walk over (a) the less-trodden crates iter 1 sampled
lightly (`pcloud-fleet`, `pcloud-idp`, `pcloud-kms`, `pcloud-policy`,
`pcloud-session`, `pcloud-p2p`, all `pcloud-plugin-*`, `pcloud-chaos`,
`pcloud-web`), (b) the IPC FS-by-path methods landed since 2026-04-29
(`ListFolderByPath`, `CreateFolderByPath`, `FileDeleteByPath`,
`FolderDeleteByPath`, `WriteFileFresh`, `ReadFileRange`), (c) macOS
peer-credential enforcement specifically, (d) outbound HTTP / SSRF
posture in `pcloud-web`, (e) every `unsafe` block added since iter 1,
and (f) every `info!`/`warn!`/`error!`/`debug!`/`trace!`/`println!`/
`eprintln!` macro in those crates, **no new exploit-class findings
were identified** beyond what iter 1 already enumerated.

The unsafe-block delta count rose from 411 → 457 (+46), but every new
block is in `pcloud-fs/src/mount_orphan.rs`, `pcloud-fs/src/lib.rs`,
`pcloud-fs/src/write_path.rs`, the platform shims, and FUSE adapters —
each carries a `// SAFETY:` comment within the immediate window
(verified by an awk pass over the new files). The class of finding
M2 from iter 1 (undocumented `unsafe { ... }` blocks) is **not
expanding**.

---

## Per-axis delta findings

### Secret-bearing fields in less-trodden crates

Re-grepped the pattern `(password|token|secret|priv_key|passphrase|
api_key|cookie|session_id) : (String|Vec<u8>|&str)` across
`pcloud-fleet`, `pcloud-idp`, `pcloud-kms`, `pcloud-policy`,
`pcloud-session`, `pcloud-p2p`, `pcloud-plugin-*`, `pcloud-chaos`,
`pcloud-web`. Only matches:

- `pcloud-fleet/src/lib.rs:230 token: String` — already iter 1 M1.
- `pcloud-idp/src/oidc.rs:148 id_token: String` — already iter 1 M1.
- `pcloud-idp/src/exchange.rs:209 id_token: &str` — function parameter,
  not a stored credential. Caller-controlled lifetime; accepted.
- `pcloud-idp/src/jwks.rs:213 id_token: &str` — same as above.
- `pcloud-web/src/lib.rs:209 web_token: String` — already iter 1 H3.
- `pcloud-web/src/routes.rs:279 password: String` (PublinkCreateForm) —
  iter 1 noted explicit `Drop` zeroize on this struct; verified.

`pcloud-kms`, `pcloud-policy`, `pcloud-session`, `pcloud-p2p`,
`pcloud-chaos`, and the four `pcloud-plugin-*` crates returned
**zero** matches. Convergence on the secret-field axis.

### Logging discipline in less-trodden crates

Re-grepped `(info!|warn!|error!|debug!|trace!|println!|eprintln!)`
intersected with `(token|password|secret|priv_key|auth)` across the
same set. Every match was either:

- a **NAME** of an env var being reported as missing (e.g.
  `PCLOUD_LIVE_PASSWORD is not set`) — the value is never logged,
- a redacted-by-Debug `SecretString` (`pcloud-secret/examples/
  roundtrip.rs:28` calls `{token:?}` on a `SecretString`, which prints
  `SecretString(<redacted>)`), or
- a generic `auth: <message>` where `<message>` is a server-provided
  status string (no token).

No log macro writes `expose_secret()` or a raw token / password.
Iter 1's "no log macro found writing a secret value" stands.

### macOS IPC peer-credential enforcement

Iter 1 covered Linux (`SO_PEERCRED`, `platform/linux.rs:42`) and Windows
(named-pipe SID DACL, `platform/windows.rs`) explicitly. **macOS goes
through `platform/unix.rs:44-53` `libc::getpeereid(fd, &mut uid,
&mut gid)`** — verified live in this delta. The unsafe block has a
correct SAFETY comment (lines 49-52). `getpeereid(3)` is the
documented BSD-family portable equivalent of `LOCAL_PEERCRED`; macOS
does technically also expose `LOCAL_PEERCRED` via `getsockopt`, but
`getpeereid(3)` is fully sufficient for uid-based authorization (it
returns the effective uid of the connecting process), and macOS man
page for `getpeereid` confirms it is a wrapper around the same
kernel call. Convergence: macOS peer-cred posture is **as enforced
as Linux**.

### Path-validation gaps at SDK / IPC / FS-by-path entry points

The IPC FS-by-path family (`ListFolderByPath`, `CreateFolderByPath`,
`FileDeleteByPath`, `FolderDeleteByPath`, `WriteFileFresh`,
`ReadFileRange`) and SDK path methods (`SdkClient::stat_path`,
`delete_file`) only verify `path.starts_with('/')`. They do **not**
explicitly reject `..` segments, NUL bytes, or empty components.

Closer inspection: these are **remote pCloud-drive paths**, not
host filesystem paths. The resolution path goes through
`FileMetadataRepository::resolve_path`
(`pcloud-store/src/repositories/file_metadata.rs:123-154`) which
splits on `/`, filters empty components, and walks segment-by-segment
via `get_by_parent_and_name` against the SQLite cache. A `..`
segment becomes a literal name lookup against the DB and returns
`None` (no matching row). NUL bytes would either error in the SQLite
binding or no-match. There is **no host syscall path-walk** between
the IPC boundary and the cache lookup, so traversal-class attacks
have no semantic. This is consistent with iter 1's L2 closure;
**no new finding**.

### `WriteFileFresh` payload sizing

`runtime.rs:1771-1839` decodes the base64 body **before** checking
`bytes.len() > MAX_WRITE` (32 MiB). At first glance this looks like
a CPU-DoS amplifier (decoding 1.33× the wire bytes before rejecting).
However, the IPC framing layer enforces `MAX_REQUEST_BYTES = 1 MiB`
**before** allocation (`pcloud-ipc/src/server.rs:42` +
`transport.rs:911-928`, confirmed in iter 1's positive findings),
so a 32 MiB body cannot reach this handler. The 32 MiB cap is dead
code in the current IPC topology and is documented as defense in
depth for a future chunked-driver variant. **Not a finding.**

### Outbound HTTP / SSRF in `pcloud-web`

`pcloud-web` does **not** use `reqwest` or any HTTP client (grep
empty). All outbound communication goes through `pcloud_ipc::IpcClient`
to the local UNIX socket. There is no network egress surface, so
SSRF is structurally impossible. CSRF is enforced via a `pcw_csrf`
cookie with `HttpOnly; SameSite=Strict; Path=/`
(`routes.rs:761`), and mutating routes additionally require an
`X-PCloud-Web-Token` header matching `WebConfig.web_token`. The
bind address is asserted to be loopback (`lib.rs::serve` panics
otherwise). The `Secure` cookie flag is missing, but the bind is
HTTP-only by design (loopback-only) and `SameSite=Strict` already
prevents cross-origin submission, so this is intentional and
documented. Convergence on the web axis.

### `unsafe` block delta since 2026-04-29

Total production unsafe count rose from 411 → 457 (+46 blocks). The
new blocks are concentrated in:

- `pcloud-fs/src/mount_orphan.rs` (+1, `libc::unmount`, SAFETY documented)
- `pcloud-fs/src/lib.rs`, `write_path.rs` (FFI shim adjustments)
- `pcloud-fs/src/platform/{linux,macos,windows,bsd}.rs` (mount-cleanup
  reaper bodies and FUSE adapter callbacks)
- `pcloud-fs/src/platform/winfsp_ffi.rs` (Windows compile-clean shim)
- `pcloud-ipc/src/platform/windows.rs` (named-pipe SID handling)

Spot-checked the new `mount_orphan.rs:280` unsafe block —
`libc::unmount(path_cstr.as_ptr(), 0)` carries a SAFETY comment
4 lines above (`277-280`). No new undocumented unsafe blocks were
introduced. Iter 1's M2 footprint stays at the same ~10
genuinely-undocumented blocks; the delta is purely additive in the
documented-unsafe column.

---

## Iter 1 findings to retract

**None.** All four HIGH (H1-H4), seven MEDIUM (M1-M7), and four LOW
(L1-L4) iter 1 findings re-verified intact:

- H1, H2, H3 secret-field offenders still have `String` types at the
  cited file:line locations.
- H4 TLS revocation still defaults to Disabled in
  `pcloud-config/src/api.rs`.
- M1-M7 conditions unchanged.
- L1-L4 conditions unchanged.

The only adjacent observation worth flagging: `pcloud-web::AppState`
**does** wrap the token in `Arc<SecretString>` (lib.rs:280), which
makes the public `WebConfig.web_token: String` (lib.rs:209)
specifically the surface to fix — the request-handler path is already
correct. This refines but does not retract H3.

---

## Single new soft observation (informational, no severity)

The IPC `WriteFileFresh` handler decodes base64 before length-checking,
which is harmless today (IPC frame cap precedes it) but creates a
trap for the future "chunked driver" variant referenced in the
in-source comment. When that variant is wired (P7 follow-up), the
length check should move *before* decoding (compute decoded length
from the b64 length: `bytes.len() ≈ data_b64.len() * 3 / 4`). This
is not a current finding; it is a forward-compat note for the
P7 maintainer.

---

delta count: 0 new findings, 0 retractions
