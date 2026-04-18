# Section 7: IPC & Daemon
## Date: 2026-04-17
## Auditor: Claude Opus (Agent 7)

Scope: `crates/pcloud-ipc/`, `crates/pcloud-daemon/src/`, `crates/pcloud-web/`.
Read-only review; no source files were modified.

---

## Findings

### CRITICAL [0]

No defects rated CRITICAL were found. All the classically-catastrophic IPC
threats (peer-uid bypass, unbounded allocation from length prefix, plaintext
secrets on the wire, unauthenticated mutating web routes) are demonstrably
mitigated.

### HIGH [6]

#### H-1 Proptest `must_match_every_method_variant` has a wildcard escape hatch (line 95) — exhaustiveness is NOT enforced
- File: `crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:61-97`
- The doc comment at line 54-60 promises a "compile-time exhaustive match
  [that] forces the test to be updated whenever a new `Method` variant is
  introduced." The code at line 95 is `_ => 0,` which makes this a
  trivially-satisfied match — any new `Method` variant will silently fall
  through. The `#[non_exhaustive]` attribute on the enum (methods.rs:36)
  requires the catch-all arm to compile, so the only way to make the guard
  meaningful is to split explicit matches per variant (no `|`-joining) and
  omit the wildcard, forcing the compiler to emit a non-exhaustive-pattern
  error on a new variant inside the same crate, then add a wildcard only
  for forward compat when the match is re-exported.
- Concrete numbers: 45 `Method` variants exist (counted at methods.rs:37-216);
  the sample list `every_method()` at lines 16-48 enumerates 30; the match
  at 62-96 enumerates 31. New variants (e.g. `Method::GetSlo`,
  `Method::IntegrityStatus`, `Method::HaStatus`, `Method::DrainStatus`,
  `Method::GetAuditVerifierStatus`, `Method::GetSyncStatus`,
  `Method::ListConflicts`, `Method::StatPath`, `Method::GetApiServers`,
  `Method::GetPromo`, `Method::GetCryptoHint`, `Method::VerifyEmail`,
  `Method::FileHistory` — 13 variants) are NOT exercised by
  `every_method_variant_round_trips` or by the arb_method proptest.
- Request coverage is worse: 81 `Request` variants defined
  (methods.rs:262-1032), but `arb_request()` at lines 145-200 generates
  only 24. Variants such as `Request::Mount`, `Request::Unmount`,
  `Request::MountForceUnmount`, `Request::CreateTreePublicLink`,
  `Request::BackupSnapshot`, `Request::AuditVerifyChain`,
  `Request::UploadCreate`, `Request::StatPath`, `Request::DeleteBackup`,
  `Request::SetApiServer`, `Request::AccountRegister`,
  `Request::AccountChangePassword`, `Request::LostPassword`,
  `Request::VerifyEmailRestricted`, `Request::DownloadFile`,
  `Request::GetFileLink`, `Request::FilesystemStatus`,
  `Request::IntegrityRunOnce`, `Request::IntegritySkip`,
  `Request::ConflictList`, `Request::ConflictResolve`, and the full
  share / account-modify / crypto-change-password family — none are
  proptest-covered for round-trip stability.
- Remediation: remove the `_ => 0` arm and split `|`-joined arms into
  per-variant matches so the compiler fails on any new variant until the
  reviewer extends both the match and `every_method()` / `arb_request()`
  lists. Alternatively, use the `enum-iterator` crate or a `#[derive]`
  macro that yields `Method::ALL`/`Request::ALL` at compile time.

#### H-2 No `AuthAttempt` rate-limit category — `LoginBegin`/`SubmitPassword`/`SubmitTwoFactorCode` are medium-bucketed together with ordinary traffic
- File: `crates/pcloud-daemon/src/rate_limit.rs:192-211`
- `categorize_plain` buckets all non-classified methods into `Medium`. The
  auth methods (`Method::LoginBegin`, `Method::SubmitPassword`,
  `Method::SubmitTwoFactorCode`, `Method::SendTwoFactorSms`,
  `Method::SendTwoFactorNotification`) end up in the default arm. The
  structured variants that actually carry the secret material
  (`Request::PasswordSubmission`, `Request::AuthTokenSubmission`,
  `Request::TwoFactorCodeSubmission`) also default to `Medium`
  (rate_limit.rs:174-189).
