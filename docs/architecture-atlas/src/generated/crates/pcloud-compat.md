# `pcloud-compat`

**Maturity:** Experimental / bounded

**Version:** `0.1.0`

**Directory:** `crates/pcloud-compat`

**Manifest:** [`crates/pcloud-compat/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/Cargo.toml)

C-CLI compatibility shim primitives (rpc_message_t codec, SysV shm producer). Isolated crate, not wired into the daemon by default.

## Feature-family profile

**Why it exists.** Isolate the small legacy C-client ABI surfaces that are still useful during migration.

**What it is good for.** Decoding legacy rpc_message_t frames and, when explicitly enabled, producing the old SysV shared-memory folder-list layout.

**Why it is good at that job.** Byte-exact codecs live outside the canonical daemon so compatibility cannot silently constrain modern internal design.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_compat` | lib | [`crates/pcloud-compat/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/lib.rs) |
| `pcloud-compat-shm-peek` | bin | [`crates/pcloud-compat/src/bin/shm_peek.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/bin/shm_peek.rs) |
| `cross_process_shm` | test | [`crates/pcloud-compat/tests/cross_process_shm.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/tests/cross_process_shm.rs) |

## Direct dependencies

`libc`, `thiserror`

## Cargo features

| Feature | Enables |
|---|---|
| `default` | empty marker |
| `legacy-shm` | empty marker |

## File inventory (8)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-compat/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/Cargo.toml) | Cargo manifest | **PLATFORM MATRIX:** |
| [`crates/pcloud-compat/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/README.md) | documentation | pcloud-compat |
| [`crates/pcloud-compat/src/bin/shm_peek.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/bin/shm_peek.rs) | Rust module | Tiny helper used by the cross-process integration test. |
| [`crates/pcloud-compat/src/folder_list.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs) | Rust module | ABI-exact mirror of the legacy `psync_folder_list_t` shared-memory payload. |
| [`crates/pcloud-compat/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/lib.rs) | library root | `pcloud-compat` — C-to-Rust IPC compatibility primitives. |
| [`crates/pcloud-compat/src/rpc_codec.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs) | Rust module | Binary codec for the C `rpc_message_t` frame. |
| [`crates/pcloud-compat/src/shm_producer.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs) | Rust module | SysV shared-memory producer matching the legacy `pclsync/pshm.c` layout. |
| [`crates/pcloud-compat/tests/cross_process_shm.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/tests/cross_process_shm.rs) | test | Cross-process shm integration test. |

## Rust declaration index (82 total; 41 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `main` | `private` | fn | [`crates/pcloud-compat/src/bin/shm_peek.rs:17`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/bin/shm_peek.rs#L17) | Read the source/rustdoc for the exact contract. |
| `main` | `private` | fn | [`crates/pcloud-compat/src/bin/shm_peek.rs:49`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/bin/shm_peek.rs#L49) | Read the source/rustdoc for the exact contract. |
| `PSYNC_MAX_PATH_LENGTH` | `pub` | const | [`crates/pcloud-compat/src/folder_list.rs:50`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L50) | Maximum path length mirrored from `PSYNC_MAX_PATH_LENGTH` in `pclsync/pfoldersync.h`. Includes the trailing N… |
| `CSizeT` | `pub` | type | [`crates/pcloud-compat/src/folder_list.rs:56`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L56) | Mirror of `size_t` on the target platform. The legacy C payload uses `size_t foldercnt`; on Linux x86_64 that… |
| `FolderEntry` | `pub` | struct | [`crates/pcloud-compat/src/folder_list.rs:66`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L66) | ABI-exact mirror of the C `psync_folder_t` struct. `#\[repr(C)\]` is load-bearing: it pins the field order and… |
| `fmt` | `private` | fn | [`crates/pcloud-compat/src/folder_list.rs:84`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L84) | Read the source/rustdoc for the exact contract. |
| `default` | `private` | fn | [`crates/pcloud-compat/src/folder_list.rs:96`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L96) | Read the source/rustdoc for the exact contract. |
| `FolderListHeader` | `pub` | struct | [`crates/pcloud-compat/src/folder_list.rs:118`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L118) | Header of the flexible-array C struct `psync_folder_list_t`. The raw serialized buffer is `FolderListHeader`… |
| `FolderListError` | `pub` | enum | [`crates/pcloud-compat/src/folder_list.rs:125`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L125) | Errors that can be produced by the folder-list builder. |
| `FolderListBuilder` | `pub` | struct | [`crates/pcloud-compat/src/folder_list.rs:149`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L149) | Safe builder that accumulates \[`FolderEntry`\] values and emits an ABI-exact `psync_folder_list_t` serialized… |
| `new` | `pub` | fn | [`crates/pcloud-compat/src/folder_list.rs:155`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L155) | Construct an empty builder. |
| `len` | `pub` | fn | [`crates/pcloud-compat/src/folder_list.rs:162`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L162) | Current number of queued entries. |
| `is_empty` | `pub` | fn | [`crates/pcloud-compat/src/folder_list.rs:167`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L167) | Whether no entries have been queued yet. |
| `push` | `pub` | fn | [`crates/pcloud-compat/src/folder_list.rs:174`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L174) | Queue a pre-built entry. Callers using this variant are responsible for ensuring path buffers are NUL-termina… |
| `push_paths` | `pub` | fn | [`crates/pcloud-compat/src/folder_list.rs:181`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L181) | Build and push an entry from individual fields. The string slices are length-checked and NUL-scanned before b… |
| `build` | `pub` | fn | [`crates/pcloud-compat/src/folder_list.rs:205`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L205) | Serialize the accumulated entries into an ABI-exact `psync_folder_list_t` buffer (header + flexible array). |
| `decode_roundtrip` | `pub` | fn | [`crates/pcloud-compat/src/folder_list.rs:243`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L243) | Minimal Rust-side decoder used by the round-trip test: reads a buffer produced by \[`FolderListBuilder::build`… |
| `copy_cstr` | `private` | fn | [`crates/pcloud-compat/src/folder_list.rs:279`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L279) | Read the source/rustdoc for the exact contract. |
| `cstr_preview` | `private` | fn | [`crates/pcloud-compat/src/folder_list.rs:300`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L300) | Read the source/rustdoc for the exact contract. |
| `_` | `private` | const | [`crates/pcloud-compat/src/folder_list.rs:313`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L313) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-compat/src/folder_list.rs:341`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L341) | Read the source/rustdoc for the exact contract. |
| `hand_crafted_fixture` | `private` | fn | [`crates/pcloud-compat/src/folder_list.rs:347`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L347) | Hand-crafted fixture mirroring exactly what a C producer using `psync_folder_list_t` would write: little-endi… |
| `builder_matches_hand_crafted_fixture` | `private` | fn | [`crates/pcloud-compat/src/folder_list.rs:387`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L387) | Read the source/rustdoc for the exact contract. |
| `roundtrip_decode_preserves_fields` | `private` | fn | [`crates/pcloud-compat/src/folder_list.rs:417`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L417) | Read the source/rustdoc for the exact contract. |
| `empty_builder_serializes_to_header_only` | `private` | fn | [`crates/pcloud-compat/src/folder_list.rs:443`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L443) | Read the source/rustdoc for the exact contract. |
| `path_too_long_is_rejected` | `private` | fn | [`crates/pcloud-compat/src/folder_list.rs:455`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L455) | Read the source/rustdoc for the exact contract. |
| `interior_nul_is_rejected` | `private` | fn | [`crates/pcloud-compat/src/folder_list.rs:476`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L476) | Read the source/rustdoc for the exact contract. |
| `layout_sizes_match_c_abi_runtime` | `private` | fn | [`crates/pcloud-compat/src/folder_list.rs:488`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L488) | Read the source/rustdoc for the exact contract. |
| `push_raw_entry_is_serialized_verbatim` | `private` | fn | [`crates/pcloud-compat/src/folder_list.rs:499`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/folder_list.rs#L499) | Read the source/rustdoc for the exact contract. |
| `folder_list` | `pub` | mod | [`crates/pcloud-compat/src/lib.rs:101`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/lib.rs#L101) | Read the source/rustdoc for the exact contract. |
| `rpc_codec` | `pub` | mod | [`crates/pcloud-compat/src/lib.rs:102`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/lib.rs#L102) | Read the source/rustdoc for the exact contract. |
| `shm_producer` | `pub` | mod | [`crates/pcloud-compat/src/lib.rs:108`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/lib.rs#L108) | Read the source/rustdoc for the exact contract. |
| `POVERLAY_BUFSIZE` | `pub` | const | [`crates/pcloud-compat/src/rpc_codec.rs:25`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L25) | C `POVERLAY_BUFSIZE` — maximum frame size the C server tolerates. |
| `HEADER_SIZE` | `pub` | const | [`crates/pcloud-compat/src/rpc_codec.rs:30`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L30) | Offset of `value\[\]` inside the C `rpc_message_t`. `uint32_t` (4) + 4 bytes of natural-alignment padding + `ui… |
| `RpcOpcode` | `pub` | enum | [`crates/pcloud-compat/src/rpc_codec.rs:38`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L38) | Command opcodes from `pclsync/pcommands.h` (values 20..=32). The enum is `u32`-repr so it matches the wire `t… |
| `as_u32` | `pub` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:70`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L70) | Numeric opcode on the wire. |
| `ALL` | `pub` | const | [`crates/pcloud-compat/src/rpc_codec.rs:75`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L75) | All known opcodes in declaration order. |
| `Error` | `private` | type | [`crates/pcloud-compat/src/rpc_codec.rs:93`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L93) | Read the source/rustdoc for the exact contract. |
| `try_from` | `private` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:95`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L95) | Read the source/rustdoc for the exact contract. |
| `UnknownOpcode` | `pub` | struct | [`crates/pcloud-compat/src/rpc_codec.rs:118`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L118) | Unknown opcode. |
| `RpcMessage` | `pub` | struct | [`crates/pcloud-compat/src/rpc_codec.rs:126`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L126) | Decoded `rpc_message_t`. `length` on the wire is total frame bytes (header + value); the struct stores the `v… |
| `CodecError` | `pub` | enum | [`crates/pcloud-compat/src/rpc_codec.rs:138`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L138) | Codec errors. |
| `new` | `pub` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:174`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L174) | Construct a message from an opcode and value bytes. |
| `encode` | `pub` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:189`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L189) | Encode to an owned byte buffer in the C wire layout. The `length` field on the output frame is always compute… |
| `decode` | `pub` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:210`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L210) | Decode a frame from a buffer. Returns the parsed message and the number of bytes consumed (equal to the decla… |
| `tests` | `private` | mod | [`crates/pcloud-compat/src/rpc_codec.rs:246`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L246) | Read the source/rustdoc for the exact contract. |
| `fixture_addsync` | `private` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:252`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L252) | Hand-built host-endian wire fixture: opcode 24 (ADDSYNC), value "a\|b". Constructed without using the encoder… |
| `opcode_numeric_values_match_c_header` | `private` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:262`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L262) | Read the source/rustdoc for the exact contract. |
| `opcode_try_from_roundtrip` | `private` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:280`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L280) | Read the source/rustdoc for the exact contract. |
| `header_size_is_sixteen_bytes` | `private` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:291`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L291) | Read the source/rustdoc for the exact contract. |
| `decode_fixture_matches_expected` | `private` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:297`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L297) | Read the source/rustdoc for the exact contract. |
| `encode_then_decode_is_identity` | `private` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:307`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L307) | Read the source/rustdoc for the exact contract. |
| `encode_matches_hand_built_fixture` | `private` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:318`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L318) | Read the source/rustdoc for the exact contract. |
| `encode_rejects_oversized_payload` | `private` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:324`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L324) | Read the source/rustdoc for the exact contract. |
| `decode_short_header_errors` | `private` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:334`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L334) | Read the source/rustdoc for the exact contract. |
| `decode_rejects_length_under_header` | `private` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:342`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L342) | Read the source/rustdoc for the exact contract. |
| `decode_rejects_length_over_limit` | `private` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:353`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L353) | Read the source/rustdoc for the exact contract. |
| `decode_rejects_truncation` | `private` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:363`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L363) | Read the source/rustdoc for the exact contract. |
| `empty_value_roundtrip` | `private` | fn | [`crates/pcloud-compat/src/rpc_codec.rs:374`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/rpc_codec.rs#L374) | Read the source/rustdoc for the exact contract. |
| `PSYNC_SHM_SIZE` | `pub` | const | [`crates/pcloud-compat/src/shm_producer.rs:70`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L70) | `PSYNC_SHM_SIZE` from `pclsync/pshm.h`. |
| `FTOK_PROJ_ID` | `pub` | const | [`crates/pcloud-compat/src/shm_producer.rs:73`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L73) | `ftok` project-id byte used by the C client. |
| `LEGACY_SHM_MODE` | `pub` | const | [`crates/pcloud-compat/src/shm_producer.rs:76`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L76) | Legacy compat mode (`0666`). World-accessible; see module docs. |
| `PsyncShm` | `private` | struct | [`crates/pcloud-compat/src/shm_producer.rs:91`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L91) | Header struct mirroring the C `psync_shm` layout on 64-bit Linux. * `data` (`*mut c_void`, 8 bytes) — the C c… |
| `_` | `private` | const | [`crates/pcloud-compat/src/shm_producer.rs:98`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L98) | Read the source/rustdoc for the exact contract. |
| `ShmError` | `pub` | enum | [`crates/pcloud-compat/src/shm_producer.rs:106`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L106) | Errors from the shm producer. |
| `default_anchor_path` | `pub` | fn | [`crates/pcloud-compat/src/shm_producer.rs:145`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L145) | Compute the canonical ftok anchor path used by the C client: `$HOME/.pcloud/data.db`. |
| `ftok_key` | `pub` | fn | [`crates/pcloud-compat/src/shm_producer.rs:154`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L154) | Compute the SysV IPC key from an anchor path, matching C `get_key()`. |
| `ShmSegment` | `pub` | struct | [`crates/pcloud-compat/src/shm_producer.rs:183`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L183) | RAII owner of a SysV shm segment. The segment is attached for the lifetime of this value and detached on drop… |
| `create` | `pub` | fn | [`crates/pcloud-compat/src/shm_producer.rs:210`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L210) | Attach to (creating if necessary) the legacy-layout shm segment. `mode` is the permission bits to pass to `sh… |
| `max_payload` | `pub` | fn | [`crates/pcloud-compat/src/shm_producer.rs:265`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L265) | Maximum payload bytes publishable in one \[`write`\](Self::write) call. |
| `write` | `pub` | fn | [`crates/pcloud-compat/src/shm_producer.rs:275`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L275) | Publish `data` into the shm payload area and set `flag = 1` with SEQ_CST semantics, matching `pshm_write()` i… |
| `try_consume` | `pub` | fn | [`crates/pcloud-compat/src/shm_producer.rs:310`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L310) | Read-back helper for same-process tests — performs the mirror of `pshm_read`: checks `flag == 1`, copies `dat… |
| `mark_for_removal` | `pub` | fn | [`crates/pcloud-compat/src/shm_producer.rs:335`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L335) | Mark the shm segment for removal when the last attacher detaches. This matches `pshm_cleanup()` in C and is a… |
| `shmid` | `pub` | fn | [`crates/pcloud-compat/src/shm_producer.rs:352`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L352) | The SysV shm identifier. Primarily for diagnostics / tests. |
| `size` | `pub` | fn | [`crates/pcloud-compat/src/shm_producer.rs:357`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L357) | Total segment size (always \[`PSYNC_SHM_SIZE`\]). |
| `drop` | `private` | fn | [`crates/pcloud-compat/src/shm_producer.rs:363`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L363) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-compat/src/shm_producer.rs:373`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L373) | Read the source/rustdoc for the exact contract. |
| `psync_shm_layout_is_stable` | `private` | fn | [`crates/pcloud-compat/src/shm_producer.rs:380`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L380) | Layout assertion: `PsyncShm` occupies exactly 24 bytes on 64-bit Linux. The compile-time guard above already… |
| `max_payload_matches_c_definition` | `private` | fn | [`crates/pcloud-compat/src/shm_producer.rs:386`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L386) | Read the source/rustdoc for the exact contract. |
| `write_then_consume_roundtrip` | `private` | fn | [`crates/pcloud-compat/src/shm_producer.rs:400`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L400) | End-to-end write + read-back in a single process. Uses a temp file as the ftok anchor and a unique project id… |
| `write_rejects_oversized_payload` | `private` | fn | [`crates/pcloud-compat/src/shm_producer.rs:428`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/src/shm_producer.rs#L428) | Read the source/rustdoc for the exact contract. |
| `second_process_reads_shm_payload` | `private` | fn | [`crates/pcloud-compat/tests/cross_process_shm.rs:25`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-compat/tests/cross_process_shm.rs#L25) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.
