# `pcloud-fleet`

**Maturity:** Experimental / bounded

**Version:** `0.8.1-beta`

**Directory:** `crates/pcloud-fleet`

**Manifest:** [`crates/pcloud-fleet/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/Cargo.toml)

Fleet-management agent for enterprise pcloud-rs installations.

## Feature-family profile

**Why it exists.** Let centrally managed deployments report health and receive bounded commands without changing single-user defaults.

**What it is good for.** Experimental standalone enrollment, device identity, CA-authenticated HTTPS heartbeat, Ed25519 device/command signatures, SLO reporting, and fleet command envelopes.

**Why it is good at that job.** Null-by-default behavior, owner-only identity files, pinned controller CA trust, and constrained signed commands limit the management trust boundary; the crate is not wired into pcloudd.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_fleet` | lib | [`crates/pcloud-fleet/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs) |
| `coverage_surface` | test | [`crates/pcloud-fleet/tests/coverage_surface.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/coverage_surface.rs) |
| `live_mtls` | test | [`crates/pcloud-fleet/tests/live_mtls.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/live_mtls.rs) |
| `reference_server` | test | [`crates/pcloud-fleet/tests/reference_server.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/reference_server.rs) |

## Direct dependencies

`base64`, `bytes`, `ed25519-dalek`, `http-body-util`, `hyper`, `hyper-util`, `pcloud-observability`, `pcloud-secret`, `rand_core`, `reqwest`, `rustls`, `serde`, `serde_json`, `sha2`, `tempfile`, `thiserror`, `tokio`, `tokio-rustls`, `zeroize`

## Cargo features

| Feature | Enables |
|---|---|
| `default` | empty marker |
| `mtls` | empty marker |

## File inventory (8)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-fleet/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/Cargo.toml) | Cargo manifest | Historically named fleet transport: controller-authenticated HTTPS plus Ed25519 device/command signatures, not TLS client-certificate mTLS. |
| [`crates/pcloud-fleet/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs) | library root | Standalone fleet contract using controller-authenticated HTTPS plus Ed25519 device/command signatures; not wired into pcloudd. |
| [`crates/pcloud-fleet/tests/coverage_surface.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/coverage_surface.rs) | test | Public fleet-agent error and value-surface coverage. |
| [`crates/pcloud-fleet/tests/fixtures/fleet_test_ca.crt`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/fixtures/fleet_test_ca.crt) | test | Executable verification for the behavior named by this file. |
| [`crates/pcloud-fleet/tests/fixtures/fleet_test_server.crt`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/fixtures/fleet_test_server.crt) | test | Executable verification for the behavior named by this file. |
| [`crates/pcloud-fleet/tests/fixtures/fleet_test_server.key`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/fixtures/fleet_test_server.key) | test | Executable verification for the behavior named by this file. |
| [`crates/pcloud-fleet/tests/live_mtls.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/live_mtls.rs) | test | End-to-end integration test: fleet agent -&gt; in-process reference server. |
| [`crates/pcloud-fleet/tests/reference_server.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/reference_server.rs) | test | In-process reference fleet server used by `live_mtls.rs`. |

## Rust declaration index (76 total; 31 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `FleetError` | `pub` | enum | [`crates/pcloud-fleet/src/lib.rs:118`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L118) | Errors surfaced by the fleet agent. |
| `SyncState` | `pub` | enum | [`crates/pcloud-fleet/src/lib.rs:155`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L155) | Last observed sync-engine state, reported in each heartbeat. |
| `Slo` | `pub` | struct | [`crates/pcloud-fleet/src/lib.rs:170`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L170) | Service-level objective snapshot. All values are privacy-safe aggregates. |
| `Heartbeat` | `pub` | struct | [`crates/pcloud-fleet/src/lib.rs:186`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L186) | Heartbeat payload sent from agent to server. **Privacy invariant:** this structure must never contain file na… |
| `FleetCommand` | `pub` | enum | [`crates/pcloud-fleet/src/lib.rs:204`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L204) | Server-to-agent command. |
| `FleetResponse` | `pub` | enum | [`crates/pcloud-fleet/src/lib.rs:225`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L225) | Successful agent response to a \[`FleetCommand`\]. |
| `FleetAgent` | `pub` | trait | [`crates/pcloud-fleet/src/lib.rs:241`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L241) | The fleet agent trait. |
| `heartbeat` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:243`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L243) | Emit a heartbeat to the fleet server. |
| `handle_command` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:247`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L247) | Handle a server-issued command. Implementations MUST verify the signature before dispatching. |
| `NullFleetAgent` | `pub` | struct | [`crates/pcloud-fleet/src/lib.rs:253`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L253) | No-op agent. Used as the default agent when fleet is disabled and in tests that do not want to stand up a rea… |
| `new` | `pub` | fn | [`crates/pcloud-fleet/src/lib.rs:257`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L257) | Create a new `NullFleetAgent`. |
| `heartbeat` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:263`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L263) | Read the source/rustdoc for the exact contract. |
| `handle_command` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:267`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L267) | Read the source/rustdoc for the exact contract. |
| `FleetIdentityFile` | `private` | struct | [`crates/pcloud-fleet/src/lib.rs:286`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L286) | On-disk representation of a device identity. The private key is always base64-encoded and the file is written… |
| `FleetIdentity` | `pub` | struct | [`crates/pcloud-fleet/src/lib.rs:296`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L296) | Ed25519 device identity persisted to disk. The private key is held in memory as a \[`SecretBytes`\], zeroized o… |
| `fmt` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:303`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L303) | Read the source/rustdoc for the exact contract. |
| `new_or_load` | `pub` | fn | [`crates/pcloud-fleet/src/lib.rs:316`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L316) | Load an existing identity from `path`, or generate and persist a new one. The file is always rewritten with o… |
| `persist` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:358`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L358) | Read the source/rustdoc for the exact contract. |
| `device_id` | `pub` | fn | [`crates/pcloud-fleet/src/lib.rs:373`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L373) | Stable device identifier (hex-encoded SHA-256 of the public key). |
| `public_key_b64` | `pub` | fn | [`crates/pcloud-fleet/src/lib.rs:378`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L378) | Base64-encoded public key (as sent in the `X-PCloud-Device-SID` header). |
| `sign` | `pub` | fn | [`crates/pcloud-fleet/src/lib.rs:383`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L383) | Sign arbitrary body bytes with the device private key. |
| `write_owner_only` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:397`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L397) | Read the source/rustdoc for the exact contract. |
| `write_owner_only` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:412`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L412) | Read the source/rustdoc for the exact contract. |
| `hex` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:416`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L416) | Read the source/rustdoc for the exact contract. |
| `HEX` | `private` | const | [`crates/pcloud-fleet/src/lib.rs:417`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L417) | Read the source/rustdoc for the exact contract. |
| `MtlsFleetConfig` | `pub` | struct | [`crates/pcloud-fleet/src/lib.rs:432`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L432) | Configuration for \[`MtlsFleetAgent`\]. |
| `SignedCommand` | `pub` | struct | [`crates/pcloud-fleet/src/lib.rs:455`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L455) | Wire envelope returned by the fleet server: a command plus a detached signature over the canonical JSON-encod… |
| `RateLimiter` | `private` | struct | [`crates/pcloud-fleet/src/lib.rs:470`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L470) | Read the source/rustdoc for the exact contract. |
| `new` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:476`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L476) | Read the source/rustdoc for the exact contract. |
| `try_admit` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:483`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L483) | Read the source/rustdoc for the exact contract. |
| `MtlsFleetAgent` | `pub` | struct | [`crates/pcloud-fleet/src/lib.rs:503`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L503) | Fleet agent over HTTPS with ed25519 device identity and server-signed commands. Name retained for API stabili… |
| `new` | `pub` | fn | [`crates/pcloud-fleet/src/lib.rs:514`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L514) | Build a new agent. Loads or creates the device identity, reads the CA bundle, and constructs a rustls-backed… |
| `identity` | `pub` | fn | [`crates/pcloud-fleet/src/lib.rs:542`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L542) | The on-disk device identity. |
| `server_url` | `pub` | fn | [`crates/pcloud-fleet/src/lib.rs:547`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L547) | Configured server URL. |
| `device_group` | `pub` | fn | [`crates/pcloud-fleet/src/lib.rs:552`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L552) | Configured device group. |
| `default_heartbeat` | `pub` | fn | [`crates/pcloud-fleet/src/lib.rs:558`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L558) | Build a default heartbeat payload. Callers typically override \[`Heartbeat`\] fields from live metrics before s… |
| `send_heartbeat` | `pub` | fn | [`crates/pcloud-fleet/src/lib.rs:577`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L577) | Send a prepared heartbeat and return any signed command the server chose to issue. The returned command is va… |
| `verify_signed_command` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:615`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L615) | Read the source/rustdoc for the exact contract. |
| `fmt` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:646`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L646) | Read the source/rustdoc for the exact contract. |
| `heartbeat` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:656`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L656) | Read the source/rustdoc for the exact contract. |
| `handle_command` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:662`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L662) | Read the source/rustdoc for the exact contract. |
| `load_ca_bundle` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:684`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L684) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-fleet/src/lib.rs:712`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L712) | Read the source/rustdoc for the exact contract. |
| `mk_ca_bundle` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:718`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L718) | Read the source/rustdoc for the exact contract. |
| `mk_config` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:741`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L741) | Read the source/rustdoc for the exact contract. |
| `null_agent_heartbeat_is_ok` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:753`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L753) | Read the source/rustdoc for the exact contract. |
| `null_agent_applies_reconfigure` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:759`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L759) | Read the source/rustdoc for the exact contract. |
| `heartbeat_roundtrips_json` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:766`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L766) | Read the source/rustdoc for the exact contract. |
| `identity_roundtrip_persists_private_key_as_secret_bytes` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:786`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L786) | Read the source/rustdoc for the exact contract. |
| `heartbeat_payload_is_privacy_safe` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:816`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L816) | Read the source/rustdoc for the exact contract. |
| `unknown_server_signature_is_rejected` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:854`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L854) | Read the source/rustdoc for the exact contract. |
| `valid_server_signature_is_accepted` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:879`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L879) | Read the source/rustdoc for the exact contract. |
| `rate_limiter_rejects_second_command_within_one_second` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:899`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L899) | Read the source/rustdoc for the exact contract. |
| `ca_bundle_missing_is_load_error` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:910`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L910) | Read the source/rustdoc for the exact contract. |
| `ca_bundle_empty_pem_is_config_error` | `private` | fn | [`crates/pcloud-fleet/src/lib.rs:925`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/src/lib.rs#L925) | Read the source/rustdoc for the exact contract. |
| `config` | `private` | fn | [`crates/pcloud-fleet/tests/coverage_surface.rs:11`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/coverage_surface.rs#L11) | Read the source/rustdoc for the exact contract. |
| `malformed_identity_files_return_typed_errors_without_panicking` | `private` | fn | [`crates/pcloud-fleet/tests/coverage_surface.rs:23`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/coverage_surface.rs#L23) | Read the source/rustdoc for the exact contract. |
| `fleet_config_and_null_agent_cover_all_command_shapes` | `private` | fn | [`crates/pcloud-fleet/tests/coverage_surface.rs:77`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/coverage_surface.rs#L77) | Read the source/rustdoc for the exact contract. |
| `reference_server` | `private` | mod | [`crates/pcloud-fleet/tests/live_mtls.rs:39`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/live_mtls.rs#L39) | Read the source/rustdoc for the exact contract. |
| `mk_config` | `private` | fn | [`crates/pcloud-fleet/tests/live_mtls.rs:50`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/live_mtls.rs#L50) | Read the source/rustdoc for the exact contract. |
| `mint_identity` | `private` | fn | [`crates/pcloud-fleet/tests/live_mtls.rs:65`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/live_mtls.rs#L65) | Mint the agent once so the ed25519 identity file exists on disk, then return the base64-encoded device SID. T… |
| `heartbeat_is_accepted_end_to_end` | `private` | fn | [`crates/pcloud-fleet/tests/live_mtls.rs:84`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/live_mtls.rs#L84) | Read the source/rustdoc for the exact contract. |
| `tampered_body_signature_is_rejected` | `private` | fn | [`crates/pcloud-fleet/tests/live_mtls.rs:137`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/live_mtls.rs#L137) | Read the source/rustdoc for the exact contract. |
| `untrusted_device_sid_is_rejected` | `private` | fn | [`crates/pcloud-fleet/tests/live_mtls.rs:208`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/live_mtls.rs#L208) | Read the source/rustdoc for the exact contract. |
| `TEST_CA_PEM` | `pub` | const | [`crates/pcloud-fleet/tests/reference_server.rs:49`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/reference_server.rs#L49) | Trust-anchor CA certificate shipped under `tests/fixtures/`. This is the PEM the agent pins as its `ca_bundle… |
| `TEST_SERVER_CERT_PEM` | `pub` | const | [`crates/pcloud-fleet/tests/reference_server.rs:52`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/reference_server.rs#L52) | Server leaf certificate, signed by \[`TEST_CA_PEM`\]. Has SAN entries `DNS:localhost` and `IP:127.0.0.1`, so ru… |
| `TEST_SERVER_KEY_PEM` | `pub` | const | [`crates/pcloud-fleet/tests/reference_server.rs:54`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/reference_server.rs#L54) | PKCS#8 RSA private key matching \[`TEST_SERVER_CERT_PEM`\]. |
| `write_ca_bundle` | `pub` | fn | [`crates/pcloud-fleet/tests/reference_server.rs:60`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/reference_server.rs#L60) | Write the CA certificate into a directory and return the cert path, suitable for use as the fleet agent's `ca… |
| `ReferenceServer` | `pub` | struct | [`crates/pcloud-fleet/tests/reference_server.rs:68`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/reference_server.rs#L68) | Handle to a running reference server. Dropping the handle shuts the server down. |
| `shutdown` | `pub` | fn | [`crates/pcloud-fleet/tests/reference_server.rs:84`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/reference_server.rs#L84) | Graceful shutdown; blocks until the accept loop exits. |
| `base_url` | `pub` | fn | [`crates/pcloud-fleet/tests/reference_server.rs:94`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/reference_server.rs#L94) | HTTPS base URL, e.g. `https://127.0.0.1:49812`. |
| `spawn` | `pub` | fn | [`crates/pcloud-fleet/tests/reference_server.rs:102`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/reference_server.rs#L102) | Start a reference fleet server that trusts only the provided set of base64-encoded ed25519 device public keys… |
| `handle` | `private` | fn | [`crates/pcloud-fleet/tests/reference_server.rs:212`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/reference_server.rs#L212) | Read the source/rustdoc for the exact contract. |
| `error_response` | `private` | fn | [`crates/pcloud-fleet/tests/reference_server.rs:279`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/reference_server.rs#L279) | Read the source/rustdoc for the exact contract. |
| `load_certs` | `private` | fn | [`crates/pcloud-fleet/tests/reference_server.rs:287`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/reference_server.rs#L287) | Read the source/rustdoc for the exact contract. |
| `load_key` | `private` | fn | [`crates/pcloud-fleet/tests/reference_server.rs:297`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-fleet/tests/reference_server.rs#L297) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.