- Because the socket is already owner-only (mode 0600, peer-uid match),
  the abuse surface is a compromised local process in the owner's
  context, not a network attacker. Still, there is no separate
  `AuthAttempt` bucket to slow down credential-stuffing / TFA-brute-force
  by a misbehaving local subagent, and the audit did not find a stronger
  gate elsewhere (no cooldown after N failed submissions, no exponential
  backoff). The `ResponseStatus::Unauthorized` is returned in constant
  latency with no jitter.
- Remediation: introduce `RateCategory::AuthAttempt` and map all
  password / TFA / recovery-code submissions to it with tight limits
  (e.g. 5 attempts / 60 s, refill 1/60 s). Pair with a per-session
  stall after N consecutive `Unauthorized` responses.

#### H-3 No sd_notify(3) READY=1 / WATCHDOG=1 — daemon cannot integrate with systemd `Type=notify` or watchdog timeout
- File: `packaging/systemd/pcloudd.service:12-18`, `crates/pcloud-daemon/src/main.rs:82-196`, `crates/pcloud-daemon/src/serve.rs:259-296`
- The unit file itself acknowledges this: "This unit uses Type=simple,
  not Type=notify, because the daemon does not currently emit
  sd_notify(3) READY=1 messages." No `sd_notify` / `sd_watchdog_enabled`
  / `NOTIFY_SOCKET` usage exists anywhere in the codebase (confirmed by
  grep across daemon and crates — only docs mention it).
- Consequences: (a) systemd starts follow-up units as soon as the
  `ExecStart` process exec's, before the IPC socket is bound — a race
  window where dependents can try to connect and get `ECONNREFUSED`;
  (b) `WatchdogSec=` cannot be used to detect a hung serve loop, so a
  livelock inside `serve_until_shutdown` is only noticed via external
  probes; (c) `STOPPING=1` is not sent at drain start, so the service
  manager's dashboard doesn't reflect in-progress shutdown; (d)
  `RELOAD=1` / `RELOADING=1` are not sent when SIGHUP triggers
  `config_reload::try_reload`.
- Remediation: add a dependency-free `sd_notify` helper (writing
  `READY=1\n`, `STOPPING=1\n`, `WATCHDOG=1\n`, `RELOADING=1\n` to the
  `NOTIFY_SOCKET` datagram socket when the env var is set) and call it
  from `main::run` (after `IpcServer::bind`), from `signals::begin_drain`,
  from a watchdog ticker inside the serve loop, and from the SIGHUP
  reload handler. Flip the unit to `Type=notify`.

#### H-4 Single-threaded IPC serve loop — one slow or mis-scheduled request blocks every other peer for up to 5 seconds
- File: `crates/pcloud-ipc/src/transport.rs:167-230`, `crates/pcloud-daemon/src/serve.rs:205`
- `BoundIpcServer::serve_once` accepts one connection, handles one
  request, writes the response, then returns. The daemon calls it in a
  tight loop (`serve.rs:205`). There is no worker pool, no per-connection
  thread spawn, no async dispatch. The 5-second
  `IPC_REQUEST_READ_TIMEOUT` (transport.rs:32) bounds the worst-case
  head-of-line blocking, but any dispatch that takes longer than a few
  ms (backend HTTP call to pCloud, SQLite transaction, crypto setup,
  integrity sweep, audit verify) stalls every other client.
- The stress test at `pcloud-ipc/tests/stress_concurrent_clients.rs`
  demonstrates 50×500 sequential reqs work, but only because each
  request is `GetHealth`/`GetStatus` (a format! over an atomic counter).
  Representative workloads (auth round-trips, public-link listing hitting
  the live API, crypto unlock hitting `argon2id`) will serialize.
- Remediation: convert the accept loop to spawn a worker thread per
  accepted connection (bounded by a `Semaphore` for max concurrency), or
  migrate to `tokio::net::UnixListener` with `spawn` per connection and
  an async `RuntimeShell` wrapped in `tokio::sync::Mutex`. At minimum,
  bound the accept-to-dispatch latency with a queue metric so the
  single-thread nature is observable.

