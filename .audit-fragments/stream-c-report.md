# Stream C — Transport HIGH fixes + parity rows 26 / 27 / 93

**Date:** 2026-04-26
**Audit wave:** audit-06 (Dimensions 4 & 6 — sync engine & transport)
**Scope:** H-4.2 idempotency, H-6.1 timeout composition, H-6.2 Retry-After,
parity rows 26 / 27 / 93.

## Findings closed

### H-4.2 — Upload retry idempotency (HIGH)

**Source:** `.audit-fragments/04-06-sync-and-transport.md` §4 H-4.2
(double-write risk on retry across `upload_create → upload_write →
upload_save`).

**Resolution.** Threaded an optional client-generated idempotency key
through the entire upload three-phase wire protocol:

- `pcloud_proto::methods::upload::UploadCreateRequest`,
  `UploadWriteRequest`, `UploadSaveRequest`,
  `UploadWriteFromFileRequest` each gained an `idempotency_key:
  Option<String>` field. When set, the encoder emits an extra
  `idempotencykey` binary parameter; when `None`, the legacy wire
  format is preserved.
- New idempotent entry points on `pcloud_proto::transfer_api::TransferApi`:
  `upload_create_idempotent`, `encode_upload_write_from_file_idempotent`.
- `pcloud_backends::transfer_backend::ChunkedUploadDriver` now
  generates a stable per-session 128-bit hex key via `getrandom` at
  construction (`new_idempotency_key`), stores it on the driver, and
  threads the same value through every `upload_create`, `upload_write`,
  `upload_save` call. The chunked retry loop re-uses the same key so a
  network retry cannot produce a double-write.
- Single-shot `upload_bytes` and the FUSE write-path leave the field
  `None` (those paths have no in-flight retry surface).

**Tests.** Six new unit tests in
`crates/pcloud-proto/src/methods/upload.rs` exercise parameter-count
deltas with and without the key (4→5 on create, 4→5 on write, 6/8/9 on
save, 8→9 on writefromfile).

### H-6.1 — Timeout composition validation (HIGH)

**Resolution.** New free function
`pcloud_config::api::validate_timeout_composition(connect, read, total)`
returns `Err(ConfigError::InvalidTimeoutComposition(...))` when
`connect > read` or (when `total` is supplied) `read > total`.
Wired into `ApiEndpoint::validate` so existing config-load paths
fail at the boundary instead of inside the hot path.

**Tests.** Six tests in `crates/pcloud-config/src/api.rs`:
well-ordered triple, equality at boundaries, `connect > read`
rejection, `read > total` rejection, optional-total skip, and a
full `validate_endpoint_rejects_inverted_connect_read_pair`
end-to-end smoke through `ApiEndpoint::validate`.

### H-6.2 — Retry-After IMF-fixdate parsing (HIGH)

**Resolution.** Extended
`pcloud_resilience::transport::parse_retry_after_header` to honour
the RFC 7231 §7.1.3 IMF-fixdate (HTTP-date) form alongside the
existing delta-seconds form. Implementation is dependency-free
(no `chrono` / `time` pull-in): a small positional parser plus
Howard Hinnant's "days_from_civil" algorithm to compute the Unix
timestamp. Far-future dates clamp to the 300s cap; past dates and
malformed values return `None`. RFC 850 / asctime forms are
intentionally unsupported (RFC 7231 mandates senders produce
IMF-fixdate); they return `None` so a misformatted header degrades
to the standard client-computed backoff.

**Tests.** Five new tests in `crates/pcloud-resilience/src/transport.rs`
cover past dates, future dates, far-future clamping, and the
malformed matrix (empty, garbage, negative, infinity, NaN). A
`format_imf_fixdate` test helper round-trips a known timestamp to
guard the parser against silent regressions.

### Row 93 — `upload_writefromfile` server-side copy IPC (Partial → Implemented)

**Resolution.** Closed the bd-1du row 93 gap:

1. **Backend.**
   `pcloud_backends::transfer_backend::TransferRuntime::upload_write_from_file`
   encodes a `UploadWriteFromFileRequest` against the live
   `BinaryApiTransport` and classifies the server result code into
   `TransferBackendError::PermanentResultCode` /
   `TransferBackendError::TransientResultCode`. Honours
   `PSYNC_MAX_COPY_FROM_REQ` at the boundary; emits a stable
   idempotency key per call.
2. **Daemon handler.**
   `pcloud_daemon::runtime::PCloudDaemon::upload_write_from_file_ipc`
   (formerly a stub returning `Unavailable`) now resolves the
   authenticated session token, derives a stable correlation
   `chunk_id` from the destination offset, and routes the call
   through `TransferRuntime::upload_write_from_file`.
3. **CLI.** New `Command::UploadWriteFromFile` plus the
   `pcloudc upload write-from-file <UPLOAD_ID> <SOURCE_FILEID>
   <SOURCE_HASH> <OFFSET> <COUNT>` parser. Aliases:
   `upload-write-from-file`, `upload-writefromfile`,
   `upload writefromfile`.
4. **Tests.**
   `network_upload_write_from_file_drives_server_side_copy` in
   `crates/pcloud-backends/src/transfer_backend.rs` exercises the
   end-to-end success path against a TCP mock server and asserts
   the wire frame carries `upload_writefromfile`, `uploadid`,
   `fileid`, `hash`, `offset`, `count`, **and the new
   `idempotencykey` byte sequence**.
   `network_upload_write_from_file_rejects_oversized_count` proves
   the `PSYNC_MAX_COPY_FROM_REQ` precondition short-circuits before
   any bytes hit the socket.
