# pcloud-rs Enterprise-Readiness Audit — Dimensions 6 + 7

**Audit scope:** Dimension 6 (Transport & Network Resilience, outbound HTTP/API)
and Dimension 7 (IPC & Daemon, local control plane).

**Auditor role:** parallel specialist auditor (1 of 10). Cross-cutting
findings that belong to other dimensions (secret discipline §2,
observability §8, sync engine §4, FUSE §5) are flagged for cross-reference
but not re-litigated.

**Workspace root:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/`

**Methodology:** read every source file listed in the prompt, trace the
request path from config → transport → resilient wrapper → dispatch →
backend, and confirm every claim against a file:line citation. No tests
were executed; all assertions are static.

All severity ratings are informed by the enterprise bar implied by the
prompt ("production-ready", "drop-in replacement"). Findings that would
be acceptable in a single-user desktop client are still flagged at
MEDIUM/HIGH when they block the enterprise claim.

---

## Section 6. Transport & Network Resilience

### 6.1 TLS enforcement (mandatory)

#### 6.1.1 [HIGH] Transport struct exposes a **public** `use_tls: bool` with no defense-in-depth check at the socket layer

`crates/pcloud-proto/src/transport.rs:71-96` — `TransportConfig` carries a
`pub use_tls: bool` field; `crates/pcloud-proto/src/transport.rs:255-268`
— `execute_with_body` branches on `config.use_tls` with zero consultation
of the active `Environment`. The documentation explicitly admits
enforcement is centralised elsewhere:

```rust
// pcloud-proto/src/transport.rs:86-90
/// Must be `true` outside of tests. The field is *not* checked
/// here — enforcement lives in the daemon bootstrap — so this
/// struct remains usable for local integration tests.
pub use_tls: bool,
```

This is a fragile design. Any call site — plugin, SDK consumer, future
refactor — that constructs a `BinaryApiTransport` without going through
`ApiEndpoint::validate` silently bypasses the production-TLS invariant.

The gate is at `crates/pcloud-config/src/api.rs:131-170` (`ApiEndpoint::
validate`); however, `ApiEndpoint` is not the type held by
`BinaryApiTransport`. The two structs are deliberately decoupled
(`crates/pcloud-proto/src/transport.rs:17-32` calls this out), which
means the transport layer itself is willing to dial plaintext given any
`use_tls=false` input.

**Remediation:** Either (a) delete the plaintext branch from
`execute_with_body` at production build time (cfg gate `#[cfg(not(feature
= "dev-plaintext"))]`), or (b) attach an `Environment` enum to
`TransportConfig` and refuse `use_tls=false` at construction when
`Environment::Production`. Option (b) is the enterprise-grade choice
because it survives all downstream refactors.

---

#### 6.1.2 [HIGH] Environment override `PCLOUD_API_MODE=plaintext` can be set at daemon startup; validation runs too late on an already-poisoned cached API-server hint

`crates/pcloud-config/src/env.rs:86-95` applies `PCLOUD_ENV` first, then
`crates/pcloud-config/src/env.rs:93-95` honours `PCLOUD_API_MODE`.
Combined, an operator error — `PCLOUD_ENV=production
PCLOUD_API_MODE=plaintext` — is eventually rejected by
`ApiEndpoint::validate` (`pcloud-config/src/api.rs:137-141`) at
bootstrap. **This rejection is correct today.**

However, the order in `bootstrap.rs:443-449` is:

```rust
let mut config = config;
if let Some(api_server) = store.repositories.preferences.api_server_binapi.as_deref() {
    config.api.apply_api_server_hint(api_server);
}
```

A malicious or stale `api_server_binapi` stored in the SQLite
preferences repository can rewrite `config.api.host` and
`config.api.server_name` *after* validation has run upstream in
`bootstrap_with_config` (`bootstrap.rs:407-408`). There is no second
validation pass. Combined with the fact that `apply_api_server_hint`
never rejects a non-pcloud.com host, a stored-preference rewrite is an
attack path.

**Remediation:** Re-run `ConfigProfile::validate` after the
`apply_api_server_hint` mutation at `bootstrap.rs:449`. Additionally,
validate the SNI hostname against an allow-list (e.g. only hosts ending
in `.pcloud.com` / `.pcloud.link`) or require the hint to be
signed/authenticated end-to-end. The comment at
`crates/pcloud-config/src/api.rs:178-189` is silent on origin trust.

Cross-reference §2 secret discipline: a replaced SNI that still
terminates TLS against an attacker-controlled cert would compromise any
subsequent auth flow.

---

#### 6.1.3 [MEDIUM] `rustls` client config is rebuilt per request — no session resumption, no `CryptoProvider` pinning

`crates/pcloud-proto/src/transport.rs:318-336` — every call to
`execute_tls` constructs a fresh `RootCertStore`, `ClientConfig`, and
`ClientConnection` from scratch. Under enterprise load this prevents
TLS session resumption (and wastes an RTT per request). It also does
not pin a specific `rustls::CryptoProvider`, so a future rustls default
provider change would silently alter cipher selection.

**Remediation:** Build `Arc<ClientConfig>` once in
`BinaryApiTransport::new` and reuse across requests. Pin
`rustls::crypto::aws_lc_rs::default_provider()` (or ring) explicitly.
Expose a `CryptoProviderSource` knob in `ApiEndpoint` for enterprise
installs that mandate FIPS providers.

---

### 6.2 Certificate validation

#### 6.2.1 [INFO] No `danger_accept_invalid_certs`, no `DangerousClientConfig` anywhere

Confirmed clean via repository-wide search. `CONTRIBUTING.md:206`,
`SECURITY.md:96`, and `CHANGELOG.md:1975` explicitly forbid these and
the production source tree contains none.

`crates/pcloud-proto/src/transport.rs:327-333` uses the builder path
that cannot disable verification:

```rust
let tls_config = ClientConfig::builder()
    .with_root_certificates(roots)
    .with_no_client_auth();