#### H-5 `Method::Shutdown` has no additional authorization — any process running as the owning UID can terminate the daemon
- File: `crates/pcloud-daemon/src/runtime.rs:2489-2492`, `crates/pcloud-daemon/src/serve.rs:79-87`
- `request_shutdown` simply flips `self.control.shutdown_requested = true`
  after passing peer-uid check. The drain gate at `serve.rs:79-87`
  explicitly admits `Method::Shutdown` during drain as well, so a
  misbehaving process can repeatedly call it without rate limiting (the
  auto-categorization is `Medium`, rate_limit.rs:209).
- While same-UID implicit trust is Unix-standard, enterprise deployments
  typically want an additional capability gate (e.g. a shutdown-only
  token staged at startup, a signed request over the IPC, or a config
  option `ipc.shutdown_requires_capability = true`). Today a desktop
  notifier, a shell one-liner, or a browser extension running in a
  sandbox that leaks same-UID IPC access can kill the daemon.
- Remediation: gate `Method::Shutdown` behind a one-time capability
  token (generated at daemon startup, written to
  `$XDG_RUNTIME_DIR/pcloud/shutdown.cap` mode 0600, scanned by `pcloudc
  drain --yes`), OR require that the request carries a fresh nonce
  signed against a keypair the daemon minted at boot. At minimum,
  categorize `Shutdown` as `Expensive` so the per-session bucket caps
  repeated attempts.

#### H-6 Web `/health` is a constant "ok" literal, not an IPC liveness probe — supervisors get lies
- File: `crates/pcloud-web/src/routes.rs:88-91`
- `async fn health() -> impl IntoResponse { (StatusCode::OK, "ok") }`
  never consults the daemon. The route comment ("liveness probe (no
  IPC)") acknowledges this is intentional, but it means an orchestrator
  probing the web UI (Kubernetes `httpGet` on `/health`) will report the
  service healthy even when the daemon IPC socket is broken, the store
  is unreachable, crypto is locked, or the mount is in error. There are
  no `/livez` or `/readyz` endpoints at all — only `/health` and `GET /`
  (which renders an HTML page).
- Remediation: add three endpoints — `/livez` (trivial HTTP 200,
  process-alive only), `/readyz` (round-trips `Method::Health` via IPC
  and returns 503 on error / non-Ok), `/health` (keep as is for
  backward compat or deprecate). Document the difference per
  Kubernetes probe conventions.

### MEDIUM [8]

#### M-1 Serve loop swallows IPC accept errors other than `Interrupted`/`WouldBlock`/`TimedOut` as `Err(other) => return Err(other)` — a single `PermissionDenied` crashes the whole daemon
- File: `crates/pcloud-daemon/src/serve.rs:212-220`
- Any accept-time I/O error that isn't in the allow-list (e.g.
  `ENFILE`/`EMFILE` exhaustion, `EPROTO`, `ECONNABORTED` on some kernels)
  propagates out of the serve loop and terminates `main::run` with a
  stringified error. A transient fd-table-full condition should be
  retried with backoff, not treated as a terminal failure.
- Remediation: add a backoff (e.g. 100ms–5s) for recoverable kinds and
  a metric (`pcloud_daemon_accept_error_total{kind="..."}`) so operators
  see the transient pressure.

#### M-2 `Method::DrainStatus` and `Method::HaStatus` fall into the `Medium` bucket — supervisors polling during drain can be throttled
- File: `crates/pcloud-daemon/src/rate_limit.rs:192-211`
- Neither method is listed under the `Cheap` arm. A k8s/systemd liveness
  probe polling `DrainStatus` every 1 s during a long drain (drain
  timeout = `upgrade.drain_timeout_secs`, which can be minutes) will
  exhaust the `Medium` bucket and receive
  `ResponseStatus::Conflict("rate limit exceeded: medium, retry after Ns")`.
  That is exactly the observability surface you want ALWAYS available.
- Remediation: add `Method::DrainStatus | Method::HaStatus |
  Method::GetSyncStatus | Method::GetAuditVerifierStatus |
  Method::GetSlo | Method::IntegrityStatus` to the `Cheap` arm (they
  already short-circuit without heavy work in runtime.rs).

#### M-3 Drain-start stamp `DRAIN_STARTED_MS` can be 0 after signal-driven draining if the serve loop hasn't yet observed the flag
- File: `crates/pcloud-daemon/src/signals.rs:259-273`, `crates/pcloud-daemon/src/serve.rs:144-164`
- The SIGTERM handler flips `DRAIN_STATE` to `Draining` but cannot call
  `SystemTime::now()` (not async-signal-safe). Between the SIGTERM
  arriving and the serve loop running `begin_drain`, a concurrent
  `Method::DrainStatus` probe will see `state=draining` but
  `elapsed_drain_ms=0`. The serve loop compensates at lines 160-164
  ("still need a deadline") but a client probe that races the loop's
  first wake-up may see an inconsistent snapshot.
