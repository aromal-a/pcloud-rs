# `pcloud-p2p`

**Maturity:** Experimental / bounded

**Version:** `0.1.0`

**Directory:** `crates/pcloud-p2p`

**Manifest:** [`crates/pcloud-p2p/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/Cargo.toml)

Peer-to-peer LAN sync scaffolding for pcloud-rs (experimental).

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_p2p` | lib | [`crates/pcloud-p2p/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs) |

## Direct dependencies

`mdns-sd`, `serde`, `serde_json`, `sha2`, `thiserror`

## Cargo features

No declared package features.

## File inventory (6)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-p2p/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-p2p/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/README.md) | documentation | pcloud-p2p |
| [`crates/pcloud-p2p/src/discovery.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/discovery.rs) | Rust module | Peer discovery configuration (maximum peers to track at once). |
| [`crates/pcloud-p2p/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs) | library root | pcloud-p2p |
| [`crates/pcloud-p2p/src/policy.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/policy.rs) | Rust module | Global on/off policy for the P2P subsystem. |
| [`crates/pcloud-p2p/src/transfer.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/transfer.rs) | Rust module | Peer-to-peer transfer tuning knobs. |

## Rust declaration index (44 total; 24 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `PeerDiscovery` | `pub` | struct | [`crates/pcloud-p2p/src/discovery.rs:20`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/discovery.rs#L20) | Peer discovery configuration (maximum peers to track at once). The planned discovery transport is mDNS / DNS-… |
| `default` | `private` | fn | [`crates/pcloud-p2p/src/discovery.rs:31`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/discovery.rs#L31) | Read the source/rustdoc for the exact contract. |
| `InstanceId` | `pub` | struct | [`crates/pcloud-p2p/src/discovery.rs:44`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/discovery.rs#L44) | Opaque instance identifier advertised on the LAN. |
| `PeerInfo` | `pub` | struct | [`crates/pcloud-p2p/src/discovery.rs:48`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/discovery.rs#L48) | Information about a discovered peer. Scaffolding shape only. |
| `P2pError` | `pub` | enum | [`crates/pcloud-p2p/src/discovery.rs:64`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/discovery.rs#L64) | Error surface for the (future) discovery runtime. Empty today. |
| `DiscoveryRuntime` | `pub` | struct | [`crates/pcloud-p2p/src/discovery.rs:73`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/discovery.rs#L73) | Inert handle that pretends to own an mDNS responder. Never advertises or browses — see the crate-level docs. |
| `start` | `pub` | fn | [`crates/pcloud-p2p/src/discovery.rs:85`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/discovery.rs#L85) | Start a no-op runtime. Kept so `P2pShell::start` compiles; real discovery will land under `bd-1du.10` / R9 #4… |
| `shutdown` | `pub` | fn | [`crates/pcloud-p2p/src/discovery.rs:93`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/discovery.rs#L93) | Shutdown hook. No-op today. |
| `peers` | `pub` | fn | [`crates/pcloud-p2p/src/discovery.rs:97`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/discovery.rs#L97) | Snapshot of known peers. Always empty on the scaffold. |
| `instance_id` | `pub` | fn | [`crates/pcloud-p2p/src/discovery.rs:103`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/discovery.rs#L103) | Advertised instance id for this runtime. |
| `discovery` | `pub` | mod | [`crates/pcloud-p2p/src/lib.rs:76`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L76) | Peer discovery primitives (LAN scan, peer inventory). |
| `policy` | `pub` | mod | [`crates/pcloud-p2p/src/lib.rs:78`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L78) | P2P on/off policy and gating rules. |
| `transfer` | `pub` | mod | [`crates/pcloud-p2p/src/lib.rs:80`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L80) | Peer-to-peer transfer tuning surface. |
| `CRATE_NAME` | `pub` | const | [`crates/pcloud-p2p/src/lib.rs:85`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L85) | Crate identifier used in logs and telemetry. |
| `SERVICE_TYPE` | `pub` | const | [`crates/pcloud-p2p/src/lib.rs:88`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L88) | mDNS service type advertised and browsed by the discovery runtime. |
| `P2pShell` | `pub` | struct | [`crates/pcloud-p2p/src/lib.rs:102`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L102) | Composition of the three P2P sub-shells (policy / discovery / transfer). # Honest scope (2026-04-15) This she… |
| `new` | `pub` | fn | [`crates/pcloud-p2p/src/lib.rs:116`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L116) | Construct a disabled-by-default shell with no active runtime. |
| `summary` | `pub` | fn | [`crates/pcloud-p2p/src/lib.rs:122`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L122) | Render a single-line human-readable summary of the shell state. |
| `start` | `pub` | fn | [`crates/pcloud-p2p/src/lib.rs:147`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L147) | Start the mDNS discovery runtime: spawn a responder advertising \[`SERVICE_TYPE`\] with TXT keys `instance=&lt;uui… |
| `stop` | `pub` | fn | [`crates/pcloud-p2p/src/lib.rs:157`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L157) | Stop the active mDNS runtime, if any. Idempotent. |
| `is_running` | `pub` | fn | [`crates/pcloud-p2p/src/lib.rs:165`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L165) | Whether an mDNS runtime is currently active. |
| `peers` | `pub` | fn | [`crates/pcloud-p2p/src/lib.rs:172`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L172) | Return a snapshot of currently-known peers. Empty when the runtime is not active. |
| `instance_id` | `pub` | fn | [`crates/pcloud-p2p/src/lib.rs:182`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L182) | Instance id advertised by the active runtime, or `None` when discovery is not running. |
| `drop` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:188`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L188) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-p2p/src/lib.rs:194`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L194) | Read the source/rustdoc for the exact contract. |
| `crate_name_is_stable` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:199`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L199) | Read the source/rustdoc for the exact contract. |
| `default_shell_is_disabled` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:204`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L204) | Read the source/rustdoc for the exact contract. |
| `summary_reflects_state_happy_path` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:212`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L212) | Read the source/rustdoc for the exact contract. |
| `summary_reflects_custom_enabled_state` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:221`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L221) | Read the source/rustdoc for the exact contract. |
| `summary_boundary_zero_values` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:237`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L237) | Read the source/rustdoc for the exact contract. |
| `summary_boundary_usize_max` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:253`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L253) | Read the source/rustdoc for the exact contract. |
| `peers_endpoint_returns_empty_when_no_peers` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:269`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L269) | Read the source/rustdoc for the exact contract. |
| `peer_list_serde_roundtrip` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:280`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L280) | Read the source/rustdoc for the exact contract. |
| `policy_serde_roundtrip` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:305`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L305) | Read the source/rustdoc for the exact contract. |
| `discovery_serde_roundtrip` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:313`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L313) | Read the source/rustdoc for the exact contract. |
| `transfer_serde_roundtrip` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:321`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L321) | Read the source/rustdoc for the exact contract. |
| `discovery_rejects_invalid_json` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:331`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L331) | Read the source/rustdoc for the exact contract. |
| `policy_rejects_invalid_json` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:338`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L338) | Read the source/rustdoc for the exact contract. |
| `policy_default_is_sane` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:344`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L344) | Read the source/rustdoc for the exact contract. |
| `discovery_runtime_constructs` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:351`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L351) | Read the source/rustdoc for the exact contract. |
| `peer_info_serde_roundtrip` | `private` | fn | [`crates/pcloud-p2p/src/lib.rs:363`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/lib.rs#L363) | Read the source/rustdoc for the exact contract. |
| `P2pPolicy` | `pub` | struct | [`crates/pcloud-p2p/src/policy.rs:20`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/policy.rs#L20) | Global on/off policy for the P2P subsystem. This is the single kill-switch for every planned LAN-acceleration… |
| `PeerTransfer` | `pub` | struct | [`crates/pcloud-p2p/src/transfer.rs:22`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/transfer.rs#L22) | Peer-to-peer transfer tuning knobs. The planned transport is UDP with hole-punching, keyed per session by a s… |
| `default` | `private` | fn | [`crates/pcloud-p2p/src/transfer.rs:33`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-p2p/src/transfer.rs#L33) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.