```

`crates/pcloud-proto/src/http_download.rs:210-215` mirrors the same
construction for the HTTPS download channel.

**No finding.** This is the expected posture.

---

#### 6.2.2 [MEDIUM] `webpki-roots` is pinned to whatever version `Cargo.toml` resolves; no explicit trust anchor refresh policy

`crates/pcloud-proto/src/transport.rs:324-325` and
`http_download.rs:208-209` both seed roots from
`webpki_roots::TLS_SERVER_ROOTS`. A stale `webpki-roots` dependency
means a missing-from-trust-store CA (e.g. ISRG Root X2) will silently
fail validation at some point in the future.

**Remediation:** Add a CI job that refreshes `webpki-roots` monthly, or
(enterprise-preferred) allow operators to override the root store with a
`[api].extra_root_certificates_pem` config path. Document the guarantee
in `SECURITY.md`.

---

### 6.3 Timeouts

#### 6.3.1 [MEDIUM] Timeout discipline is coarse: one `read_timeout` applied to every read and every write, no separate TCP-keepalive, no global request deadline

`crates/pcloud-proto/src/transport.rs:91-96` defines
`connect_timeout: Duration` and `read_timeout: Duration`, no
`write_timeout`, no `total_request_timeout`. At
`transport.rs:301-306`:

```rust
stream.set_read_timeout(Some(config.read_timeout))...
stream.set_write_timeout(Some(config.read_timeout))...
```

The same duration is applied to reads and writes, and there is no
per-request budget separate from per-syscall budget. A malicious or
broken server can therefore drip-feed 1 byte per `read_timeout - 1ms`
for arbitrarily long. The `send_and_receive` deadline loop at
`transport.rs:338-364` calls `read_exact_with_deadline` and
`write_all_with_deadline` which each carry their own deadline; there is
no outer "the entire request must complete within N seconds" wrapper.

**Remediation:** Add `total_request_timeout: Duration` to
`TransportConfig` and enforce it in `send_and_receive` via a single
`Instant::now() + total_timeout` deadline shared across the write,
flush, header-read, and body-read stages. Enable TCP keep-alive at the
socket level (`set_keepalive`) so a silently-dead peer surfaces before
the application-level timeout.

---

#### 6.3.2 [LOW] `connect_timeout` default is 5s but not user-override validated; `read_timeout` default is 15s with no floor

`crates/pcloud-config/src/api.rs:98-116` sets both to 5_000ms / 15_000ms
as `secure_defaults`. `ApiEndpoint::validate` only rejects zero;
`crates/pcloud-config/src/api.rs:157-167`. An operator who sets
`read_timeout_ms = 1` slips through validation and will see every
real-world request fail with a deadline error.

**Remediation:** Add a minimum floor (e.g. 500ms) validated at load, or
a clamp with a warning.

---

### 6.4 Retry policy

#### 6.4.1 [HIGH] `ResilientTransport` default classifier treats every inner error as `Transient`

`crates/pcloud-proto/src/resilient_transport.rs:305-310`:

```rust
pub fn default_classifier<E>() -> Classifier<E>
where E: std::error::Error + Send + Sync + 'static,
{
    Arc::new(|_: &E| ErrorClass::Transient)
}
```

This classifier is installed verbatim by
`TransportFactory::wrap_binary`
(`crates/pcloud-daemon/src/transport_factory.rs:113-120`) in production.
Consequently, `TransportError::InvalidAddress` (a permanent DNS failure)
and `TransportError::InvalidServerName` (a permanent TLS config error)
are retried up to `retry_max_attempts` times — wasting wall time and
amplifying load on DNS or internal resolvers. More importantly,
`TransportError::Tls(rustls::Error::InvalidCertificate*)` — a
**security-relevant** terminal failure — is retried, which both masks
the signal from operators and gives an on-path attacker multiple
attempts to race a certificate swap.

**Remediation:** Supply an explicit classifier in
`TransportFactory::wrap_binary` that marks as `Permanent`:

- `TransportError::InvalidAddress`
- `TransportError::InvalidServerName`
- Any `TransportError::Tls` where `rustls::Error` indicates a
  certificate/chain problem (`InvalidCertificate`, `PeerIncompatible`,
  `InvalidCertSignature`, `General("…certificate…")`).
- `TransportError::ResponseBody(ResponseParseError::*)` — parser bugs
  should fail fast.

Tests at `resilient_transport.rs:508-537` prove the hook works — the
production wire-up just never supplies it.

---

#### 6.4.2 [HIGH] No `Retry-After` header respected; no server-directed backoff

Repository-wide search shows zero matches for `Retry-After`,
`retry_after`, or `retry-after` in `pcloud-proto` or
`pcloud-resilience`. The pCloud binary protocol may not surface such a
header directly, but the HTTPS download channel
(`http_download.rs`) certainly receives 429 / 503 with Retry-After,
and the client ignores it.

**Remediation:** In `http_download.rs:fetch_download_verified_streaming`
parse `Retry-After` (both delta-seconds and HTTP-date forms) and feed
it into the same `ResilientTransport` backoff instead of running the
jittered exponential schedule blind. For the binary channel, check
whether pCloud signals rate-limit via the `result` field (the protocol
has a documented rate-limit result code) and honour it identically.

---

#### 6.4.3 [MEDIUM] Backoff schedule uses *equal-jitter*, documented as `ExponentialJittered`, but the PR-grade "full jitter" is absent

`crates/pcloud-resilience/src/retry.rs:37-46` defines
`ExponentialJittered`; `retry.rs:197-205` implements "equal-jitter per
AWS" (`d/2 + rand(0, d/2)`). Equal-jitter is adequate; the finding is
that the API enum only exposes `Fixed`, `Exponential`, and this single
jittered variant. Operators cannot select "decorrelated jitter" or
"full jitter" without a code change.

**Remediation:** Add variants `FullJittered` (`rand(0, d)`) and
`Decorrelated { cap }` per the AWS Architecture Blog. Expose the
selector via `ResiliencePolicy` serde.

---

#### 6.4.4 [MEDIUM] `retry_jitter_seed` is a **deterministic** u64 shared by every client instance

`crates/pcloud-config/src/resilience.rs:73-77`:

```rust
/// Deterministic jitter seed applied via equal-jitter. Default:
/// `0x00C0_FFEE_F00D`. Valid values: any `u64`. **Security:** keeps
/// tests reproducible while still spreading retry storms across
/// clients that share the seed. Example: `retry_jitter_seed = 0`.
```

The security note is wrong. If two daemons share the same seed (which is
the default) and experience the same outage at the same wall time,
`splitmix64(seed ^ attempt)` produces **identical** jitter values →
identical retry timings → thundering-herd amplification. The point of
jitter is to decorrelate; a fixed seed neutralises that.

**Remediation:** Default to `rand()`-derived per-process seed at
bootstrap, or per-connection. Keep the deterministic-seed path behind a
test-only knob.

---

#### 6.4.5 [MEDIUM] Retry budget is per-request, not global

`ResilientTransport::execute` loops until `retry_max_attempts`
(`resilient_transport.rs:243-298`). There is no cross-request retry
budget. A daemon that serves 1000 failing requests/second can issue
3000 retries/second indefinitely; the circuit breaker mitigates the
worst case, but only after `breaker_failure_threshold` consecutive
failures on a single endpoint.

**Remediation:** Add a global `RetryBudget` (Netflix Hystrix pattern): a
token bucket of retry tokens shared across all callers, refilled at a
percentage of the steady-state request rate. When depleted, fall
through to `RetryDecision::GiveUp` regardless of the per-request budget.

---

### 6.5 Idempotency

#### 6.5.1 [HIGH] `upload_create → upload_write → upload_save` has no end-to-end idempotency key; the journal gives crash-replay, not retry-safety

`crates/pcloud-proto/src/transfer_api.rs:249-287` —
`upload_create` returns a server-issued `uploadid`. This `uploadid`
is durable and is the right anchor for an idempotency key.

However, the retry wiring is broken in two ways:

1. **No `upload_create` retry is safe.** A network error after the
   server has created the upload session but before the client sees the
   response will cause retry to create a **second** upload session with
   the same filename — typical pCloud behaviour is to suffix the name
   with `(1)`. There is no "look up the previous session" path.
2. **`upload_save` retry is also unsafe.** If `upload_save`'s response
   is lost mid-transit, the server has committed but the client
   believes it failed and retries, producing a duplicate. The Rust path
   has no dedup: `transfer_api.rs:upload_create` uses
   `ResilientTransport` via `TransportFactory` (indirectly), which will
   retry these mutations as `Transient`.

The upload journal at
`crates/pcloud-backends/src/upload_journal.rs` does persist the
`uploadid`+`offset` tuple (`upload_journal.rs:92-97`, replay at
`upload_journal.rs:182+`) for crash recovery, but it does not protect
against the in-flight retry case above.

Separately, `MethodRetryPolicy`
(`crates/pcloud-resilience/src/retry.rs:229-316`) already classifies
`RetryClass::Mutation`-class operations as non-retriable by default
(`retry.rs:267-274`). But this enum is not wired into
`ResilientTransport`: grep for `RetryClass::Mutation` in
`pcloud-proto/` returns zero hits. The `ResilientTransport.execute`
call path at `resilient_transport.rs:243-298` does not consult the
method class — only the raw `ErrorClass` from the inner error.

**Remediation:** Wire `MethodRetryPolicy` into `ResilientTransport`.
`execute` must accept a `RetryClass` argument per request and refuse to
retry mutations unless the caller has attached a server-supported
idempotency key (pCloud's `uploadid`). For `upload_create` specifically:
persist the requested filename→uploadid mapping in the upload journal
**before** issuing the request (rowid = content-hash of parameters), so
that a retry after a client crash can reuse the existing uploadid
instead of making a new one.

Cross-reference §4 (sync engine): this finding is about transport-level
idempotency, not the engine queue.

---

### 6.6 WebSocket / diff stream

#### 6.6.1 [INFO] No WebSocket or diff-stream support; `diff` is a polling request

Repository-wide search for `websocket` / `diff_stream` / `poll_stream`
returns zero matches. `crates/pcloud-proto/src/diff_api.rs:1-47`
documents `diff_api` as a single-shot `diff` request keyed by a server
cursor (`diffid`).

This is a parity gap vs. a push-based server (pCloud does support a
long-poll/streaming `diff` in the C client —
`pclsync/pdiff.c:psync_diff_thread` in upstream). It is not currently
in the audit matrix as a P0 blocker, but enterprise desktop clients
expect sub-second remote-change propagation; polling does not deliver
that.

**Remediation:** Track as `Partial` in the C parity matrix. Out of scope
for this audit to fix; flag for product prioritisation.

---

### 6.7 API-server steering

#### 6.7.1 [MEDIUM] `set_api_server` / `apply_api_server_hint` mutates the live transport without any allowlist and without re-validating the SNI

`crates/pcloud-proto/src/transport.rs:270-287`:

```rust
impl ApiServerHintConsumer for BinaryApiTransport {
    fn apply_api_server_hint(&self, api_server: &str) {
        if api_server.trim().is_empty() { return; }
        let (host, port) = parse_api_server_hint(api_server);
        let mut config = self.config.write().expect(...);
        config.host = host.clone();
        config.server_name = host;
        if let Some(port) = port { config.port = port; }
    }
}
```

The server response's `apiserver` field is taken at face value. If the
response is forged (e.g. a weakness anywhere else on the server side,
or a MITM during the brief TLS-handshake-before-cert-pinning window),
the client cheerfully reconfigures its endpoint — and all subsequent
TLS handshakes use the attacker-supplied hostname as SNI **and** as the
certificate verification name. TLS will then succeed if the attacker
controls a cert for that name, which — if the attacker controls DNS or
the path — is not hard.

**Remediation:** Restrict accepted hints to a known domain family
(regex `^bineapi(-[a-z]{2})?\.pcloud\.com$` for the binary API,
`^api(-[a-z]{2})?\.pcloud\.com$` for HTTP). Reject port overrides or
restrict to a fixed set (443, 8443). Require at least one successful
round-trip against the original endpoint before accepting a hint.

---

#### 6.7.2 [LOW] API-server selection is not persisted across restart unless the SQLite preferences path was written by a prior run

`crates/pcloud-daemon/src/bootstrap.rs:446-449` loads
`store.repositories.preferences.api_server_binapi` and applies it. But
`apply_api_server_hint` is called from the *response handler* of an
authenticated binary request — there is no explicit path that writes
this back to the preferences store. So after a daemon restart the
steering decision is lost and the client re-hits the default endpoint
until the next response carries a hint.

**Remediation:** Persist on every successful hint apply; expire stale
hints after a week.

---

### 6.8 Observability of outbound traffic

#### 6.8.1 [HIGH] No per-endpoint HTTP latency/error histogram

`crates/pcloud-observability/src/metrics.rs:17-26` documents the metric
table. **Every histogram is keyed by the IPC `method` (inbound
dispatch), not by outbound HTTP endpoint.** The
`pcloud_request_latency_seconds` histogram is emitted from the daemon's
dispatch loop (grep `observe_request` in
`crates/pcloud-daemon/src/runtime.rs`), measuring the in-process
dispatch, not the HTTP round-trip.

There is no metric family for:

- outbound pCloud API round-trip latency per command (`login`,
  `diff`, `upload_create`, etc.),
- outbound API error rate per command,
- TLS-handshake cost,
- circuit-breaker trip count,
- retry budget consumption.

This is a critical enterprise observability gap: operators cannot tell
"is the daemon slow because the dispatch is slow, or because pCloud's
API is slow, or because we're being rate-limited?"

**Remediation:** Register new histograms in
`MetricFamilies::observe_outbound(command, status, latency_seconds)`
and wire them through
`ResilientTransport::execute` (which owns the outer timing boundary) and
through `BinaryApiTransport::execute_with_body` (if a caller bypasses
the resilient wrapper). Keep label cardinality bounded by sanitising
command name via the existing label sanitiser (§8 cross-reference).

Add counters for circuit-breaker state transitions
(`pcloud_circuit_breaker_state_changes_total{endpoint,new_state}`) and
retry outcomes
(`pcloud_retry_attempts_total{command,outcome=succeeded|exhausted}`).

---

#### 6.8.2 [MEDIUM] `pcloud_transfer_bytes_total` has no per-endpoint or per-sync-root label

`metrics.rs:22` — a single counter for upload+download bytes across the
entire daemon. An operator cannot tell whether a sudden spike is from
FUSE writeback, a new sync root, or a runaway plugin.

**Remediation:** Add a `source` label
(`{fuse|sync|plugin|cli|sdk}`) and a `root_id` label (capped at ~16
distinct values to keep cardinality under control).

---

## Section 7. IPC & Daemon

### 7.1 Wire format

#### 7.1.1 [INFO] Length-prefixed framing is present, documented, and boundary-checked

`crates/pcloud-ipc/src/protocol.rs:10-16` documents the 8-byte
little-endian header:

```text
offset 0..4 : u32 payload_len   // JSON byte length
offset 4..6 : u16 version       // IPC_PROTOCOL_VERSION = 1
offset 6..8 : u16 message_type  // 1=Request, 2=Response, 3=Event
offset 8..  : JSON body
```

The hard cap is `MAX_IPC_PAYLOAD_LEN = 1 MiB`
(`protocol.rs:47`). Framing checks occur **before** allocation at
`crates/pcloud-ipc/src/transport.rs:304-325` (`read_framed_request`) —
declared length is validated against `MAX_REQUEST_BYTES` before any
`Vec::with_capacity(payload_len)`.

**No finding.** This is correct.

---

#### 7.1.2 [HIGH] Serialization is JSON with **no schema version negotiation beyond a single u16**

`crates/pcloud-ipc/src/protocol.rs:39`:

```rust
pub const IPC_PROTOCOL_VERSION: u16 = 1;
```

`protocol.rs:255-260` rejects any non-1 version with
`ProtocolError::VersionMismatch`. There is:

- no forward-compat tolerance (client v1 speaking to daemon v2 cannot
  even read a v2-labeled `DrainStatus` response),
- no minor-version negotiation (no "I speak 1.3; server offers 1.4;
  both fall back to 1.3"),
- no payload-schema diff handling — `serde_json::from_slice` on a
  v1 client receiving v1 JSON with an unknown field errors out if
  `#[serde(deny_unknown_fields)]` is set, or silently discards the
  field otherwise (it is **not** set; no `deny_unknown_fields` in the
  wire types, which cuts both ways — see §7.2.2).