- Remediation: either fail `DrainStatus` with a brief "draining,
  bookkeeping not yet initialized" when `DRAIN_STARTED_MS == 0` while
  `DRAIN_STATE == Draining`, or have the SIGTERM handler CAS a magic
  sentinel (e.g. `u64::MAX`) and let the first reader replace it with
  the real wall-clock stamp.

#### M-4 SIGHUP handler installs via `install_handler` without `SA_RESTART`, but the handler only sets an atomic flag — the accept loop interprets EINTR correctly, but any other in-flight syscall in a backend will ALSO get EINTR and may fail noisily
- File: `crates/pcloud-daemon/src/signals.rs:279-308`
- The choice to NOT set `SA_RESTART` is documented and correct for
  `accept(2)` on the serve thread. But the backend dispatch path runs
  on the same thread (single-threaded serve loop). If a backend is in
  the middle of a libc read/write (e.g. SQLite syscall during a SIGHUP)
  the syscall aborts with `EINTR` and the backend may surface that as
  `InternalError` instead of retrying.
- Remediation: either install a dedicated signal-handling thread using
  `signalfd(2)` on Linux so the main thread never observes EINTR, or
  audit every long-running backend path to ensure EINTR is wrapped in
  a retry.

#### M-5 `generate_web_token` prints the token to stderr; it is never persisted; operator misses or the terminal scrolls and mutating routes become unusable without a daemon restart
- File: `crates/pcloud-web/src/lib.rs:122-130`, `crates/pcloud-web/src/lib.rs:279`
- `eprintln!("[pcloud-web] auth token: {}", config.web_token)` prints
  once at startup. There is no secure token-retrieval IPC method, no
  `$XDG_RUNTIME_DIR/pcloud/web_token` file, and no way to rotate without
  restarting the daemon. Under `journalctl --user -u pcloudd.service`
  the token is logged in plaintext into the systemd journal
  (persistable).
- Remediation: write the token to
  `$XDG_RUNTIME_DIR/pcloud/web_token` (mode 0600) atomically via
  rename, don't print it to stderr at all (or print only a short-lived
  one-time URL with the token embedded). Add a rotation IPC method.

#### M-6 Peer-credential-unavailable path reads the framed request before responding — a malicious peer that can't produce credentials can still waste up to `MAX_REQUEST_BYTES` of read budget
- File: `crates/pcloud-ipc/src/transport.rs:186-198`
- When `peer_identity(&stream)` fails, the code calls
  `let _ = read_framed_request(&mut stream)` before writing the
  `Unauthorized` response. The size cap is enforced inside
  `read_framed_request`, so there's no memory exhaustion, but a slow
  peer can tie up the serve thread for the full 5s read timeout after
  already failing the auth check. Combined with H-4 this amplifies
  head-of-line blocking.
- Remediation: drop the `read_framed_request` call in the
  credential-failure path. The reply doesn't depend on the request body;
  just write `Unauthorized` and close.

#### M-7 No crash recovery for in-progress uploads / mounts on daemon restart
- File: `crates/pcloud-daemon/src/bootstrap.rs`, `crates/pcloud-fs/src/platform/linux.rs:687`, `crates/pcloud-fs/src/fuser_shim.rs:222`
- The journal-replay at `fuser_shim.rs:222` recovers per-file write
  intent on mount, and `Request::MountForceUnmount` (methods.rs:720)
  exists for recovering orphan mounts, but the daemon does not
  proactively scan `$XDG_RUNTIME_DIR/pcloud/` or parse `/proc/mounts` on
  boot to re-adopt a mount left behind by a previous instance that was
  SIGKILL'd (e.g. OOM). In-progress uploads rely on the store to
  remember state, but there is no "resume on boot" loop that drains the
  upload journal; the current recovery is a separate
  `integrity_sweeper_service`. Sync state is persisted in SQLite, but
  sessions are ephemeral — the daemon's `session_refresh` ticks only
  after a client logs in.
