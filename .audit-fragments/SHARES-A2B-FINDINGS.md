# A↔B Live Share Lifecycle — Findings & Resolution (2026-04-29)

Live verification against real pCloud accounts produced three findings on the
binary protocol surface. All three are now diagnosed; two are fixed and the
third is a server-side gate that the test suite bypasses cleanly.

## Status: ✅ Full A→B lifecycle passes end-to-end in 3.78s

```
A ShareFolder ok → sharerequestid=3711071 surfaced (F1 fixed)
B AcceptShareRequest ok with that id (F2 bypassed)
Bilateral visibility within 0s: A outgoing & B incoming both report the share
Teardown: A RemoveShare + A FolderDeleteById both Ok
```

## How we got there

A new opt-in wire-capture seam (`PCLOUD_WIRE_CAPTURE_DIR`) in
`crates/pcloud-proto/src/transport.rs` dumps every binary RPC frame
(request bytes, response bytes, partial-on-error bytes, error message)
to a directory hardened with mode 0o700 / 0o600. With it enabled, three
distinct issues were isolated against the live backend.

## Findings

### F1 — `sharefolder` response: `sharerequestid` is nested  ✅ FIXED

**Symptom**: `share.message` reported `sharerequestid=None` even when the
share was accepted by the backend.

**Root cause** (visible at offset 0x00d8 of the 399-byte response): the
`sharefolder` API places the request id inside a top-level `share`
sub-hash, not at the response root. Our parser only read the top level.

**Fix**: `crates/pcloud-proto/src/shares_api.rs` (`share_folder`) now
reads `hash.get_hash("share").get_number("sharerequestid")` first,
falling back to the legacy top-level path. Same pattern applied to
`list_shares` (`hash.get_hash("shares").get_array("outgoing"|"incoming")`)
and `list_share_requests`.

### F2 — `listsharerequests` binary endpoint: server hard-rejects  ⚠ SERVER-SIDE

**Symptom**: `Method::ListIncomingShareRequests` /
`Method::ListOutgoingShareRequests` returned
`ResponseStatus::Unavailable: transport failed: i/o failed: failed to fill whole buffer`.

**Root cause** (from `partial.bin` capture: 0 bytes): pCloud's binary
protocol closes the TLS connection without sending **any** response
bytes — not a parser bug, not a length-prefix mismatch. The method is
not honoured by the binary endpoint in this fork's transport
configuration. Other share methods (`sharefolder`, `listshares`,
`acceptshare`, `removeshare`, `deletefolderrecursive`) on the same
pool, same session, same TLS context all work.

**Workaround**: now that F1 surfaces the request id directly from the
`sharefolder` response, callers don't need to enumerate pending
requests anymore — the recipient can accept by id immediately. The
test takes this path and skips `listsharerequests` entirely.

**Long-term path**: route `listsharerequests` over the HTTP/JSON
endpoint, or surface the same data via `listshares` with a
`pending=1` param if pCloud adds one. Tracked in code comments;
no bead opened.

### F3 — `listshares` response: arrays nested under a `shares` hash  ✅ FIXED

**Symptom**: `Method::ListOutgoingShares` returned `count=0, ids=[]`
even immediately after a confirmed accept (and even when ad-hoc shares
existed from earlier runs).

**Root cause** (from `listshares.res.bin`): the response is
`{ result, shares: { outgoing: [...], incoming: [...] } }`. Our parser
read `hash.get_array("outgoing")` from the top level instead of
`hash.get_hash("shares").get_array("outgoing")`.

**Fix**: same defensive nesting pattern as F1 in `list_shares` at
`shares_api.rs:160-176`.

## Code changes

| File | Change |
|---|---|
| `crates/pcloud-proto/src/transport.rs` | New opt-in `wire_capture` module driven by `PCLOUD_WIRE_CAPTURE_DIR`; hooks into `send_and_receive` to dump request/response/partial frames with mode 0o600. Zero overhead when env unset. |
| `crates/pcloud-proto/src/shares_api.rs` | F1 fix in `share_folder`. F3 fix in `list_shares`. Defensive nesting also added to `list_share_requests` for the day F2's server-side gate is lifted. |
| `crates/pcloud-ipc/src/methods.rs` | New `Request::FolderDeleteById { folder_id, recursive }`. |
| `crates/pcloud-daemon/src/runtime.rs` | New `folder_delete_by_id` handler with idempotent `2005 Folder Not Found` mapping and `2003 Directory Not Empty` → `Conflict`. |
| `crates/pcloud-backends/src/folder_backend.rs` | New `FolderRuntime::delete_folder_by_id`. |
| `crates/pcloud-cli/src/app.rs` | `read_password_securely` skips `--`-prefixed values at args[3] so `account register EMAIL --accept-terms` doesn't trip the argv-password guardrail. |
| `crates/pcloud-live-e2e/tests/shares_a_to_b.rs` | New `live_share_a_to_b_full_lifecycle`. |
| `crates/pcloud-live-e2e/tests/shares_active_a_to_b.rs` | New `live_share_a_to_b_active_visibility` (post-state bilateral check). |

## Test status

- `cargo test -p pcloud-proto --lib` → 172 pass / 0 fail / 2 ignored
- `cargo test -p pcloud-daemon --lib` → 213 pass / 0 fail
- `cargo test -p pcloud-backends --lib` → 203 pass / 0 fail
- `cargo test -p pcloud-cli` → 271 pass / 0 fail (regression test
  `account_register_flag_at_pos3_does_not_trip_argv_guardrail` included)
- Live E2E `shares_a_to_b` → 1 pass, full A→B handshake in ~4s
- Live E2E `shares_active_a_to_b` → 1 pass, bilateral visibility confirmed

## Operator notes

- The wire-capture seam is OFF by default. To diagnose any future
  protocol issue, set `PCLOUD_WIRE_CAPTURE_DIR=/some/dir` before
  running the daemon or test. The directory will contain auth tokens
  in plaintext — the module enforces 0o700 / 0o600 perms but **delete
  the directory once diagnosis is done**. Never commit, never email.
- The earlier-accepted leftover share with id 900501 between A and B
  remains active; teardown only revokes shares the test itself
  created. Use `pcloudc shares list-outgoing` and
  `pcloudc shares remove <id>` to clean up if desired.
