# Fleet Management Agent — Landed

Status: **Landed (in-process reference server, end-to-end tested over
real TLS).** Implementation ships in `crates/pcloud-fleet/`. The
`FleetAgent` trait is object-safe and held behind `Arc<dyn FleetAgent>`
in the daemon runtime. An in-process reference fleet server now lives
under `crates/pcloud-fleet/tests/reference_server.rs`; it serves HTTPS
on `127.0.0.1:<auto_port>`, presents a CA-signed leaf cert from
`tests/fixtures/`, and validates the agent's `X-PCloud-Body-Signature`
header against a configured device-SID trust set. The
`tests/live_mtls.rs` integration test drives a real `MtlsFleetAgent`
through that server (happy path, tampered-body rejection, untrusted-SID
rejection). Live-in-prod interop against a third-party fleet controller
is **still not tested** and is **still not claimed** — the reference
server is a protocol spec in code form, not a substitute for a real
controller behind a production load balancer.

## 0. What actually landed

- `MtlsFleetAgent` in `crates/pcloud-fleet/src/agent.rs` is the
  concrete enterprise implementor. It drives heartbeat, command intake,
  and response emission over mTLS WebSocket using the protocol
  specified below.
- **Device identity is an ed25519 keypair** generated on first run.
  The private key is held in `pcloud_secret::SecretBytes` in memory
  (zeroized on `Drop`) and persisted to a dedicated identity file with
  owner-only permissions (`0600`, parent directory `0700`) — the same
  posture as the auth vault. The on-disk format is validated for
  ownership and mode on every load; a tampered file is refused with an
  audit record.
- **rustls uses an explicit fleet root store.** The TLS client is
  constructed with a `RootCertStore` built **only** from the
  operator-supplied `ca_bundle`. System trust roots are not consulted.
  A certificate chain that would validate against system CAs but not
  against the operator bundle is rejected — this matches §4 of this
  document and is enforced in code, not by policy.
- **Body signatures use an `X-PCloud-Body-Signature` header** over a
  deterministic canonical JSON serialisation of the frame payload
  (sorted keys, no insignificant whitespace, UTF-8). Every
  `FleetCommand` is verified against the server's
  **trusted command-signing key** loaded from operator config — the
  verification runs **before** the frame is matched on variant, and a
  verification failure never reaches command dispatch.
- **Rate limit: 1 command per second** (token bucket, burst 5).
  Excess commands are rejected with `FleetError::RateLimited` and
  audited.
- **Offline + in-process end-to-end test coverage.** The unit test
  suite exercises: heartbeat frame construction, canonical-JSON
  stability, signature verification pass/fail, identity file permission
  guard, rustls trust-root restriction (system-CA chain is rejected),
  rate-limit token-bucket behaviour, and each `FleetCommand` handler's
  happy and error paths. In addition, `tests/live_mtls.rs` spins up the
  `reference_server.rs` helper (real TLS listener, fixture CA chain)
  and drives a real `MtlsFleetAgent` through it:
  - `heartbeat_is_accepted_end_to_end` — pinned-CA TLS handshake +
    valid body signature -> 200 OK, zero rejections on the server side.
  - `tampered_body_signature_is_rejected` — handcrafted request whose
    signature header covers a different payload than is posted -> 401
    at the server, zero verified requests.
  - `untrusted_device_sid_is_rejected` — agent whose SID is not in the
    server's allow-list gets 401 even with a mathematically valid
    signature, surfaced to the caller as `FleetError::Transport`.
  No external network is touched, and no live production fleet
  controller is contacted. The reference server is an in-repo spec, not
  a real controller.

## 1. Problem Statement

In the legacy C pcloud-rs, each installation is an island. A corporate IT
team running pcloud-rs across hundreds of laptops has **no supported way** to:

- see which devices are online, at what version, and in what sync state,
- push a config change (e.g. new API server, new sync roots, new policy)
  without touching every endpoint by hand,
- roll out an upgrade on a schedule and verify it landed,
- collect a diagnostic (`doctor`) bundle from a misbehaving device,
- quarantine a compromised device (stop syncing, force re-auth) without
  waiting for the user to click something,
- detect **configuration drift** — a device whose effective config no longer
  matches the policy baseline.

Every one of these tasks is a real enterprise requirement that competitors
(Dropbox Business, Box Shield, OneDrive for Business) already ship. Shipping
pcloud-rs into a regulated enterprise without a fleet surface means shipping
it into an environment where it cannot be audited, cannot be steered, and
cannot be safely retired. The Rust rewrite is the right place to fix that
because adding it to the C path would mean duplicating auth, IPC, and
transport hardening that only exist on the Rust side.

This design specifies the **client-side agent** and the **wire protocol**.
The fleet server itself is out of scope for this repository; any server that
speaks this protocol correctly is a valid peer.