- Remediation: add an explicit bootstrap step that (1) enumerates FUSE
  mounts owned by the daemon under its configured mount root and either
  re-adopts or forcibly unmounts; (2) replays the upload journal at
  boot instead of waiting for the first IPC; (3) re-hydrates the last
  session from `auth_vault` and kicks off a refresh tick before serving.

#### M-8 Response encoder does not enforce a server-side maximum response size
- File: `crates/pcloud-ipc/src/protocol.rs:220-233`, `crates/pcloud-daemon/src/runtime.rs` (multiple list handlers)
- `encode_response` checks `payload.len() > MAX_IPC_PAYLOAD_LEN` and
  returns `PayloadTooLarge`, but several list handlers
  (`ListPublicLinks`, `GetSyncRoots`, `ListConflicts`,
  `ListNotifications`) build the response body as a large JSON string
  first and then try to encode. An account with > 1 MiB of metadata
  (e.g. thousands of public links) will trigger `PayloadTooLarge` and
  the daemon returns `InternalError` with no pagination fallback.
- Remediation: list endpoints should paginate explicitly (cursor +
  limit), truncate with a continuation token, or stream responses. The
  `MAX_IPC_PAYLOAD_LEN = 1 MiB` cap is appropriate; the missing piece is
  proactive paging.

### LOW [7]

#### L-1 `RedactedString::Debug` uses `write!(f, "<redacted {} bytes>", self.0.len())` — discloses exact secret length
- File: `crates/pcloud-ipc/src/redacted.rs:75-79`
- An attacker observing `Debug` logs learns the byte length of every
  password / TFA code / passphrase. This is a known trade-off but worth
  noting; for very-low-entropy secrets (e.g. 6-digit TFA codes) knowing
  "the TFA field was 6 bytes" narrows brute-force space slightly.
- Remediation: bucket the length (`< 8`, `< 32`, `>= 32`) or omit it
  entirely. Keep the `is_empty` signal.

#### L-2 `ProtocolError::Codec` displays the underlying `serde_json::Error`, which may include column numbers and partial payload text
- File: `crates/pcloud-ipc/src/protocol.rs:144-146`, used at `transport.rs:346`
- `handle_client_error` writes `protocol_err.to_string()` into the
  response message. `serde_json` errors include line/column and
  sometimes a snippet of the offending JSON. For authenticated owner-UID
  peers this is fine, but it does leak the parser's diagnostic text
  into the IPC response.
- Remediation: map `Codec` errors to a fixed string like
  `"malformed IPC request body"` before embedding in the response
  message, and log the detailed version at `debug!` with redaction.

#### L-3 `encode_request_bare` is still exported as the default client helper — discourages envelope-aware (traceparent-propagating) callers
- File: `crates/pcloud-ipc/src/lib.rs:89-91`, `client.rs:73-75`
- The simpler API is still the bare one. A new caller looking at
  examples will reach for `encode_request_bare` and silently drop
  observability context. The envelope-aware path is documented but not
  the default.
- Remediation: flip the default — deprecate `encode_request_bare` with
  `#[deprecated]`, keep it for a release, and make envelope the obvious
  path.

#### L-4 Stress test is gated behind `#[ignore]` and is not exercised in CI by default
- File: `crates/pcloud-ipc/tests/stress_concurrent_clients.rs:44`
- `#[ignore = "stress: 50 clients x 500 reqs, run with --release --ignored"]`
  means the only concurrency regression test doesn't run in normal
  `cargo test`. Given H-4 (single-threaded serve), a small scale version
  (5 clients × 50 reqs, unignored) would catch serve-loop regressions.
- Remediation: add a non-ignored smaller scale variant; keep the big
  stress test opt-in.

#### L-5 `/health` returns plain `"ok"` (Content-Type default text/plain) — monitoring tools expecting JSON get opaque output
- File: `crates/pcloud-web/src/routes.rs:88-91`
- `"ok"` is not self-describing. Monitoring tools (Datadog, Blackbox
  exporter) typically prefer structured JSON (`{"status":"ok"}`) for
  health checks.
- Remediation: return a tiny JSON body once `/livez` vs `/readyz` split
  lands (M-6 / H-6 fix).

