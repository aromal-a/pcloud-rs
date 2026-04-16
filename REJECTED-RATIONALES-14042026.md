# Rejected Rationales — C-to-Rust Parity Matrix

Date: 2026-04-14
Source matrix: `C_FEATURE_PARITY_MATRIX.csv`
Total rows with status `Rejected`: **28** (matrix rows 2, 5, 6, 10, 12, 13, 43, 44, 45, 46, 99, 100, 101, 102, 103, 104, 105, 106, 113, 114, 115, 126, 151, 152, 157, 160, 167, 169).

## How to read this document

If you are new to the parity effort:

- **"Rejected" does not mean "broken"** in the Rust rewrite. It means
  the legacy C symbol was inspected on this fork and a deliberate
  decision was made *not* to mirror it. Each entry below cites the
  exact C header/source location and justifies the verdict.
- **Nothing here changes a verdict.** This file is a companion to
  `C_FEATURE_PARITY_MATRIX.csv` — the CSV is the machine-readable
  source of truth for *which* rows are rejected; this document
  explains *why*, one symbol at a time.
- **Rationale categories** are introduced directly below; every
  rejected row is tagged with one of them, which is the quickest way
  to orient yourself when skimming.
- **Counts live in [`STATUS.md`](./STATUS.md)**, per ADR 0009. If this
  file's "28" count drifts from `STATUS.md`, `STATUS.md` is the one
  you trust.

This document gives a verified per-symbol rationale for every Rejected row. Each entry was checked against the C source body (or absence thereof) on this fork. No row status was changed; this doc only justifies them.

Categories used:

- **Ghost** — declared in `psynclib.h` but no compiled body in this fork.
- **Stub** — body exists but is a no-op (`return 0;`, queue-event passthrough, etc.).
- **Replaced** — superseded by typed Result/event-stream/IPC notifications in Rust.
- **Billing-out-of-scope** — billing/subscription surface; not part of the crypto runtime parity slice.
- **C-internal-plumbing** — internal warmup, refresh, allocator, or UI bridge with no public Rust analog.
- **Insecure-legacy-not-carried-forward** — legacy behavior that conflicts with the secure default posture.
- **Typo-duplicate** — header typo or duplicate-prefixed symbol.

---

## Category: Ghost (declared, no body)

### Row 2 — `psync_set_alloc`
- C: `pclsync/psynclib.h:589`.
- Body search across `pclsync/*.c` returns no implementation; only the header declaration exists. No translation unit defines `psync_set_alloc`.
- Category: **Ghost** + **C-internal-plumbing** (allocator override).
- Rust replacement: none required. The Rust workspace uses the global allocator; per-call allocator injection is a C-runtime hook with no enterprise-Rust analog. Not exposed in `pcloud-sdk` or `pcloud-daemon`.

### Row 102 — `psync_check_new_version`
- C: `pclsync/psynclib.h:1081`.
- No body in `pclsync/*.c` (grep across the tree returns header + docs only).
- Category: **Ghost**.
- Rust replacement: none. Update-check delivery is intentionally out of the daemon scope; release distribution is a packaging concern (`packaging/`).

### Row 103 — `psync_check_new_version_str`
- C: `pclsync/psynclib.h:1079`. Same: declaration only, no body.
- Category: **Ghost**.
- Rust replacement: none, see Row 102.

### Row 104 — `psync_check_new_version_download`
- C: `pclsync/psynclib.h:1087`. Header-only.
- Category: **Ghost**.
- Rust replacement: none.

### Row 105 — `psync_check_new_version_download_str`
- C: `pclsync/psynclib.h:1084`. Header-only.
- Category: **Ghost**.
- Rust replacement: none.

### Row 106 — `psync_run_new_version`
- C: `pclsync/psynclib.h:1088`. Header-only.
- Category: **Ghost**.
- Rust replacement: none. Self-update is delegated to OS package managers / installer pipelines.

### Row 157 — `psync_sow_link`
- C: `pclsync/psynclib.h:1349` (referenced in comments only). Grep across `pclsync/*.c` returns no definition. Header documentation refers to it; the actual exported symbol is `psync_show_link` (Implemented).
- Category: **Typo-duplicate** + **Ghost**.
- Rust replacement: `pcloud-cli` exposes `show_link` only.

### Row 160 — `psync_psync_change_link`
- C: `pclsync/psynclib.h:1443`. Body exists at `pclsync/psynclib.c:2038`, but the symbol carries the duplicated `psync_psync_` prefix and merely forwards to `do_psync_change_link`. The non-duplicated public surface (`psync_change_link_*`) is the retained API.
- Category: **Typo-duplicate**.
- Rust replacement: `pcloud-daemon/src/public_link_backend.rs` exposes the typed `change_public_link` operation; no double-prefixed alias is mirrored.

