# `pcloud-sdk`

**Maturity:** Stable public contract

**Version:** `1.0.0`

**Directory:** `crates/pcloud-sdk-public`

**Manifest:** [`crates/pcloud-sdk-public/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/Cargo.toml)

Stable blocking Rust SDK for pCloud drive operations through pcloudd.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_sdk` | lib | [`crates/pcloud-sdk-public/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs) |

## Direct dependencies

`base64`, `pcloud-ipc`, `serde`, `serde_json`, `thiserror`

## Cargo features

No declared package features.

## File inventory (3)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-sdk-public/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-sdk-public/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/README.md) | documentation | pcloud-sdk |
| [`crates/pcloud-sdk-public/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs) | library root | Stable, filesystem-focused pCloud SDK. |

## Rust declaration index (51 total; 33 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `RequestSender` | `private` | type | [`crates/pcloud-sdk-public/src/lib.rs:24`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L24) | Read the source/rustdoc for the exact contract. |
| `VERSION` | `pub` | const | [`crates/pcloud-sdk-public/src/lib.rs:27`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L27) | Package version of the stable SDK contract. |
| `Client` | `pub` | struct | [`crates/pcloud-sdk-public/src/lib.rs:35`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L35) | Blocking client for an owner-authenticated `pcloudd` endpoint. Clones share the immutable transport callback… |
| `fmt` | `private` | fn | [`crates/pcloud-sdk-public/src/lib.rs:41`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L41) | Read the source/rustdoc for the exact contract. |
| `new` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:57`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L57) | Configure a client for the daemon endpoint at `socket_path`. Construction performs no I/O. On Unix this is th… |
| `remote` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:70`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L70) | Borrow the stable remote-drive API. |
| `dispatch` | `private` | fn | [`crates/pcloud-sdk-public/src/lib.rs:74`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L74) | Read the source/rustdoc for the exact contract. |
| `with_sender` | `private` | fn | [`crates/pcloud-sdk-public/src/lib.rs:79`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L79) | Read the source/rustdoc for the exact contract. |
| `RemoteEntryId` | `pub` | enum | [`crates/pcloud-sdk-public/src/lib.rs:93`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L93) | Stable, kind-carrying identifier for a remote drive entry. |
| `value` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:103`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L103) | Return the numeric pCloud id. |
| `is_folder` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:111`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L111) | Return whether this identifies a folder. |
| `RemoteEntry` | `pub` | struct | [`crates/pcloud-sdk-public/src/lib.rs:119`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L119) | Owned metadata for one remote drive entry. |
| `RemoteListing` | `pub` | struct | [`crates/pcloud-sdk-public/src/lib.rs:145`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L145) | An authoritative folder listing. |
| `RemoteRead` | `pub` | struct | [`crates/pcloud-sdk-public/src/lib.rs:155`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L155) | A bounded range read and its EOF metadata. |
| `RemoteCopyResult` | `pub` | struct | [`crates/pcloud-sdk-public/src/lib.rs:167`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L167) | Aggregate counters from a remote recursive copy. |
| `RemoteUploadResult` | `pub` | struct | [`crates/pcloud-sdk-public/src/lib.rs:179`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L179) | Receipt for a streamed local-to-remote upload. |
| `RemoteDownloadResult` | `pub` | struct | [`crates/pcloud-sdk-public/src/lib.rs:195`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L195) | Receipt for a streamed remote-to-local download. |
| `SharePermissions` | `pub` | struct | [`crates/pcloud-sdk-public/src/lib.rs:209`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L209) | Typed permissions for a folder-share invitation. |
| `READ_ONLY` | `pub` | const | [`crates/pcloud-sdk-public/src/lib.rs:222`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L222) | Read-only permissions. |
| `READ_WRITE` | `pub` | const | [`crates/pcloud-sdk-public/src/lib.rs:230`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L230) | Read/write permissions without share administration. |
| `to_bits` | `private` | fn | [`crates/pcloud-sdk-public/src/lib.rs:237`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L237) | Read the source/rustdoc for the exact contract. |
| `default` | `private` | fn | [`crates/pcloud-sdk-public/src/lib.rs:246`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L246) | Read the source/rustdoc for the exact contract. |
| `ShareOptions` | `pub` | struct | [`crates/pcloud-sdk-public/src/lib.rs:254`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L254) | Options for sharing one remote folder. |
| `new` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:264`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L264) | Create a read-only invitation for `recipient`. |
| `message` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:275`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L275) | Attach a human-readable invitation message. |
| `permissions` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:282`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L282) | Set the recipient's permissions. |
| `hint` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:289`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L289) | Attach an optional pCloud share hint. |
| `Error` | `pub` | enum | [`crates/pcloud-sdk-public/src/lib.rs:298`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L298) | Errors returned by the stable SDK. |
| `RemoteDrive` | `pub` | struct | [`crates/pcloud-sdk-public/src/lib.rs:332`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L332) | Borrowed, focused view of a \[`Client`\]'s remote drive. |
| `stat` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:338`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L338) | Resolve authoritative metadata for an absolute remote path. |
| `list` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:348`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L348) | List the immediate children of an absolute remote folder path. |
| `read_range` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:372`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L372) | Read a bounded byte range from a remote file. One call is capped by the daemon at 8 MiB. Use consecutive rang… |
| `upload` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:403`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L403) | Stream a local regular file to an absolute remote destination. |
| `download` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:424`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L424) | Stream a remote file into a crash-safe local destination. |
| `copy` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:446`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L446) | Recursively copy a remote file or folder tree. |
| `move_path` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:462`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L462) | Rename or move a remote file or folder. |
| `delete` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:473`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L473) | Idempotently delete a remote file or folder. |
| `mkdir` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:483`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L483) | Create one remote folder. |
| `share_folder` | `pub` | fn | [`crates/pcloud-sdk-public/src/lib.rs:492`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L492) | Share a remote folder with an email recipient. |
| `send` | `private` | fn | [`crates/pcloud-sdk-public/src/lib.rs:510`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L510) | Read the source/rustdoc for the exact contract. |
| `validate_remote_path` | `private` | fn | [`crates/pcloud-sdk-public/src/lib.rs:515`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L515) | Read the source/rustdoc for the exact contract. |
| `successful_body` | `private` | fn | [`crates/pcloud-sdk-public/src/lib.rs:524`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L524) | Read the source/rustdoc for the exact contract. |
| `decode_payload` | `private` | fn | [`crates/pcloud-sdk-public/src/lib.rs:540`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L540) | Read the source/rustdoc for the exact contract. |
| `entry_from_stat` | `private` | fn | [`crates/pcloud-sdk-public/src/lib.rs:545`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L545) | Read the source/rustdoc for the exact contract. |
| `entry_from_listing` | `private` | fn | [`crates/pcloud-sdk-public/src/lib.rs:569`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L569) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-sdk-public/src/lib.rs:593`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L593) | Read the source/rustdoc for the exact contract. |
| `ok` | `private` | fn | [`crates/pcloud-sdk-public/src/lib.rs:603`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L603) | Read the source/rustdoc for the exact contract. |
| `stat` | `private` | fn | [`crates/pcloud-sdk-public/src/lib.rs:610`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L610) | Read the source/rustdoc for the exact contract. |
| `stat_list_and_range_use_only_canonical_requests` | `private` | fn | [`crates/pcloud-sdk-public/src/lib.rs:633`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L633) | Read the source/rustdoc for the exact contract. |
| `mutation_and_transfer_payloads_are_sdk_owned` | `private` | fn | [`crates/pcloud-sdk-public/src/lib.rs:671`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L671) | Read the source/rustdoc for the exact contract. |
| `errors_and_paths_do_not_leak_ipc_types` | `private` | fn | [`crates/pcloud-sdk-public/src/lib.rs:742`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-sdk-public/src/lib.rs#L742) | Read the source/rustdoc for the exact contract. |

## Usage guidance

This is the intended third-party SemVer boundary. The daemon must be running and authenticated; registry release qualification is tracked separately.