## 2. Architecture

```
┌──────────────────────────┐        mTLS + WebSocket         ┌──────────────────┐
│  pcloud-rs daemon         │ ◀──────────────────────────────▶ │  Fleet server    │
│  ┌────────────────────┐  │     JSON frames, ed25519-signed  │  (out of repo)   │
│  │ FleetAgent trait   │  │                                  │  - CA            │
│  │  - NullFleetAgent  │  │                                  │  - policy store  │
│  │  - MtlsFleetAgent  │  │                                  │  - audit log     │
│  └────────────────────┘  │                                  │                  │
│  ↑ heartbeat tick        │                                  │                  │
│  ↓ command dispatch      │                                  │                  │
└──────────────────────────┘                                  └──────────────────┘
```

The agent lives inside the daemon process (not a sidecar) so that it shares
the daemon's supervision, logging, and crash semantics. It exposes the
[`FleetAgent`] trait, which the daemon runtime drives from two places:

- A periodic **heartbeat tick** (interval is operator-configurable).
- An **inbound command channel** whose only producer is the verified WSS
  connection; every other source is rejected at the type level.

The default build ships `NullFleetAgent` so that a daemon started without a
fleet configuration is a no-op. Enterprise builds enable the `mtls` feature
and wire in `MtlsFleetAgent`, which speaks the protocol below over `rustls`
+ `tokio-tungstenite`.

## 3. Wire Protocol

All frames are JSON, UTF-8. One frame per WebSocket message. Unknown fields
are ignored on receive; unknown variants cause a hard reject.

### 3.1 Heartbeat (agent → server)

```json
{
  "device_id": "<hex-sha256-of-pubkey>",
  "version": "0.1.0+sha-abc123",
  "os": "linux",
  "last_sync_state": "active",
  "slo": {
    "ip95_ms": 12,
    "upload_retry_ratio": 0.01,
    "crash_free_fraction": 0.999
  },
  "config_hash": "<hex-sha256-of-config.toml>"
}
```

`last_sync_state` is one of `idle | active | stalled | paused | quarantined`.

### 3.2 FleetCommand (server → agent)

```json
{ "kind": "reconfigure", "0": { "heartbeat_interval": 60 } }
{ "kind": "upgrade",     "target_version": "0.2.0", "signature": [/* bytes */] }
{ "kind": "run_doctor" }
{ "kind": "quarantine" }
{ "kind": "unregister" }
```

### 3.3 FleetResponse (agent → server)

```json
{ "kind": "applied" }
{ "kind": "scheduled", "token": "sch_abc" }
{ "kind": "doctor_report", "report_hash": "<hex-sha256>" }
```

## 4. Device Identity and mTLS

On first contact the agent:

1. Generates an ed25519 keypair locally. The private key is stored in the
   platform keystore (Linux: secret-service; macOS: Keychain; Windows: DPAPI)
   — **never** on disk in the clear.
2. Sends an enrollment request with the public key and a one-time
   enrollment token supplied by IT out-of-band.
3. Receives an mTLS client certificate signed by the fleet CA, valid for
   90 days and auto-renewed at 60-day age.
4. Presents that certificate on every subsequent WSS handshake.

The daemon treats the fleet CA bundle as operator-supplied config; it is
**not** derived from system trust roots.

## 5. Config Drift Detection

On each heartbeat the agent computes `sha256(config.toml)` over the
effective, post-expansion config and emits the hex digest as `config_hash`.

The server maintains an expected hash per device group. A mismatch can
trigger any server-side policy — most commonly a `Reconfigure` command that
pushes the canonical blob back, or an operator alert.

No config **content** is transmitted with the heartbeat. Only the hash.

## 6. Command Execution

- `Reconfigure(blob)`: the agent serializes the JSON blob into TOML, writes
  it to a tempfile in the same directory as `config.toml`, `fsync`s, and
  `rename(2)`s it into place (atomic replace). Then it signals the daemon
  runtime to reload. If validation fails, the agent reverts and reports
  the failure on the next heartbeat.
- `Upgrade { target_version, signature }`: the agent downloads the signed
  installer (MSI on Windows, pkg on macOS, tarball on Linux), verifies the
  detached `signature` against the fleet CA's distribution key, and
  schedules the self-replace for the next idle window.
- `RunDoctor`: the agent collects a standard doctor bundle (see
  `pcloud-observability`), uploads it, and returns `DoctorReport`.
- `Quarantine`: the agent locks every sync root, clears the auth vault's
  live-session cache, and requires interactive re-auth before resuming.
- `Unregister`: the agent revokes its own certificate locally, wipes the
  enrollment record, and shuts down the fleet subsystem. The daemon
  continues to run as a standalone client.

## 7. Security