### Row 100 — `psync_add_device_monitor_callback`
- C: `pclsync/psynclib.h:1554` — the declaration is **commented out**: `//  void psync_add_device_monitor_callback(device_event_callback callback);`.
- Category: **Ghost** (never compiled into the library).
- Rust replacement: none. No active C consumer; not part of any retained surface.

### Row 101 — `psync_list_devices`
- C: `pclsync/psynclib.h:1556` — also commented out: `//  pdevice_item_list_t * psync_list_devices(char **err /*OUT*/);`.
- Category: **Ghost**.
- Rust replacement: none. Device listing is not exposed by the legacy library and is not mirrored.

---

## Category: Stub (no-op body)

### Row 10 — `psync_download_state`
- C: `pclsync/psynclib.h:598`. Body at `pclsync/psynclib.c:252` literally reads:
  `uint32_t psync_download_state() { return 0; }`
- Category: **Stub**.
- Rust replacement: per-transfer state is exposed by `pcloud-daemon/src/transfer_backend.rs` over typed IPC; consumers query real progress instead of a globally-stubbed integer.

---

## Category: Replaced (by typed Result / structured event stream)

### Row 5 — `psync_set_notification_callback`
- C: `pclsync/psynclib.h:594`. Body at `pclsync/psynclib.c:243` forwards to `pnotify_set_callback`. Pattern: register a single global C function pointer.
- Category: **Replaced**.
- Rust replacement: notifications are surfaced as structured event records over local IPC (`pcloud-daemon` event stream), not via global C function pointers — eliminates use-after-free and concurrency hazards.

### Row 6 — `psync_init_data_event_handler`
- C: `pclsync/psynclib.h:1606`. Body at `pclsync/psynclib.c:2705`: `void psync_init_data_event_handler(void *ptr) { ptevent_init(ptr); }` — wires a `void *` blob into the C event subsystem.
- Category: **Replaced** + **C-internal-plumbing**.
- Rust replacement: typed event subscriptions in the daemon runtime; no untyped `void *` registration is exposed.

### Row 12 — `psync_get_last_error`
- C: `pclsync/psynclib.h:756`. Body at `pclsync/psynclib.c:113`: `uint32_t psync_get_last_error() { return psync_error; }` — global `errno`-style integer.
- Category: **Replaced** + **Insecure-legacy-not-carried-forward** (thread-unsafe global state).
- Rust replacement: every fallible operation in `pcloud-proto`, `pcloud-daemon`, and `pcloud-sdk` returns a typed `Result<_, Error>`; no global last-error register exists.

### Row 13 — `psync_network_exception`
- C: `pclsync/psynclib.h:947`. Body at `pclsync/psynclib.c:1153`: `void psync_network_exception() { ptimer_notify_exception(); }`.
- Category: **Replaced**.
- Rust replacement: transport errors are typed and propagated through `pcloud-proto`; the API client retries internally rather than asking callers to nudge a global timer.

### Row 44 — `psync_register_account_events_callback`
- C: `pclsync/psynclib.h:1520`. Body at `pclsync/psynclib.c:2073` simply forwards to `do_register_account_events_callback`.
- Category: **Replaced**.
- Rust replacement: account events flow through the daemon's structured event stream; callers subscribe via IPC, not by registering a C function pointer.

### Row 45 — `psync_register_backup_events_callback`
- C: `pclsync/psynclib.h:1522`. No matching `psync_register_backup_events_callback(` body in `pclsync/*.c` (grep finds only the header declaration). Where backup-event delivery exists, it goes via `pqevent_queue_eventid` directly.
- Category: **Replaced** + **Ghost** (declared but not defined in this fork).
- Rust replacement: backup events surface through the same typed event stream as account/sync events.

---

## Category: C-internal plumbing (UI bridge / cache warmup / refresh hooks)

### Row 43 — `psync_ptools_create_backend_event`
- C: `pclsync/psynclib.h:1599`. Body at `pclsync/psynclib.c:2696` forwards to `ptools_create_backend_event` with `psync_my_auth` and a per-call timestamp — internal telemetry plumbing.
- Category: **C-internal-plumbing**.
- Rust replacement: structured audit events are emitted by the daemon directly (see `pcloud-daemon` audit/event paths); callers do not poke at telemetry from outside.

### Row 46 — `psync_async_ui_callback`
- C: `pclsync/psynclib.h:739`. Body at `pclsync/psynclib.c:2534` rate-limits a UI event id and posts it to `pqevent_queue_eventid`. Used as a thread entrypoint via `prun_thread1("psync_async_sync_delete", psync_async_ui_callback, …)` in `pclsync/pfs.c:2685`.
- Category: **C-internal-plumbing** (UI bridge).
- Rust replacement: the Rust CLI talks directly to the daemon over IPC; rate-limited UI dispatch is unnecessary because there is no shared in-process UI loop.