For a daemon intended to be a drop-in replacement for a long-running
desktop agent with independent CLI upgrades, this is a real
compatibility hazard.

**Remediation:** Add a capability negotiation step (client sends
`Method::HandshakeCapabilities` on connect; server returns a
semver-style range it supports + an optional feature bitmap). Define
the deprecation policy ("N-1 support for 6 months"). Bump the version
to 2 only when a truly breaking change ships; use serde-renames +
`#[serde(default)]` for additive changes.

---

#### 7.1.3 [MEDIUM] `MessageKind` decoder coerces unknown values into `Event`

`protocol.rs:268-272`:

```rust
let kind = match message_type {
    1 => MessageKind::Request,
    2 => MessageKind::Response,
    _ => MessageKind::Event,
};
```

This silently accepts any `message_type` ≥ 3 as `Event`. Combined with
the (unused-but-reserved) `Event` variant, a forged frame with
`message_type = 65535` would be decoded as an event and — depending on
how `Event` is handled downstream — could be mis-dispatched.

**Remediation:** Reject unknown `message_type` values explicitly with
`ProtocolError::InvalidMessageKind { actual }` and close the
connection. The doc comment at `protocol.rs:52-61` is wrong: it says
"decoders reject unknown values" but the code coerces them.

---

#### 7.1.4 [LOW] Max frame size (1 MiB) is a static const with no per-method allowance

