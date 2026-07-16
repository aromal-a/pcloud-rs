# Audit 06 — Transport Delta (Iteration 2)

**Date:** 2026-04-29
**Auditor:** Claude Agent (Opus 4.7, 1M context)
**Iter 1 baseline:** `CLAUDEREV/06-transport.md` (HIGH 1 / MED 3 / LOW 3)
**Mode:** read-only delta walk against the live tree

## Verdict

**Convergence on this dimension.** No new findings. One iter-1 LOW
finding (L-3, `PCLOUD_WIRE_CAPTURE_DIR` plaintext auth-token leak) is
now **resolved** in code — the wire-capture seam has been removed
from `crates/pcloud-proto/src/transport.rs` and no `PCLOUD_WIRE_*`
env-var read survives anywhere under `crates/`. All other iter-1
findings stand exactly as previously characterised.

## Re-verification by delta question

### 1. TRANSPORT-H-1 — does any production call path use `ResilientTransport`?

**No.** Walked the chain `runtime.rs` → `bootstrap.rs` →
`transport_factory.rs` → `crates/pcloud-backends/src/*_backend.rs`.
The factory exists and is constructed in
`crates/pcloud-daemon/src/bootstrap.rs:513`
(`TransportFactory::new(config.environment, config.resilience.clone())`)
and stored on `RuntimeShell::transport_factory`
(`crates/pcloud-daemon/src/runtime.rs:174`), but **no backend pulls it
out and calls `wrap_binary`**. A repeat
`grep -n "resilient_transport\|ResilientTransport\|wrap_binary\|transport_factory"`
across `crates/pcloud-backends/` returns **zero matches**, identical to
iter 1. The same 10 backend sites still construct
`BinaryApiTransport::new(TransportConfig::with_tls(...))` directly
(`auth_backend.rs:283`, `account_backend.rs:235`,
`transfer_backend.rs:318`, `folder_backend.rs:263`,
`sync_backend.rs:426`, `shares_backend.rs:277`,
`crypto_backend.rs:211`, `backup_backend.rs:434`,
`public_link_backend.rs:641`, `notifications_backend.rs:179`).
H-1 is unchanged: circuit breaker, token-bucket rate limit, and the
`GlobalRetryBudget(100)` allocated by `TransportFactory::new` are
**dead in the production hot path**.

### 2. HTTP client choice and timeouts

**Binary API (auth, folder, transfer-control, shares, crypto, backup,
public-link, notifications):** raw `std::net::TcpStream` + rustls
(`crates/pcloud-proto/src/transport.rs:36`,
`crates/pcloud-proto/src/tls.rs`). **Not reqwest, not hyper.**

**Signed-HTTP download (`http_download.rs`):** also raw
`TcpStream` + rustls (`crates/pcloud-proto/src/http_download.rs:36-46`).
A bespoke HTTP/1.1 request is hand-written and headers are
hand-parsed.

**`reqwest` / `hyper` are pulled into the workspace, but only in
non-protocol crates** that have nothing to do with the pCloud API:

- `crates/pcloud-kms/Cargo.toml:33` — KMS HTTP clients (Vault, etc.)
- `crates/pcloud-fleet/Cargo.toml:24,44` — fleet management plane
- `crates/pcloud-idp/Cargo.toml:31` — external IdP / OIDC
- `crates/pcloud-observability/Cargo.toml:29` — OTLP exporter

None of these are on the data path against `*.pcloud.com`. So the
"HTTP client surface area" for the API is the workspace's own
hand-rolled implementation, audited as part of `pcloud-proto`.

**Timeout budget:** `connect_timeout` (default 10 s,
`TransportConfig::DEFAULT_CONNECT_TIMEOUT`,
`crates/pcloud-proto/src/transport.rs:136`) and
`total_request_timeout` (default 300 s, line 145) are both bound and
both enforced. `read_timeout` is per-syscall, not per-request, but
the 300 s whole-request deadline closes the slowloris hole. Iter-1
M-2 (lack of `write_timeout_ms` / `total_request_timeout_ms` in
`ApiEndpoint`) still applies — nothing changed.

### 3. DNS resolution

Pure stdlib `ToSocketAddrs::to_socket_addrs` —
`crates/pcloud-proto/src/transport.rs:474`,
`crates/pcloud-proto/src/http_download.rs:398`. **No custom resolver,
no `trust-dns`, no in-process DNS cache.** That means resolution falls
through to libc / nsswitch / glibc-resolver and the OS controls
caching. There is **no stale-cache risk inside the workspace**:
every TCP connect reissues DNS through the OS resolver. (Conversely,
at very high QPS, no in-process cache means every connect pays a
syscall — but this is the same model the legacy C client used and is
not a regression.) No new finding.

### 4. Cookie handling on auth flows

