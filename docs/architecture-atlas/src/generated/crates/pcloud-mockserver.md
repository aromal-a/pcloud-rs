# `pcloud-mockserver`

**Maturity:** Verification support

**Version:** `0.1.0`

**Directory:** `crates/pcloud-mockserver`

**Manifest:** [`crates/pcloud-mockserver/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/Cargo.toml)

Local in-process HTTP mock of the pCloud REST API for integration tests. No network access required; no production secrets involved.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_mockserver` | lib | [`crates/pcloud-mockserver/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs) |
| `mock_flows` | test | [`crates/pcloud-mockserver/tests/mock_flows.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/tests/mock_flows.rs) |

## Direct dependencies

`serde`, `serde_json`

## Cargo features

No declared package features.

## File inventory (4)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-mockserver/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-mockserver/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/README.md) | documentation | pcloud-mockserver |
| [`crates/pcloud-mockserver/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs) | library root | `pcloud-mockserver` — a tiny, in-process HTTP mock of the pCloud REST API. |
| [`crates/pcloud-mockserver/tests/mock_flows.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/tests/mock_flows.rs) | test | Mock-backed integration flows. |

## Rust declaration index (63 total; 15 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `TEST_TOKEN` | `pub` | const | [`crates/pcloud-mockserver/src/lib.rs:88`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L88) | The single well-known auth token the mock accepts. Any request that presents a different token (via `?auth=…`… |
| `ERR_INVALID_TOKEN` | `pub` | const | [`crates/pcloud-mockserver/src/lib.rs:97`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L97) | pCloud-style error code for an invalid or expired login token. Returned by the mock when a request presents a… |
| `ERR_LOGIN_REQUIRED` | `pub` | const | [`crates/pcloud-mockserver/src/lib.rs:106`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L106) | pCloud-style error code for "Log in required." Returned by the mock when a request omits an auth token entire… |
| `ERR_GENERIC_INJECTED` | `pub` | const | [`crates/pcloud-mockserver/src/lib.rs:117`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L117) | Generic "injected" failure code used by the `?inject_error=N` hook. Callers pick any `N` they want — the mock… |
| `MockHandle` | `pub` | struct | [`crates/pcloud-mockserver/src/lib.rs:129`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L129) | Handle returned by \[`MockServer::start`\]. Owns the accept thread and the shared \[`MockState`\]. Dropping the h… |
| `base_url` | `pub` | fn | [`crates/pcloud-mockserver/src/lib.rs:142`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L142) | Returns the base URL of the form `http://127.0.0.1:&lt;port&gt;` (no trailing slash). Tests build endpoint URLs by… |
| `addr` | `pub` | fn | [`crates/pcloud-mockserver/src/lib.rs:151`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L151) | Returns the bound local socket address. Useful for tests that want the chosen port explicitly (for example to… |
| `state` | `pub` | fn | [`crates/pcloud-mockserver/src/lib.rs:162`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L162) | Returns an `Arc` clone of the shared server state. Tests use this to seed fixtures (e.g. inserting a \[`MockFi… |
| `shutdown` | `pub` | fn | [`crates/pcloud-mockserver/src/lib.rs:171`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L171) | Explicitly shut the server down and join the accept thread. Equivalent to letting the handle drop, except tha… |
| `stop` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:175`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L175) | Read the source/rustdoc for the exact contract. |
| `drop` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:186`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L186) | Read the source/rustdoc for the exact contract. |
| `MockState` | `pub` | struct | [`crates/pcloud-mockserver/src/lib.rs:202`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L202) | In-memory state shared by all requests. Tests reach into this via \[`MockHandle::state`\] to assert what the se… |
| `MockFile` | `pub` | struct | [`crates/pcloud-mockserver/src/lib.rs:237`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L237) | Seeded or synthetic file entry stored inside the mock server's state. Scenario: download, `listfolder`, and p… |
| `ShareEntry` | `pub` | struct | [`crates/pcloud-mockserver/src/lib.rs:253`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L253) | Synthetic share entry exposed by the mock's `/listshares` response. Scenario: outgoing-share tests assert tha… |
| `new` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:263`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L263) | Read the source/rustdoc for the exact contract. |
| `alloc_id` | `pub` | fn | [`crates/pcloud-mockserver/src/lib.rs:279`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L279) | Allocate a fresh monotonic id. Exposed for tests that seed fixtures directly (files, folders, shares, public… |
| `MockServer` | `pub` | struct | [`crates/pcloud-mockserver/src/lib.rs:292`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L292) | Entry point for starting the mock server. This is a zero-sized type used purely as a namespace for \[`MockServ… |
| `start` | `pub` | fn | [`crates/pcloud-mockserver/src/lib.rs:308`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L308) | Bind to `127.0.0.1:0` (OS-assigned random port) and spawn the accept loop. Returns a \[`MockHandle`\] that expo… |
| `accept_loop` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:337`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L337) | Read the source/rustdoc for the exact contract. |
| `ParsedRequest` | `private` | struct | [`crates/pcloud-mockserver/src/lib.rs:360`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L360) | Read the source/rustdoc for the exact contract. |
| `handle_connection` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:372`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L372) | Read the source/rustdoc for the exact contract. |
| `read_request` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:382`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L382) | Read the source/rustdoc for the exact contract. |
| `parse_query` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:433`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L433) | Read the source/rustdoc for the exact contract. |
| `url_decode` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:442`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L442) | Read the source/rustdoc for the exact contract. |
| `auth_token_from` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:471`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L471) | Read the source/rustdoc for the exact contract. |
| `is_public_endpoint` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:485`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L485) | Routes that do not require a valid token. |
| `dispatch` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:489`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L489) | Read the source/rustdoc for the exact contract. |
| `handle_userinfo` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:540`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L540) | Read the source/rustdoc for the exact contract. |
| `handle_listfolder` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:557`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L557) | Read the source/rustdoc for the exact contract. |
| `handle_getfilepublink` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:590`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L590) | Read the source/rustdoc for the exact contract. |
| `handle_listpubs` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:613`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L613) | Read the source/rustdoc for the exact contract. |
| `handle_upload_create` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:622`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L622) | Read the source/rustdoc for the exact contract. |
| `handle_upload_write` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:628`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L628) | Read the source/rustdoc for the exact contract. |
| `handle_upload_save` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:648`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L648) | Read the source/rustdoc for the exact contract. |
| `handle_listnotifications` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:691`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L691) | Read the source/rustdoc for the exact contract. |
| `handle_readnotifications` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:704`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L704) | Read the source/rustdoc for the exact contract. |
| `handle_listshares` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:709`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L709) | Read the source/rustdoc for the exact contract. |
| `handle_sharefolder` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:729`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L729) | Read the source/rustdoc for the exact contract. |
| `handle_createbackup` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:756`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L756) | Read the source/rustdoc for the exact contract. |
| `handle_stopdevice` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:770`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L770) | Read the source/rustdoc for the exact contract. |
| `error_body` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:782`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L782) | Read the source/rustdoc for the exact contract. |
| `json_response` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:786`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L786) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-mockserver/src/lib.rs:807`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L807) | Read the source/rustdoc for the exact contract. |
| `http_get` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:810`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L810) | Read the source/rustdoc for the exact contract. |
| `http_post` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:835`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L835) | Read the source/rustdoc for the exact contract. |
| `starts_and_serves_userinfo_with_valid_token` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:862`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L862) | Read the source/rustdoc for the exact contract. |
| `rejects_unknown_token_with_2094` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:872`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L872) | Read the source/rustdoc for the exact contract. |
| `inject_error_wins` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:880`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L880) | Read the source/rustdoc for the exact contract. |
| `upload_roundtrip_persists_file` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:891`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L891) | Read the source/rustdoc for the exact contract. |
| `sharefolder_and_listshares` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:924`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L924) | Read the source/rustdoc for the exact contract. |
| `notifications_mark_read` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:939`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L939) | Read the source/rustdoc for the exact contract. |
| `createbackup_and_stopdevice` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:967`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L967) | Read the source/rustdoc for the exact contract. |
| `getfilepublink_and_listpubs` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:988`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L988) | Read the source/rustdoc for the exact contract. |
| `shutdown_joins_thread` | `private` | fn | [`crates/pcloud-mockserver/src/lib.rs:1022`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/src/lib.rs#L1022) | Read the source/rustdoc for the exact contract. |
| `http_get` | `private` | fn | [`crates/pcloud-mockserver/tests/mock_flows.rs:29`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/tests/mock_flows.rs#L29) | Read the source/rustdoc for the exact contract. |
| `http_post` | `private` | fn | [`crates/pcloud-mockserver/tests/mock_flows.rs:53`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/tests/mock_flows.rs#L53) | Read the source/rustdoc for the exact contract. |
| `userinfo_flow_against_mock` | `private` | fn | [`crates/pcloud-mockserver/tests/mock_flows.rs:83`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/tests/mock_flows.rs#L83) | Read the source/rustdoc for the exact contract. |
| `upload_create_write_save_roundtrip_against_mock` | `private` | fn | [`crates/pcloud-mockserver/tests/mock_flows.rs:92`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/tests/mock_flows.rs#L92) | Read the source/rustdoc for the exact contract. |
| `listshares_then_sharefolder_then_listshares_against_mock` | `private` | fn | [`crates/pcloud-mockserver/tests/mock_flows.rs:120`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/tests/mock_flows.rs#L120) | Read the source/rustdoc for the exact contract. |
| `notifications_list_and_read_against_mock` | `private` | fn | [`crates/pcloud-mockserver/tests/mock_flows.rs:143`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/tests/mock_flows.rs#L143) | Read the source/rustdoc for the exact contract. |
| `backup_create_and_device_stop_against_mock` | `private` | fn | [`crates/pcloud-mockserver/tests/mock_flows.rs:172`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/tests/mock_flows.rs#L172) | Read the source/rustdoc for the exact contract. |
| `getfilepublink_and_listpubs_against_mock` | `private` | fn | [`crates/pcloud-mockserver/tests/mock_flows.rs:189`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/tests/mock_flows.rs#L189) | Read the source/rustdoc for the exact contract. |
| `inject_error_and_invalid_token_paths_against_mock` | `private` | fn | [`crates/pcloud-mockserver/tests/mock_flows.rs:221`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-mockserver/tests/mock_flows.rs#L221) | Read the source/rustdoc for the exact contract. |

## Usage guidance

This package proves behavior and is not a shipped end-user runtime surface.