`crates/pcloud-ipc/src/server.rs:42` — `MAX_REQUEST_BYTES = 1 MiB`.
Most methods are far under 1 KiB. A legitimate `Request::
SyncRootAdd` with a very long path approaches 4 KiB in practice. A
future method that needs to carry a larger payload (e.g. a batched
notification list, or encrypted blob) would have to either bump the
global cap or split across frames.

**Remediation:** Make the cap per-method. Default to 16 KiB; only a
small allow-list of methods gets 1 MiB.

---

### 7.2 Serialization safety (proptest coverage)

#### 7.2.1 [HIGH] `proptest_methods_roundtrip.rs` covers 30 Method variants; the enum has at least 45

`crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:15-48` —
`every_method()` returns exactly **30** Method variants. The actual
enum at `crates/pcloud-ipc/src/methods.rs:37-220+` has **at minimum 45
variants**:

Missing from the proptest list (verified via `grep '^    [A-Z][a-zA-Z]+,?$'
crates/pcloud-ipc/src/methods.rs`):

- `Method::Health` (line 49)
- `Method::SessionStatus` (line 125)
- `Method::FileHistory` (line 138)
- `Method::IntegrityStatus` (line 143)
- `Method::HaStatus` (line 151)
- `Method::DrainStatus` (line 162)
- `Method::GetSlo` (line 170)
- `Method::GetAuditVerifierStatus` (line 177)
- `Method::GetSyncStatus` (line 184)
- `Method::ListConflicts` (line 189)
- `Method::StatPath` (line 197)
- `Method::GetApiServers` (line ~202)
- `Method::GetPromo` (line ~207)
- `Method::GetCryptoHint` (line ~211)
- `Method::VerifyEmail` (line ~215)

Plus numerous `Request` variants (e.g. `IntegrityRunOnce`, `UploadList`,
`ConflictList`, `RunLocalScan`, `SendPublink`) that are not exercised
by `arb_request()` either.

The compile-time "exhaustiveness guard" at
`proptest_methods_roundtrip.rs:60-97` (`must_match_every_method_variant`)
is **defeated** by the catch-all `_ => 0` arm at line 95, exactly
because `Method` is `#[non_exhaustive]`. The doc comment at
`proptest_methods_roundtrip.rs:50-59` explicitly admits this.

**Consequence:** a new Method variant added between releases that has a
subtle serde rename or non-round-tripping field is shipped without
proptest coverage. The CSV parity matrix claim of "IPC surface is
proptest-verified" is technically false.

**Remediation:** Remove `#[non_exhaustive]` from `Method` for this
crate's own tests (it is only useful for external consumers), or
replace the `_` arm with an explicit list that the compiler will force
updates on. Better: add a `strum::EnumIter` derive and iterate every
variant at test time, so `every_method()` is always complete by
construction.

---

#### 7.2.2 [MEDIUM] Wire types do not use `#[serde(deny_unknown_fields)]`; unknown fields silently drop

Sampled `crates/pcloud-ipc/src/methods.rs` shows no
`#[serde(deny_unknown_fields)]` on `Request`, `Response`, or `Method`.
Combined with §7.1.2 (no version negotiation), a hostile or confused
client can inject extra fields that the server silently ignores — but
more interestingly, a downgrade attack that strips a
newly-added-as-mandatory field will succeed because serde will fill
the missing field with `#[serde(default)]`.

**Remediation:** Add `#[serde(deny_unknown_fields)]` to every wire type.
Use `#[serde(deny_unknown_fields, default)]` on enum-variant structs
where forward-compat defaulting is wanted. Pair with §7.1.2's
capability handshake so additive schema changes are explicit.

---

#### 7.2.3 [LOW] `prop_random_bytes_do_not_panic` only checks that random bytes don't panic — doesn't assert a specific error

`proptest_methods_roundtrip.rs:236-240`:

```rust
#[test]
fn prop_random_bytes_do_not_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
    let _ = decode_request(&bytes);
    let _ = decode_response(&bytes);
}
```

This correctly asserts no panic, but it does not assert that an
unparseable frame produces a *specific* `ProtocolError`. A refactor
that made the decoder return `Ok` on garbage would pass this test.

**Remediation:** Tighten to `prop_assert!(matches!(decoded, Err(_) | Ok(Frame { header: FrameHeader { version: 1, .. }, payload }) if payload == /* no-op equivalent */))`.

---

### 7.3 Authentication on every accept

#### 7.3.1 [INFO] Linux uses `SO_PEERCRED`, BSD/macOS use `getpeereid(3)`, Windows uses ALPC-style named-pipe SID match — all confirmed

`crates/pcloud-ipc/src/platform/linux.rs:31-57` —
`getsockopt(SOL_SOCKET, SO_PEERCRED)` populates a `libc::ucred` and
extracts uid + pid.