`grep -n "cookie\|Cookie"` on `crates/pcloud-proto/`,
`crates/pcloud-backends/`, and the daemon: the **only** cookie on the
pCloud-server-facing path is the `dwltag` download cookie attached to
signed-HTTP GETs (`http_download.rs:763,1061`). It is per-request,
non-secret, and not persisted. There is **no session-cookie storage**
on the API path — pCloud's binary API is auth-token-based, not
cookie-based. The auth token is persisted (vault-protected,
`auth_vault.rs`, owner-only `0600`), audited under dim 02 already.

The `pcloud-web` crate uses CSRF cookies (double-submit pattern,
`HttpOnly; SameSite=Strict`) — that's a local web-UI layer, not an
egress concern. No new finding.

### 5. Compression / decompression — bomb protection?

No `Accept-Encoding` is sent and **no `Content-Encoding` handling
exists** anywhere in `pcloud-proto`. `grep -in
"accept-encoding\|content-encoding\|gzip\|deflate\|brotli"
crates/pcloud-proto/src/http_download.rs` → **no matches**. The body
is read as bytes-on-the-wire, with a hard 64 MiB cap on binary-API
responses (`DEFAULT_MAX_RESPONSE_BYTES`, `transport.rs:147`) and a
deadline on signed downloads.

That means **compression-bomb risk is structurally absent on the API
path** — there is no decompressor that could expand a small response
into an OOM-class allocation. The workspace's exposure to bombs is
limited to (a) opt-in OTLP exporter via `reqwest` and (b)
`pcloud-fleet` / `pcloud-kms` / `pcloud-idp`, all of which are out of
scope for this dim. No new finding.

### 6. 5xx vs 4xx classification, `Retry-After` interaction with H-1

The HTTP-side `Retry-After` parser
(`crates/pcloud-resilience/src/transport.rs:218-378`) is correct — it
honors both delta-seconds and IMF-fixdate, caps at 300 s, and the
budget logic does not consume tokens during `Retry-After` waits
(line 808-814). **But** because the binary-API hot path does not pass
through `ResilientTransport` at all (H-1), the binary path **does not
parse `Retry-After`** — it can't, the binary protocol does not carry
HTTP headers. So `Retry-After` is only meaningful on
`http_download.rs::fetch_download_resumable`, which delegates to the
canonical parser and retries once on 429/503 (line 638-647).

**5xx vs 4xx classification:** the `transport_error_classifier` in
`resilient_transport.rs:505-530` only sees `TransportError` (Io, Tls,
Connect, ResponseTooLarge, …) — not HTTP status codes. HTTP status
classification is done inside `http_download.rs` and is consistent:
4xx (other than 429) is fatal-permanent (no retry), 429/503 retry once
with `Retry-After`. No re-finding to add — this is described accurately
in iter 1 retry table row 5.

The classifier tag `TransportError::Connect(_) => Transient` (iter-1
L-2) still treats every connect failure the same regardless of
underlying `io::ErrorKind`, so a typo'd port still burns budget. **L-2
unchanged.**

### 7. Wire-capture / debug dumps — env-var or config knob that turns on dumping silently?

**Resolved.** Iter 1 L-3 flagged `PCLOUD_WIRE_CAPTURE_DIR` writing
auth-token plaintext to mode-0o600 files. As of this audit:

- `grep -in "PCLOUD_WIRE_CAPTURE\|wire_capture\|WireCapture" crates/`
  returns **zero matches**.
- `grep -n "env::var\|getenv\|std::env" crates/pcloud-proto/` returns
  only two unrelated reads (`HOSTNAME` / `HOST` for client device
  identification in `methods/auth.rs:405-406`, and a `temp_dir()` in a
  test).
- The historical seam survives only as text in `GPTREV/`,
  `CLAUDEREV/06-transport.md`, and `.audit-fragments/SHARES-A2B-…` —
  audit-trail documents, not live code.

Because there are **no other PCLOUD_* env vars that mutate transport
behaviour silently** (the bootstrap reads `PCLOUD_*` knobs through the
typed `apply_env_overrides` validator in `pcloud-config`, which writes
warn-level logs on every override), the previously-flagged silent
debug-dump risk is closed. **Drop L-3 from the open finding count on
the next consolidated tally.**

## Net delta

- Resolved: 1 (L-3 wire-capture leak)
- New: 0
- Unchanged: 5 (H-1, M-1, M-2, M-3, L-1, L-2 = 1H/3M/2L)
- Updated severity-class totals: HIGH 1 / MED 3 / LOW 2.

H-1 remains the dominant outstanding item: the resilience scaffolding
is in the tree, instantiated at boot, exercised in tests, but **not
threaded into a single production backend**. Until that lands,
"production has circuit breaker, retry budget, and rate limit" is a
documentation claim, not a behavioural one.

delta count: 0