- Every `FleetCommand` is signed by the server's ed25519 command-signing
  key (separate from the CA). The agent verifies the signature **before**
  matching on the variant.
- A token-bucket rate limiter caps command execution at **1/s**, with a
  burst of 5. Excess commands are rejected with `FleetError::RateLimited`.
- Signature verification failures are logged with the frame's SHA-256 but
  never with the raw bytes.
- The heartbeat carries only opaque hashes and numeric SLOs. File names,
  paths, account ids, and user-controlled strings are **forbidden** in the
  protocol; see the doc comment on `Heartbeat` for the invariant.

## 8. Operator Configuration

```toml
[fleet]
server_url        = "wss://fleet.corp.example/v1"
ca_bundle         = "/etc/pcloud-rs/fleet-ca.pem"
device_group      = "finance-laptops"
heartbeat_interval = "60s"
```

Absent or empty `[fleet]` keeps the daemon on `NullFleetAgent`. This is
the correct default for individual users.

## 9. CLI Surface

- `pcloudc fleet status`: prints enrollment state, server URL, last
  heartbeat, last command, drift state.
- `pcloudc fleet disenroll`: runs the `Unregister` path locally (requires
  operator confirmation).
- `pcloudc fleet force-checkin`: skips the timer and emits a heartbeat
  immediately; useful for IT validation.

## 10. Privacy

The heartbeat schema is the privacy contract. Any change that would add a
path, a filename, an account identifier, or a user-controlled string to the
frame MUST be reviewed as a privacy-impacting change and documented in the
release notes. The rustdoc on `Heartbeat` enforces this as a written
invariant; reviewers are expected to hold new fields to it.

## 11. Interface / trait shape

Authoritative declarations:

- `FleetAgent` trait — `crates/pcloud-fleet/src/lib.rs:238`
- `FleetError` — `crates/pcloud-fleet/src/lib.rs:115`
- `Heartbeat` (privacy contract) — `crates/pcloud-fleet/src/lib.rs:183`
- `FleetCommand` — `crates/pcloud-fleet/src/lib.rs:201`
- `FleetResponse` — `crates/pcloud-fleet/src/lib.rs:222`
- `FleetIdentity` (ed25519, `SecretBytes`, `0600` file) —
  `crates/pcloud-fleet/src/lib.rs:293`
  - `::new_or_load(path)` at `:313`
  - `::sign(body)` at `:380`
- `MtlsFleetAgent` — `crates/pcloud-fleet/src/lib.rs:500`
  - `::new(config)` at `:511`
  - Explicit root store (`tls_built_in_root_certs(false)`) at `:522`
  - `::send_heartbeat(&hb)` at `:574` (emits
    `X-PCloud-Body-Signature` header at `:586`)
- `RateLimiter` (1/s floor, mutex-poisoned-panic on contention bug) —
  `crates/pcloud-fleet/src/lib.rs:467`
- `NullFleetAgent` (default when `[fleet]` is absent) —
  `crates/pcloud-fleet/src/lib.rs:250`

```rust
// Simplified; see crates/pcloud-fleet/src/lib.rs:238.
pub trait FleetAgent: Send + Sync {
    fn send_heartbeat(&self, hb: &Heartbeat) -> Result<Option<FleetCommand>, FleetError>;
    fn execute(&self, cmd: FleetCommand) -> Result<FleetResponse, FleetError>;
    fn identity(&self) -> &FleetIdentity;
}
```

## 12. Configuration reference — every key

| Key                   | Type      | Default | Purpose                                                                                | Example |
|-----------------------|-----------|---------|----------------------------------------------------------------------------------------|---------|
| `server_url`          | string    | —       | HTTPS URL of the fleet server. Scheme must be `https`.                                  | `"https://fleet.corp.example/v1"` |
| `ca_bundle`           | string    | —       | PEM CA bundle. Replaces the system trust store — we call `tls_built_in_root_certs(false)`. | `"/etc/pcloud-rs/fleet-ca.pem"` |
| `identity_path`       | string    | `$XDG_STATE_HOME/pcloud-rs/fleet.key` | ed25519 private key file, `0600`, created on first call. | `"/var/lib/pcloud-rs/fleet.key"` |
| `device_group`        | string    | `"default"` | Logical group label emitted on heartbeat. No PII allowed.                              | `"finance-laptops"` |
| `heartbeat_interval`  | duration  | `60s`   | Interval between heartbeats. Server may overrule via `SetInterval`.                    | `"60s"` |
| `trusted_server_keys` | `[string]`| —       | Base64 ed25519 public keys authorised to sign commands.                                | `["MCowBQYDK2VwAyEA..."]` |

Absent or empty `[fleet]` keeps the daemon on `NullFleetAgent`.

## 13. Onboarding recipe

### Beginner — deploy in 5 steps