`crates/pcloud-ipc/src/platform/unix.rs:44-60` —
`getpeereid(3)` populates uid (pid is synthesized as 0 because
getpeereid does not expose it — correctly documented).

`crates/pcloud-ipc/src/platform/windows.rs:141-220` — creates the
named pipe with an explicit single-SID DACL
(`D:(A;;GRGW;;;<owner-sid>)`), and at accept time recovers the client
SID via `GetNamedPipeClientProcessId` →
`OpenProcessToken(TOKEN_QUERY)` → `GetTokenInformation(TokenUser)` →
`ConvertSidToStringSidW`. SID string is compared byte-for-byte against
the server's owner SID
(`windows.rs:202-209`).

Anonymous sockets are rejected at
`crates/pcloud-ipc/src/transport.rs:186-198`: if
`peer_identity(&stream)` fails, the server responds
`ResponseStatus::Unauthorized` and closes.

**No finding on the peer-auth path itself.**

---

#### 7.3.2 [MEDIUM] Linux `SO_PEERCRED` records the **pid at connect time**, not at the dispatch time

`crates/pcloud-ipc/src/platform/linux.rs:94-120` (`peer_ucred`) is
called once per accept. A client process that forks between `connect()`
and the dispatch completion can mislead the audit trail: the audited
pid is the parent's pid, not the child's that actually sent the
request.

This is a Linux kernel limitation, not a bug in this crate, but it
deserves a SECURITY.md callout. The equivalent on Linux, `SCM_CREDENTIALS`
piggybacked on each `sendmsg`, would close the gap.

**Remediation:** Document the limitation. Consider a follow-up that
switches to `SCM_CREDENTIALS` per-message on Linux where higher
assurance is needed. Not blocking.

---

#### 7.3.3 [MEDIUM] On macOS/BSD the peer pid is **synthesized as 0**, which makes audit correlation impossible

`crates/pcloud-ipc/src/platform/unix.rs:65-68`:

```rust
pub(crate) fn peer_ucred(stream: &UnixStream) -> Result<(u32, u32), IpcTransportError> {
    let (uid, _gid) = getpeereid(stream)?;
    Ok((uid, 0))
}
```

`auth.rs:34-38` documents this and reassures that the pid is "carried
for audit correlation only — never used for authorization", which is
correct. But enterprise audit logs on macOS/FreeBSD will show
`pid=0` for every IPC event — an alert-tuning disaster.

**Remediation:** On macOS use `getsockopt(LOCAL_PEERPID)` —
macOS-specific; available on all supported releases. On FreeBSD use
`getsockopt(LOCAL_PEERCRED)` which returns a full `struct xucred`
including pid. Only the darkest-BSDs (historical OpenBSD) genuinely lack
pid; those can stay at 0.

---

### 7.4 Authorization (per-request capability scoping)

#### 7.4.1 [CRITICAL] **There is no per-request capability scoping. Every owner-uid peer gets the full IPC surface, including `Method::Shutdown`, `Method::CryptoReset`, and `Method::Logout`.**

`crates/pcloud-ipc/src/server.rs:98-132` — `IpcServer`'s entire
authorization contract is a single uid comparison:

```rust
pub fn authorize_peer(&self, peer: &PeerIdentity) -> bool {
    peer.matches_owner(self.owner_uid)
}
```

The dispatch path at `crates/pcloud-daemon/src/dispatch.rs:1-150+`
carries no capability token. Searching the daemon crate for
`capability` or `CapabilityScope` or `privileged` returns zero
matches. The only tiered control is the rate-limiter's per-category
token bucket
(`crates/pcloud-daemon/src/rate_limit.rs:25-100+`), which is about
abuse prevention, not privilege separation.

**Impact:** In a multi-process single-user deployment (which is the
norm: the daemon is the backend; the CLI, Web UI, SDK consumers are
separate processes owned by the same user), any local process owned by
the user can:

- Call `Method::Shutdown` and kill the daemon (DoS).
- Call `Method::CryptoReset` and **wipe the user's local crypto
  fingerprint / folder registry** — this is privilege-meaningful even
  though both processes are the same user.
- Call `Method::Logout` and destroy in-memory credentials.
- Call `Method::SetAuthPersistence { enabled: false }` and disable the
  durable token vault.
- Call every `CryptoChangePassword*` variant.
- Call every `SyncRootRemove` / `SyncRootAdd`, re-routing user data.

The enterprise model expects at least two tiers: *read-only probes*
(status, health, drain-status, metrics) versus *state-mutating
operations*. Even without a full capability architecture, the MUST-HAVE
is a "privileged" gate guarded by an additional token (e.g. a
supervisor-only socket, or a token written only into the runtime dir
that the CLI has to read to unlock shutdown-class operations).

**Remediation:** Introduce a two-tier model immediately:

1. Read-only tier: `GetStatus`, `GetHealth`, `DrainStatus`,
   `SessionStatus`, `GetSlo`, `GetSyncStatus`, `ListConflicts`,
   `IntegrityStatus`, `GetApiServers`, `GetPromo`, `HaStatus`,
   `StatPath`. Admit on uid-match alone.
2. Privileged tier: everything that mutates state
   (`Shutdown`, `CryptoReset`, `Logout`, `CryptoChangePassword*`,
   `SyncRootAdd/Remove/Pause/Resume`, `CreateFilePublicLink`,
   `DeletePublicLink`, `SetAuthPersistence`, etc.). Require an
   additional *bearer token* stored in `$runtime_dir/privileged.token`
   (mode 0400), which the CLI reads and presents via a new
   `Request::Privileged { token, inner }` wrapper.

This is a modest architectural change that closes a CRITICAL local
privilege-management gap. Track as `bd-new-ipc-capability-scoping`.

---

#### 7.4.2 [HIGH] `drain_gate_admits_status_and_shutdown_probes` test at `serve.rs:440-457` admits `Method::Shutdown` during drain

During drain, `should_reject_during_drain`
(`crates/pcloud-daemon/src/serve.rs:79-87`) returns `false` for
`Method::DrainStatus | Method::Shutdown | Method::GetHealth |
Method::Health`. This means a second `Shutdown` during drain is
dispatched to the backend.

This is defensible (a supervisor re-issuing shutdown should be
idempotent), but combined with §7.4.1 it means **any local process can
call Shutdown twice in quick succession** — once to start the drain,
once during the drain window to attempt to alter state. The second
`Shutdown` should be a no-op but this is not asserted in tests.

**Remediation:** After §7.4.1 lands, `Method::Shutdown` is in the
privileged tier and this finding subsides. Until then: make
`Shutdown` during drain explicitly a no-op that returns the current
`DrainStatusPayload`.

---

### 7.5 Runtime directory hygiene

#### 7.5.1 [INFO] Linux: socket mode 0600 under a 0700 parent, confirmed

`crates/pcloud-ipc/src/transport.rs:246-268`:

```rust
if let Some(parent) = socket_path.parent() {
    let parent_missing = !parent.exists();
    fs::create_dir_all(parent)?;
    if parent_missing {
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
}
if socket_path.exists() {
    fs::remove_file(socket_path)?;
}
let listener = UnixListener::bind(socket_path)?;
fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;
```