5. **Proptest.** The pre-existing IPC roundtrip in
   `crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs` already
   covers `Request::UploadWriteFromFile` (all five field-arbitrary
   combinations); no change required.

### Rows 26 / 27 — `psync_tfa_has_devices` / `psync_tfa_type` (already Rejected)

No code change. Both rows had already flipped to `Rejected` under
audit-06 ncx.4 (2026-04-19) — they are C-desktop-UI helpers with no
enterprise Rust analog on the daemon-over-IPC surface
(`send_two_factor_notification` returns the device list in-band, and
method dispatch is caller-driven via `send_two_factor_sms` /
`send_two_factor_notification` / `submit_two_factor_code`). The CSV
already reflects this — rows 23 (`psync_tfa_has_devices`) and 24
(`psync_tfa_type`) carry `Rejected` with rationales pointing at
`REJECTED-RATIONALES-14042026.md`. The fragment plan listed them
under their CSV-numeric labels (26 / 27) but the names are
authoritative and no further work is necessary.

## Parity matrix delta

| Field         | Before stream-c   | After stream-c    |
|---------------|-------------------|-------------------|
| Implemented   | 153               | 154               |
| Partial       | 3 (93 / 124 / 142)| 2 (124 / 142)     |
| Missing       | 0                 | 0                 |
| Rejected      | 30                | 30                |

`STATUS.md` was updated to add a 2026-04-26 stream-c section and
correct the headline to **154 / 2 / 0 / 30 (186 rows)**.

`C_FEATURE_PARITY_MATRIX.csv` row 93 was edited in place to flip
`Partial` → `Implemented` and document the new code paths.

## Files modified (in scope)

- `crates/pcloud-proto/src/methods/upload.rs` — H-4.2 fields + tests.
- `crates/pcloud-proto/src/methods/mod.rs` — H-4.2 struct-init fix.
- `crates/pcloud-proto/src/transfer_api.rs` — `upload_create_idempotent`,
  `encode_upload_write_from_file_idempotent`.
- `crates/pcloud-backends/src/transfer_backend.rs` — `upload_write_from_file`,
  `new_idempotency_key`, integration tests.
- `crates/pcloud-backends/Cargo.toml` — added workspace `getrandom` dep.
- `crates/pcloud-resilience/src/transport.rs` — H-6.2 IMF-fixdate parser
  + tests.
- `crates/pcloud-config/src/api.rs` — H-6.1 `validate_timeout_composition`
  + tests + `ApiEndpoint::validate` integration.
- `crates/pcloud-config/src/lib.rs` — `ConfigError::InvalidTimeoutComposition`.
- `crates/pcloud-cli/src/commands.rs` — `Command::UploadWriteFromFile`
  + inputs fields + IPC build.
- `crates/pcloud-cli/src/app.rs` — `upload write-from-file` subcommand
  parser + canonical token map + positional-arg parser.
- `STATUS.md` — 2026-04-26 stream-c section + headline.
- `C_FEATURE_PARITY_MATRIX.csv` — row 93 flipped to Implemented.

## Files modified (out-of-strict-scope, but required by task)

- `crates/pcloud-daemon/src/runtime.rs` — `upload_write_from_file_ipc`
  upgraded from stub to real handler. The user task explicitly
  required "finish it" — the handler had to leave the stub state for
  row 93 to be honestly closed.
- `crates/pcloud-fs/src/backend.rs` — three call sites of
  `UploadCreateRequest` / `UploadWriteRequest` / `UploadSaveRequest`
  needed `idempotency_key: None` filled in to keep the workspace
  compiling after the proto field addition. No behavioural change;
  the FUSE path explicitly opts out of the new idempotency key
  (its retry discipline lives elsewhere, per audit-06 wording in the
  in-line comment).
- `crates/pcloud-sdk/src/upload_session.rs` — fixed an unrelated
  pre-existing brace mismatch (`if let Err(err) = journal.clear()
  { ... }` was missing the closing brace) that was blocking the
  workspace from compiling. Detected when running the
  required-by-task `cargo check`. No semantic change.

## Verification

```sh
cargo check  -p pcloud-proto -p pcloud-backends -p pcloud-resilience \
             -p pcloud-config -p pcloud-ipc -p pcloud-cli -p pcloud-sdk
# Finished `dev` profile [unoptimized + debuginfo] target(s).

cargo test   -p pcloud-proto -p pcloud-resilience -p pcloud-ipc --lib
# 70 + 39 + 5 passed; 0 failed.

cargo test   -p pcloud-config --lib api
# 15 passed; 0 failed (including six new H-6.1 tests).

cargo test   -p pcloud-backends --lib network_upload
# 3 passed; 0 failed (including the two new row-93 integration tests).
```

`cargo fmt` was run on every touched crate; no formatting drift remains.

## Pre-existing failures NOT introduced by stream-c

Four `pcloud-config` loader tests fail with
`SchemaViolations("... 'pause_on_battery' (additionalProperties=false)")`.
That is a sibling agent's incomplete schema change in
`crates/pcloud-config/src/sync_loop.rs`; the `pause_on_battery`
field was added to the typed struct but not to the JSON schema
fixture. Confirmed unrelated by stashing the stream-c changes —
the same four tests fail on `35eedd9` clean tree.

## Constraints honoured

- **No `.unwrap()` outside tests.** All new production code uses
  `?` / `match` / `Option::unwrap_or_default` / explicit error
  propagation. The two test helpers that use `.expect(...)` are
  test-side only.
- **Proptest coverage on new IPC variants.** No new `Request`
  variants were added in this stream (the existing
  `Request::UploadWriteFromFile` was already covered). The proto
  field additions are covered by the new parameter-count unit tests.
- **`cargo fmt` on touched crates.** Done.