#### L-6 `pidfile` write is best-effort with only a `log::warn!` on failure — operator tooling may target a stale pid after a crash-recovery start
- File: `crates/pcloud-daemon/src/main.rs:102-105`
- If `write_pid_file` fails (e.g. state_dir read-only) the daemon still
  serves but `pcloudc drain` cannot find the pid. Removal on exit
  (`std::fs::remove_file`) is also best-effort and will not run on
  SIGKILL. A daemon-crash → daemon-restart sequence leaves a pidfile
  pointing to the DEAD pid; the new pid is written over it in the next
  startup, but between the two there is a window of confusion.
- Remediation: stat the existing pidfile at startup, check whether the
  pid is alive via `kill(pid, 0)`, and reject startup (or unlink and
  proceed) if stale. Use `flock(2)` on the pidfile to exclude concurrent
  daemons.

#### L-7 Doc drift — `serve.rs:181` documents "SIGHUP → hot-reload config from disk" but `signals.rs:275-277` says "SIGHUP is currently a no-op"
- File: `crates/pcloud-daemon/src/signals.rs:275-277`
- `handle_hup` sets `RELOAD_REQUESTED`. The serve loop at `serve.rs:181-204`
  actually DOES consume the flag and calls `try_reload`. But the module
  doc at `signals.rs:15-18` still says "the main loop treats it as a
  no-op today (documented; no config reload wired yet)." Inconsistent
  doc state invites future refactors to re-break what was fixed.
- Remediation: update the signals.rs module doc to reflect the live
  SIGHUP hot-reload path.

---

## Additional verifications (positive findings, no fix required)

- Peer-UID verification: Linux `SO_PEERCRED` and BSD/macOS `getpeereid(3)`
  are wired per-platform via `crates/pcloud-ipc/src/platform/{linux,unix}.rs`
  (confirmed by `transport.rs:386-399`). Windows remains scaffolded
  (documented).
- Socket file is created with mode `0600`; parent dir is `0700` when
  created (transport.rs:246-267).
- Framing has a hard 1 MiB cap enforced BEFORE any allocation
  proportional to the attacker-controlled length prefix
  (transport.rs:308-317). Verified by `tests/request_size_cap.rs` and
  `tests/security_invariants.rs::sec_12_*`.
- Version-mismatch frames are rejected cleanly
  (`ProtocolError::VersionMismatch`, protocol.rs:255-260). Test at
  `tests/peer_and_protocol.rs::decode_request_rejects_version_mismatch`.
- Oversized declared frame closes the connection WITHOUT replying, which
  is the correct DoS posture (transport.rs:337-340).
- `catch_unwind` boundary around dispatch is documented and tested
  (`ADR 0004`, `tests/security_invariants.rs::sec_50_*`).
- Web UI loopback-only bind is a hard panic on non-loopback address
  (`lib.rs:268-274`).
- CSRF double-submit + constant-time comparison on both CSRF tokens
  (routes.rs:611-637) and web-session tokens (routes.rs:659-687).
- In-flight counter via RAII guard decrements even on backend panics
  (signals.rs:182-206).
- Signal handler is async-signal-safe (only atomic stores;
  `SystemTime::now` explicitly deferred to the serve thread).
- `RequestEnvelope::try_from_wire` provides backward compatibility with
  pre-envelope bare-`Request` clients (methods.rs:1514-1522).
- Runtime directory uses `$XDG_RUNTIME_DIR` when available; socket path
  derives from `config.paths.ipc_socket_path()` which is under
  `runtime_dir` (pcloud-config/src/paths.rs:92-93).
- The serve loop honours an external `Arc<AtomicBool>` shutdown flag
  for Windows-Service-style shutdown (serve.rs:110-231, test at
  `serve_with_shutdown_exits_when_flag_set`).
- Rate-limiter reject produces `ResponseStatus::Conflict`, never
  `InternalError` — the right error class for actionable retry
  (rate_limit.rs:132-149).

---

## Summary table

| Severity | Count | Areas |
|----------|-------|-------|
| CRITICAL | 0     | —     |
| HIGH     | 6     | Proptest exhaustiveness, rate-limit auth category, sd_notify, single-threaded serve, Shutdown privilege, /health semantics |
| MEDIUM   | 8     | Accept-error handling, DrainStatus classification, drain-start race, EINTR in backends, web-token distribution, credential-fail read budget, crash-recovery, response size cap |
| LOW      | 7     | Debug length leak, serde_json leak in response, default envelope discouragement, ignored stress test, /health content-type, pidfile handling, doc drift |