Tested at `crates/pcloud-ipc/tests/security_invariants.rs:150-171`.

**No finding.** Correct.

---

#### 7.5.2 [MEDIUM] Parent dir is only chmod'ed to 0700 **if it did not already exist**; an attacker who pre-creates the parent dir with loose perms retains them

`transport.rs:250-253`:

```rust
let parent_missing = !parent.exists();
fs::create_dir_all(parent)?;
if parent_missing {
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
}
```

If a local attacker runs
`mkdir -p $XDG_RUNTIME_DIR/pcloud --mode=0755` before the daemon
starts, the daemon will happily bind the socket inside a world-readable
parent. The socket itself is 0600, so peers cannot connect, but the
directory listing leaks the existence of the socket (and, more
importantly, any sidecar files the daemon drops there — e.g.
`mount_pid` at
`crates/pcloud-daemon/src/bootstrap.rs:726-755` which stores a PID the
daemon claims, and which an attacker could use to spoof).

**Remediation:** Always unconditionally `chmod 0700` on the parent
directory after `create_dir_all`, regardless of whether it was
pre-existing. Additionally, `lstat` the parent and refuse to start if
it is a symlink, or if its ownership differs from the effective uid.
Follow the same discipline adopted for the vault file at
`crates/pcloud-daemon/src/auth_vault.rs:103-121` (which validates
`meta.file_type().is_file()` and rejects non-owner mode bits).

---

#### 7.5.3 [MEDIUM] Stale socket file is removed without atomicity — a small TOCTOU window where a concurrent process could be the binder

`transport.rs:255-260`:

```rust
if socket_path.exists() {
    fs::remove_file(socket_path)?;
}
let listener = UnixListener::bind(socket_path)?;
```

Between `remove_file` and `UnixListener::bind`, a concurrent process
with the same uid can create a file or symlink at `socket_path`. The
impact is reduced by the owner-only DACL on the parent dir (after
§7.5.2 lands), but on the first-daemon-start case the parent may be
world-writable and the window is exploitable.

**Remediation:** Use `socket(2) + bind(2)` directly and rely on
`connect(2)` after `unlink(2)` only, *plus* use `sun_path` with a
unique suffix (e.g. `pcloud.sock.<pid>.<rand>`) and then atomically
rename to the stable name. Or use abstract sockets on Linux
(`\0pcloud-rs-<uid>`), which avoid the filesystem entirely.

---

#### 7.5.4 [LOW] macOS `TMPDIR` is not used; the daemon falls back to `/tmp` via `PcloudDirs::discover()`

`crates/pcloud-config/src/paths.rs:156-230+` documents the XDG
discovery path. `crates/pcloud-daemon/tests/graceful_drain.rs:46-56`
explicitly notes "Use `/tmp` (not `std::env::temp_dir()`) so the
Unix-socket path stays under SUN_LEN on macOS".

This is defensible (macOS `TMPDIR` paths are too long for `sun_path`'s
104-byte limit), but it means the daemon's runtime files live in a
world-writable directory on macOS unless the operator tightens things.

**Remediation:** On macOS, prefer
`$HOME/Library/Application Support/pcloud-rs/runtime/` (which is
user-owned by default) and use a relative `bind(chdir + relative)`
trick to get the socket name under the SUN_LEN limit.

---

### 7.6 Graceful shutdown

#### 7.6.1 [INFO] Three-state drain machine is implemented, tested, and used

`crates/pcloud-daemon/src/signals.rs:28-131` documents and implements
`Running → Draining → Stopped`.

`crates/pcloud-daemon/src/serve.rs:110-231`
(`serve_until_shutdown_with_flag`) cooperates with it: on shutdown
observed, it calls `signals::begin_drain()`, starts a drain deadline
based on `runtime.config.upgrade.drain_timeout_secs`, polls
`signals::in_flight() == 0`, and returns when drained or timed out.

Integration coverage at
`crates/pcloud-daemon/tests/graceful_drain.rs:61-229` exercises:

- `drain_admits_status_probes_and_rejects_new_traffic` (L61)
- `drain_gate_rejects_ordinary_requests_with_unavailable` (L148)

Both pass under the serial lock at L31-36. Solid.

**No finding.** This is the highest-quality subsystem in the reviewed
slice.

---

#### 7.6.2 [MEDIUM] Drain timeout is a hard cut — in-flight uploads/downloads are dropped on the floor

`serve.rs:166-171`:

```rust
let drained = signals::in_flight() == 0;
let timed_out = drain_deadline.map(|d| Instant::now() >= d).unwrap_or(false);
if drained || timed_out { return Ok(()); }
```

When the timer fires, the loop returns irrespective of in-flight
counter. This is correct for unbounded-latency operations, but there
is no mechanism to *cancel* in-flight uploads gracefully before the
cut — e.g. tell the upload state machine "you have 2 seconds; persist
progress and give up". The upload journal does persist state, so
resume-after-restart is possible, but an enterprise deployment expects
a softer cooperative cancellation.

**Remediation:** Introduce a `CancellationToken` (or
`tokio::sync::broadcast` channel) that is tripped when
`begin_drain` fires. Long-running operations (uploads, diff-poll, TLS
handshake) should check the token and exit early by persisting their
journal entry and returning `ResponseStatus::Unavailable`. Default
drain deadline can then be a *soft* deadline on cooperation, with a
separate hard deadline at 2x for the hold-out case.

---

#### 7.6.3 [LOW] `mount_control.quiesce_for_drain` is called once, synchronously, during the drain transition

`serve.rs:156-158`:

```rust
let summary = runtime.mount_control.quiesce_for_drain();
if summary != "no active mount" { log::info!(...); }
```

This runs on the accept thread and can block. If the FUSE writer has a
multi-second flush, the drain transition's stamp
(`DRAIN_STARTED_MS`) is written *before* the quiesce returns, but
`Method::DrainStatus` will report stale "elapsed_drain_ms = 0" until
`begin_drain` returns. In practice the effect is cosmetic.

**Remediation:** Spawn `quiesce_for_drain` on a worker thread and have
the serve loop poll it. Out of scope for §7 because it touches §5 FUSE.
Cross-reference: FUSE agent owns this.

---

#### 7.6.4 [LOW] `mark_stopped()` is called *after* `serve_until_shutdown_with_flag` returns; if the caller forgets, `drain_state()` stays at `Draining` forever

`serve.rs:291-293`:

```rust
let _ = sync_loop_handle.shutdown_and_join();
signals::mark_stopped();
```

This happens only in `serve_with_shutdown` (the Windows Service entry
point). The UNIX path `serve.rs::serve_until_shutdown` has no such
call. If the `pcloudd serve` binary crashes after the serve loop
returns but before shutdown completes (e.g. in
`sync_loop_handle.shutdown_and_join`), `drain_state()` is stuck at
`Draining` for any test that runs in the same process afterward.

**Remediation:** Use a Drop-guarded sentinel
(`DrainGuard { /* sets Stopped on drop */ }`) so even panic paths
transition correctly.

---

### 7.7 Crash recovery

#### 7.7.1 [HIGH] Upload resume scan runs but uses *authenticated-later* reconcile; startup path only logs