### Row 99 — `psync_send_backup_del_event`
- C: `pclsync/psynclib.h:736`. Body at `pclsync/psynclib.c:2593` rate-limits and posts `PEVENT_BKUP_F_DEL_NOTSYNCED` / `PEVENT_BKUP_F_DEL_SYNCED` to the queue. Called from `pclsync/plocalscan_helpers.c:126`.
- Category: **C-internal-plumbing** (UI event bridge).
- Rust replacement: backup-deletion events are emitted as structured audit events on the daemon stream.

### Row 126 — `psync_update_cryptostatus`
- C: `pclsync/psynclib.h:1541`. Body at `pclsync/pbusinessaccount.c:694` issues a `userinfo` API call solely to refresh DB-cached `cryptosubscription` / `cryptoexpires` settings.
- Category: **C-internal-plumbing** (push-refresh hook).
- Rust replacement: `pcloud-daemon` exposes `GetCryptoStatus` which pulls live state on demand; there is no need for an external "please refresh" entrypoint.

### Row 167 — `psync_cache_links_all`
- C: `pclsync/psynclib.h:1396`. Body at `pclsync/psynclib.c:2000` is a time-throttled prewarm of the in-memory link cache, with a debug log when called too soon.
- Category: **C-internal-plumbing** (cache warmup).
- Rust replacement: the Rust public-link backend issues the appropriate API call on demand and does not expose a prewarm entrypoint.

### Row 169 — `psync_cache_bookmarks`
- C: `pclsync/psynclib.h:1438`. Body at `pclsync/psynclib.c:2025` forwards to `do_cache_bookmarks` for cache warmup.
- Category: **C-internal-plumbing** (cache warmup).
- Rust replacement: bookmarks are fetched on demand by the Rust SDK; no warmup helper is exposed.

### Row 151 — `psync_delete_all_links_folder`
- C: `pclsync/psynclib.h:1393`. Body at `pclsync/psynclib.c:1992` calls `do_delete_all_folder_links`, which iterates the local pfs link cache that the Rust daemon deliberately does not mirror.
- Category: **C-internal-plumbing** (depends on a local link cache the Rust runtime has chosen not to maintain).
- Rust replacement: explicit `list_public_links` + per-link `delete_public_link` / `delete_upload_link` provide the same effective cleanup with no hidden in-memory cache contract.

### Row 152 — `psync_delete_all_links_file`
- C: `pclsync/psynclib.h:1394`. Body at `pclsync/psynclib.c:1996` calls `do_delete_all_file_links`. Same cache dependency as Row 151.
- Category: **C-internal-plumbing**.
- Rust replacement: same as Row 151 — explicit list + per-link delete.

---

## Category: Billing out-of-scope (crypto subscription surface)

### Row 113 — `psync_crypto_hassubscription`
- C: `pclsync/psynclib.h:1221`. Body at `pclsync/psynclib.c:1707` reads `setting WHERE id='cryptosubscription'`. Pure billing/subscription surface.
- Category: **Billing-out-of-scope**.
- Rust replacement: not in the crypto runtime parity slice. Account/billing data is reachable via the typed `userinfo` path (`pcloud-daemon/src/auth_backend.rs`).

### Row 114 — `psync_crypto_isexpired`
- C: `pclsync/psynclib.h:1223`. Body at `pclsync/psynclib.c:1712` compares `cryptoexpires` against `ptimer_time()` — billing expiry check used in `pfs.c` to gate plaintext fallback.
- Category: **Billing-out-of-scope**.
- Rust replacement: no equivalent gate is required because the Rust crypto path does not silently fall back to plaintext on subscription lapse; expiry is enforced at the userinfo layer.

### Row 115 — `psync_crypto_expires`
- C: `pclsync/psynclib.h:1227`. Body at `pclsync/psynclib.c:1719` returns the cached `cryptoexpires` timestamp.
- Category: **Billing-out-of-scope**.
- Rust replacement: same as Row 114; subscription timestamps are exposed through the typed account surface, not from the crypto runtime.

---

## Verification

- Every header line cited above was checked in `pclsync/psynclib.h` at the listed line.
- Every body claim ("ghost", "stub", "forwarder", "rate-limited UI bridge") was verified by reading the corresponding location in `pclsync/psynclib.c`, `pclsync/pbusinessaccount.c`, `pclsync/pfs.c`, or `pclsync/plocalscan_helpers.c`.
- No `Rejected` row was reclassified; this document only justifies them.

Cross-references:

- Matrix: `C_FEATURE_PARITY_MATRIX.csv`
- Narrative: `C_FEATURE_PARITY_REVIEW.md`
- Final-parity wave docs: `FINAL-PARITY-PROOF-WAVE7-14042026.md`