1. On the fleet server: generate an ed25519 command-signing key. Keep
   the private key in your fleet server's secrets store; publish the
   public key base64 to operators.
2. Distribute the `ca_bundle` PEM to every client (MDM profile,
   Ansible, Jamf, Intune — anything that writes to
   `/etc/pcloud-rs/fleet-ca.pem`).
3. Append `[fleet]` to `pcloud-rs.toml` with `server_url`, `ca_bundle`,
   `trusted_server_keys`, and `device_group`.
4. `sudo systemctl restart pcloudcd`. The agent autogenerates its
   ed25519 device identity at `identity_path` on first boot (`0600`,
   owner-only, see `crates/pcloud-fleet/src/lib.rs:313`).
5. `pcloudc fleet status` — verify `enrolled=true`, non-zero
   `last_heartbeat_age`.

### Expert — Ansible with MDM profile

```yaml
- name: configure pcloud-rs fleet enrolment
  hosts: corp_laptops
  vars:
    fleet_server: "https://fleet.corp.example/v1"
    trusted_keys: "{{ lookup('hashi_vault', 'secret/pcloud-rs:trusted_keys') }}"
  tasks:
    - copy:
        dest: /etc/pcloud-rs/fleet-ca.pem
        content: "{{ fleet_ca_pem }}"
        mode: '0644'
    - template:
        src: pcloud-rs.toml.j2
        dest: /etc/pcloud-rs/pcloud-rs.toml
        mode: '0644'
    - service: { name: pcloudcd, state: restarted }
```

## 14. Verification

1. **Signed heartbeat on the wire** — tcpdump/pcap on the fleet server
   ingress. Every request must carry `X-PCloud-Body-Signature` (see
   `crates/pcloud-fleet/src/lib.rs:586`) and verify under the
   advertised public key.
2. **Rate limit** — script 10 commands/second at the server; 9 must
   return `RateLimited` (`crates/pcloud-fleet/src/lib.rs:126`,
   limiter at `:467`).
3. **Identity file perms** — `stat -c '%a %U' $identity_path` → `600 root`.
4. **Forged command rejected** — `cargo test -p pcloud-fleet
   rejects_bad_signature`.
5. **In-process end-to-end** — `cargo test -p pcloud-fleet --test
   live_mtls` stands up the in-repo reference server
   (`tests/reference_server.rs`, real TLS, real ed25519 verification)
   and drives the agent through happy-path + tamper + untrusted-SID
   flows. There is still no external reference-server **binary** and
   no live-prod interop claim; see §10.

## 15. Failure modes + remediation

| Symptom / `FleetError`            | Root cause                                                          | Remediation |
|-----------------------------------|---------------------------------------------------------------------|-------------|
| `InvalidSignature`                | Command signed by a key not in `trusted_server_keys`                | Confirm key rotation was published; re-roll config. Never loosen the allow-list. |
| `RateLimited`                     | Server is spamming commands, or a retry storm                        | Server-side fix; client limiter is intentional. |
| `Transport`                       | TLS handshake failure — wrong CA bundle, expired cert, proxy MITM    | Verify `openssl s_client`; remember the client disables system roots. |
| Identity file wrong mode          | Someone ran `chmod 644` on `fleet.key`                              | Restore `chmod 600`, `chown root:root`. Rotate the key if exposure is possible. |
| Heartbeat carries PII             | A contributor added a free-form string field                        | Revert the field; the `Heartbeat` rustdoc invariant is blocking. |

## 16. Extension points

- **Alternate transports.** Implement `FleetAgent` for a WebSocket or
  gRPC transport; keep the ed25519 signature envelope identical so
  replay defences transfer. See `crates/pcloud-fleet/src/lib.rs:238`.
- **New command variants.** Extend `FleetCommand`
  (`crates/pcloud-fleet/src/lib.rs:201`). Every new variant must:
  (a) carry an explicit signature field,
  (b) be verified *before* the `match` dispatch,
  (c) pass the "no PII" review bar.
- **Alternate identity backing** (TPM / Secure Enclave). Replace
  `FleetIdentity::new_or_load` with a hardware-backed signer but keep
  the trait contract; `sign(body)` must still return a detached ed25519
  `[u8; 64]` (`crates/pcloud-fleet/src/lib.rs:380`).

## 17. Cross-refs

- CLI: `docs/book/src/cli/fleet.md`
- Runbook — fleet server outage: `docs/runbooks/fleet-outage.md`
- Secret wrapper: `crates/pcloud-secret/src/secret_bytes.rs`
- Observability: `docs/enterprise/tracing.md` (companion; owned by
  another agent)
- Parity row: `C_FEATURE_PARITY_MATRIX.csv`
  (`fleet.*` rows — `Rejected` on legacy C, net-new in Rust)