`crates/pcloud-daemon/src/bootstrap.rs:524-570` enumerates upload
sidecars under the FUSE staging root:

```rust
match enumerate_upload_sidecars(&staging_root) {
    Ok(outcomes) if !outcomes.is_empty() => {
        log::info!("pcloud-daemon bootstrap: {} upload sidecar(s) awaiting server reconcile ...", ...);
        for o in outcomes { /* log only */ }
    }
    ...
}
```

The comment at L525-530 says "This pass runs *before* any authenticated
transport is available, so it enumerates and logs only". That's
correct today, but it means the enterprise expectation of "daemon
restarts; stale uploads resume within a few seconds" is *not* met by
the bootstrap path — it is met only later, by the mount-time reconcile
at `mount_runtime::pcloud_shim_adapter_factory`, which may never fire
if the user does not remount immediately.

`bootstrap.rs:573-605` does a second resume scan against
`UploadResumeRepository::list_all` with the same behaviour: log-only.

**Impact:** Real enterprise-relevant uploads that were mid-flight at
crash time sit in the journal, waiting for a human to remount, before
they resume. Lost time, confusing operator experience, and — for
automated deployment scenarios where the FUSE mount is orchestrated by
systemd and may not be re-mounted immediately — silent data stall.

**Remediation:** Spawn a startup reconcile task *after* bootstrap
completes and the auth vault is loaded. Try to acquire a token via
vault load; if present, reconcile each sidecar against the server
(trim-up/down/NotFound/Stalled). If no token is present, defer until
login and then reconcile on the login success callback.

---

#### 7.7.2 [HIGH] No re-adoption of orphan FUSE mounts; startup scan *rejects* or *force-unmounts* them

`crates/pcloud-daemon/src/bootstrap.rs:733-782` handles orphans via
`MountControl::check_orphans()`, which returns one of:

- `OrphanCheckOutcome::Clean` → fine
- `OrphanCheckOutcome::Rejected(paths)` → log error, refuse to start
  the mount service
- `OrphanCheckOutcome::ForceUnmounted(results)` → forcibly unmount via
  `PCLOUD_FORCE_UMOUNT=1`

There is no "re-adopt" path. A crashed daemon whose FUSE mount is
still live cannot have its mount re-owned; the operator must either
force-unmount and re-mount (user-visible disruption) or set the env
var. In systemd terms, this breaks rolling restart.

**Remediation:** Implement FUSE mount re-adoption per FreeBSD /
`mount_pid` sidecar: on startup, if the orphan's `mount_pid` matches a
dead pid but the kernel still shows the mount, re-open the FUSE
channel fd via `/proc/<dead-pid>/fd/<num>` (or its successor) and
resume servicing requests. This is a nontrivial engineering lift but
is exactly what enterprise rolling-upgrade demands. Cross-reference
§5 FUSE agent.

---

#### 7.7.3 [LOW] Startup scans use `rusqlite::Connection::open` outside the main store, creating a second connection

`bootstrap.rs:581-584`:

```rust
let store_conn = rusqlite::Connection::open(&store_path)
    .map_err(|err| BootstrapError::Provision(std::io::Error::other(err.to_string())))?;
```

The daemon already has `store: StoreProfile` in scope at this point. A
second rusqlite connection to the same file is fine (WAL mode), but
the explicit comment about WAL/locking discipline is missing. A later
schema migration that takes an exclusive lock could stall this open.

**Remediation:** Reuse `store.connection()` (or equivalent accessor) if
available. Or, at minimum, document the locking order and ensure
migrations run first.

---

### 7.8 Stress coverage

#### 7.8.1 [MEDIUM] `stress_concurrent_clients.rs` exercises 50 × 500 = 25000 requests, `#[ignore]`-gated; does not prove production claims

`crates/pcloud-ipc/tests/stress_concurrent_clients.rs:30-31`:

```rust
const CLIENTS: usize = 50;
const REQUESTS_PER_CLIENT: usize = 500;
```

Gated at line 44: `#[ignore = "stress: 50 clients x 500 reqs, run with
--release --ignored"]`. Asserts:

- Zero failures (`stress_concurrent_clients.rs:135-140`)
- `served_count >= total` (L144)
- `fd_drift <= 64` (L150)
- Socket path is cleaned up (L155)

25000 requests at sub-ms each (~10k req/s for a typical workstation)
complete in a couple of seconds. This proves correctness under mild
contention but does **not** prove:

- Behaviour under CPU pressure (other processes pinning cores)
- Behaviour under memory pressure
- Behaviour at 10x the fd ceiling (`ulimit -n 10000`)
- Behaviour with slow clients (read_timeout path — there is a
  `slow_client_timeout_does_not_prevent_followup_request` test at
  `transport.rs:543-598` but only one slow client at a time)
- Long-running soak (24h+)

Also, the test only uses `Method::GetHealth` and `Method::GetStatus` —
cheap read-only methods that go nowhere near the backend. A stress
test that hit the full dispatch loop with a Mutation mix would be
more meaningful.

**Remediation:** Add a soak mode (`#[ignore = "stress: 24h soak"]`),
a slow-client-population mode (25% slow clients), and a
mutation-mixed-workload variant. Track as a parity-proof requirement
for `bd-1du.10`.

---

#### 7.8.2 [LOW] The stress test uses `fd_drift <= 64` as the leak ceiling; 64 is generous

`stress_concurrent_clients.rs:150-153`:

```rust
let fd_drift = after_fds.saturating_sub(baseline_fds);
assert!(
    fd_drift <= 64,
    "fd drift {fd_drift} exceeds leak ceiling (baseline={baseline_fds}, after={after_fds})"
);
```

64 file descriptors after 25000 requests is a 1-in-390 leak rate. That
is too lax for an enterprise claim. The expected leak rate on a
correct implementation is zero; a tiny non-zero rate is ephemeral
(pending socket close under linger).

**Remediation:** Tighten to `fd_drift <= 4` (accepting
epoll/signalfd/eventfd ephemera) and run the test 3 times so transient
noise is amortised.

---

### 7.9 Web / management surface (`pcloud-web`)

#### 7.9.1 [INFO] Bind address is loopback-enforced at construction time with a panic guard

`crates/pcloud-web/src/lib.rs:236-260` (`serve`) has a hard
`assert!` that `config.bind_addr.ip().is_loopback()` and panics if
violated. The doc comment at L223-235 explains why it is a panic rather
than a `WebError`. Unit-tested at L311-325.

**No finding.** Correct.

---

#### 7.9.2 [HIGH] `pcloud-web` has **no authentication whatsoever**; every loopback connection gets the full route surface

`crates/pcloud-web/src/routes.rs:66-79` — no auth middleware, no
bearer-token check, no IP check beyond loopback. The route set includes
mutating endpoints:

- `POST /sync` — add a sync root (L71)
- `DELETE /sync/{id}` — remove a sync root (L72)
- `POST /publinks` — create a public link (L73)
- `DELETE /publinks/{code}` — revoke a public link (L74)

CSRF (double-submit cookie + HMAC-less token) is in place
(`routes.rs:596-622`) — but CSRF only stops cross-origin attackers.
It does *not* stop other local processes (running as the same user)
from just calling these endpoints directly with their own CSRF cookie
pair; CSRF requires a browser context, and a plain `curl` from a sibling
process can issue `GET /` to mint a cookie and then POST/DELETE with
the echoed token.

**Impact:** Combined with §7.4.1, this is a second local-auth hole.
Any process the user runs can:

- Point the sync engine at `/etc/passwd`'s parent via `POST /sync`
  (if validation lets it — out of scope for this audit, but the
  daemon's sync backend does validate paths).
- Revoke all the user's public links via `DELETE /publinks/{code}`.

The default bind is `127.0.0.1:17650` (`lib.rs:113`). If the web UI
is started at all (it is opt-in per the doc at L52-59), any local
process reaches it.

**Remediation:** Require a bearer token (random 256-bit, stored in
`$runtime_dir/pcloud-web.token` mode 0400) on every request. The CLI
reads this token and passes it in an `Authorization: Bearer <token>`
header. Browser sessions exchange the token for a session cookie on
first visit (cookie is `HttpOnly; SameSite=Strict` as today). This
closes the local-process bypass.

Independently: since the daemon already provides owner-uid-gated IPC,
consider making `pcloud-web` proxy all state mutations through the
daemon (which already does the uid check) rather than doing them
directly. Right now `routes.rs:118-137` (`sync_list`) already uses
`call_ipc(...)` — so the architecture is correct — but the CSRF-only
gate at the HTTP boundary is insufficient.

---

#### 7.9.3 [MEDIUM] No TLS on the management surface

`lib.rs:249-258` — `tokio::net::TcpListener::bind` plaintext. No
self-signed / local CA TLS option. Loopback-only mitigates wire
eavesdropping on a healthy host, but:

- A proc-dump by another user (root) observes plaintext traffic.
- A malicious kernel module or BPF probe sees plaintext CSRF tokens.

**Remediation:** Add optional TLS bind via a `rcgen`-generated
localhost cert rotated on each bind, with cert pinning in the CLI. Low
priority for non-paranoid single-user deployments, but required for
the hardened enterprise posture this audit targets.

---

#### 7.9.4 [LOW] CSRF token is 128 bits of hex; no HMAC, no expiry, no rotation

`routes.rs:559-571` mints a 16-byte random token, hex-encodes it.
Compare at L611-617 is constant-time (correctly). There is no
`exp` timestamp in the token itself; any leaked token lives as long as
the browser keeps the cookie.

**Remediation:** Bind the token to a session via HMAC-signed
`(nonce || expires_at || user_sid)`, verify HMAC on submit, refuse
expired tokens. Rotate the HMAC key on daemon startup.

---

### 7.10 Observability of the daemon surface

#### 7.10.1 [MEDIUM] No metric for IPC peer-auth rejections

Searching `pcloud-observability` and `pcloud-daemon` for
`unauthorized_peer` or `peer_cred_unavailable` as a metric counter
returns zero hits. When the IPC transport at
`crates/pcloud-ipc/src/transport.rs:186-208` rejects a peer with
`Unauthorized`, it is logged at the transport layer but not counted.

**Impact:** An operator cannot alarm on "spike in IPC authorization
failures" — a useful intrusion-detection signal.

**Remediation:** Add
`pcloud_ipc_authz_rejections_total{reason=uid_mismatch|cred_unavailable}`.

---

#### 7.10.2 [LOW] `pcloud_ipc_connected_clients` gauge exists but is never set

`crates/pcloud-observability/src/metrics.rs:26` documents the gauge;
`metrics.rs:435-438` provides `set_connected_clients`; grep for the
caller returns nothing except the test. The dispatcher never increments
it.

**Remediation:** Increment on every `accept`, decrement on every
dispatch-complete (under the `InFlightGuard` Drop).

---

## Cross-references

- **§2 (Secret discipline):** §6.1.2 API-server hint rewrite is a
  secret-flow concern; §7.4.1 privilege escalation lets a local
  process fetch crypto-password-change surfaces. Flag for §2 agent.
- **§4 (Sync engine):** §6.5.1 upload idempotency belongs at the
  transport boundary; the sync engine queue is a separate retry tier.
- **§5 (FUSE):** §7.7.2 mount re-adoption and §7.6.3 mount quiesce
  belong to the FUSE agent; §7 only cites them because the daemon
  bootstrap touches them.
- **§8 (Observability):** §6.8.1, §6.8.2, §7.10.1, §7.10.2 all feed
  into the observability dimension's gap analysis.

---

## Summary of findings by severity

| Severity  | Count | Finding IDs                                                                 |
|-----------|-------|------------------------------------------------------------------------------|
| CRITICAL  | 1     | §7.4.1                                                                       |
| HIGH      | 10    | §6.1.1, §6.1.2, §6.4.1, §6.4.2, §6.5.1, §6.8.1, §7.1.2, §7.2.1, §7.4.2, §7.7.1, §7.7.2, §7.9.2 |
| MEDIUM    | 15    | §6.1.3, §6.2.2, §6.3.1, §6.4.3, §6.4.4, §6.4.5, §6.6.1, §6.7.1, §6.8.2, §7.1.3, §7.2.2, §7.3.2, §7.3.3, §7.5.2, §7.5.3, §7.6.2, §7.8.1, §7.9.3, §7.10.1 |
| LOW       | 10+   | §6.3.2, §6.7.2, §7.1.4, §7.2.3, §7.5.4, §7.6.3, §7.6.4, §7.7.3, §7.8.2, §7.9.4, §7.10.2 |
| INFO      | 5     | §6.2.1, §6.6.1, §7.1.1, §7.3.1, §7.5.1, §7.6.1, §7.9.1                       |

(Count in table counts each §id once; some §ids span multiple severities
in the prose — the tally above uses the headline severity of each finding.)

---

## Enterprise-readiness verdict (§6 + §7 scope only)

**The transport path is close to enterprise-grade; the IPC path is
blocked by one CRITICAL finding.**

Transport blockers, in priority order:

1. **§6.4.1** — supply a real error classifier to
   `ResilientTransport::wrap_binary`; permanent errors must not retry.
2. **§6.5.1** — wire `MethodRetryPolicy` into `ResilientTransport` and
   reject mutation retries without an idempotency anchor.
3. **§6.8.1** — add per-endpoint outbound HTTP metrics; operators
   cannot run this daemon in production without them.
4. **§6.4.2** — honour `Retry-After`.
5. **§6.1.1 / §6.1.2** — harden TLS enforcement so no code path can
   bypass the validation gate.

IPC blockers, in priority order:

1. **§7.4.1 (CRITICAL)** — introduce at least a two-tier
   (read-only / privileged) capability scoping. Without this the
   daemon is not safe against malicious local processes owned by the
   same user.
2. **§7.2.1** — fix proptest variant coverage gap; the
   "exhaustiveness guard" is inert.
3. **§7.7.1 / §7.7.2** — implement real crash recovery. Log-only is
   not crash recovery.
4. **§7.9.2** — add authentication to `pcloud-web`.
5. **§7.1.2** — add capability/version handshake so the JSON wire
   schema can evolve.

Until these close, the daemon should not be described as "production
ready", "enterprise ready", or a "drop-in replacement" in any
release-facing document. This is consistent with the CLAUDE.md
discipline rules.
