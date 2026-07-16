# Future open-source pCloud-like API surface

## Why this document exists

`pcloud-rs` is a third-party Rust client for **pCloud** — a closed,
proprietary cloud-storage product. We have **no access** to the pCloud
backend; the public HTTP/binary API is the only contract we can target,
and that contract is stable, narrow, and under upstream control.

Several "best-in-the-world" enhancement ideas surfaced during the
CLAUDEREV tier-implementation campaign turned out to **require backend
features pCloud does not expose** (or does not document for third-party
clients). Building scaffolds against those nonexistent features is
pointless — the scaffolds rot, mislead future maintainers, and never
actually run because the wire surface is missing.

This document captures those enhancement ideas as a **design brief for
a future open-source pCloud-equivalent** (a self-hostable cloud-storage
system that this client could target as a second backend). When such a
backend exists, the foundations live in this document, not in
unreachable scaffolds.

## Removed scaffolds (per 2026-05-01 directive)

The following code was removed because it built mock backends or
client-side scaffolds against pCloud APIs that do not exist:

- **T1.2 — file-version listing + restore.**
  `RevisionProvider` trait, `NullRevisionProvider`, `HttpRevisionProvider`
  (feature-gated), and the corresponding CLI / IPC / daemon entries.
  pCloud's public API has no `listrevisions` / `revertfile` endpoint
  for third-party clients; the C client uses a session-tied binary
  variant we cannot re-expose safely.
- **T2.1.d — differential-upload execute path.**
  `DeltaUploadTransport` trait + `execute_delta_upload` + in-tree mock.
  pCloud's `upload_writefromfile` endpoint does not expose the byte-range /
  block-index semantics the rsync-style delta encoder produces. The
  codec primitives stay (they're generic library code).
- **T2.2.b — parallel HTTP-range fetcher.**
  `fetch_parallel` + the mock-server `/download` Range route. Pure
  range-fetcher code is generic infrastructure but the campaign built
  it specifically to exercise pCloud's nonexistent multi-thread-friendly
  endpoint at GiB scale. The `plan_ranges` planner stays (it's a pure
  arithmetic helper).
- **T2.6 — QUIC transport.**
  `quinn` workspace dep, `quic` cargo feature, `QuicTransport` scaffold.
  pCloud has no public QUIC endpoint; the localhost-self-signed test
  proved the seam compiles but ran only against a fixture, not pCloud.
  The `transport_protocol` selector + `resolve_after_handshake`
  decision matrix in `pcloud-config` stay (they're a config-shape
  contract independent of any QUIC stack).
- **T4.4 — server-side dedup awareness CLI.**
  `Method::GetStorageSummary` + `StorageSummaryPayload` + the CLI
  `pcloudc storage` command + renderer. pCloud's public `userinfo`
  endpoint does not expose per-account physical bytes or a dedup
  ratio; the renderer would have always omitted the dedup line.

## Enhancement ideas for a future open-source backend

A future open-source cloud-storage system targeting feature parity
with pCloud should consider exposing the following endpoints. Each
entry names the enhancement, the semantics needed, and the existing
client-side foundations in this codebase that a future second-backend
adapter can lean on.

### 1. File-version history + restore (was T1.2)

**Endpoint shapes the client can drive:**

- `GET /file/revisions?file_id=<id>` → array of revisions:
  `{rev_id, mtime, size, user, comment}`.
- `POST /file/restore_revision?file_id=<id>&rev_id=<rev>` → either
  copies the historical revision into the current head OR returns a new
  `file_id` for the materialised revision.

**Why valuable:** revision history is the single largest "Dropbox-class"
feature most operators expect from a sync client; pCloud's web UI
exposes it, but not the public API.

**Client-side foundations that survive in-tree:** `Revision` shape
(simple struct), CLI `revisions list` / `revisions restore` patterns
(documented in `OPERATIONS-RUNBOOK.md` historical sections), and the
`pcloud-rsync` codec already handles the differential transfer story
once a `restore` produces bytes worth syncing back.

### 2. Differential / block-level upload (was T2.1)

**Endpoint shapes the client can drive:**

- `POST /upload/begin_with_baseline?file_id=<src>&new_name=<n>` →
  upload session that lets the client emit either:
  - `upload_write_bytes(offset, body)` for new content, OR
  - `upload_copy_from_baseline(src_offset, len, dest_offset)` for
    server-side copy of existing blocks.
- `POST /upload/finish?session=<sid>` → commits the new revision.

**Why valuable:** edit-1-byte-of-1-GB-file should ship a single 4 KiB
block + index metadata, not the whole file. Roughly 3 orders of
magnitude transfer reduction on typical document edits.

**Client-side foundations that survive in-tree:** `pcloud-rsync` crate
(rolling-hash + signature + delta encoder + apply); the
`UploadStrategy::Delta` plan-side helper in
`pcloud-engine::transfers::differential` remains as the seam where the
adapter would plug in.

### 3. HTTP-range parallel download (was T2.2)

**Endpoint shapes the client can drive:**

- File-content endpoints SHOULD honour standard RFC 7233 `Range:
  bytes=N-M` headers + return `206 Partial Content` with
  `Accept-Ranges: bytes`.

**Why valuable:** GiB-scale cold reads on multi-flow connections
benefit ~4x from N parallel range fetches that reassemble client-side.
A backend that just complies with RFC 7233 unlocks this for free.

**Client-side foundations that survive in-tree:** the `plan_ranges`
planner (pure arithmetic) in `pcloud-proto::parallel_download`.

### 4. QUIC / HTTP/3 transport (was T2.6)

**Endpoint shapes the client can drive:**

- A QUIC-listening endpoint with the same TLS cert chain as the HTTPS
  fallback, on port 443 (or a documented alternate). Service discovery
  via DNS HTTPS / SVCB record + Alt-Svc header.

**Why valuable:** HTTP/3 transport recovers from packet loss faster
on lossy networks and reduces handshake RTT; useful on mobile and
satellite links where the existing TLS + TCP path stalls.

**Client-side foundations that survive in-tree:** the
`TransportProtocol::{Tls, Quic}` + `FallbackPolicy` selector +
`resolve_after_handshake` decision matrix in
`pcloud-config::transport_protocol`.

### 5. Server-side dedup ratio / physical bytes (was T4.4)

**Endpoint shapes the client can drive:**

- `GET /account/storage_summary` → `{logical_bytes_used,
  logical_quota, physical_bytes_used, dedup_ratio}` where physical
  bytes is the post-dedup figure the storage system actually consumes.

**Why valuable:** users with large media collections see dedup ratios
of 1.5-3x in practice; surfacing the figure helps them understand
why their account isn't filling up faster than expected.

**Client-side foundations that survive in-tree:** none — the entire
T4.4 scaffold was removed because it would have always rendered the
non-existent fields as `None`. A future second-backend adapter can
re-introduce the renderer when its `userinfo` / `storage_summary`
endpoint surfaces the fields.

### 6. Listrevisions-aware sync engine (was the implicit T1.2 follow-up)

**Endpoint shapes the client can drive:**

- The diff endpoint SHOULD include revision-id changes alongside
  content changes so a sync client can detect "this file was rolled
  back to a prior revision" without doing its own content comparison.

**Why valuable:** tightens the sync engine's conflict detection — if
the server tells the client "this file's revision id changed but the
content hash is identical to a known prior revision", the client can
skip the download.

**Client-side foundations that survive in-tree:** the conflict
resolver in `pcloud-engine::conflict_resolver` already knows how to
choose between local / remote / rename-both — it would gain a fourth
arm `Restore` once a revision-id-aware diff endpoint exists.

## What stays in this codebase

Every other tier-implementation deliverable is **local, generic, or
targets pCloud's existing public surface** and stays in the codebase:

- **Local-only features** (no server interaction): selective sync,
  conflict UX, bandwidth scheduling (incl. NetworkManager metered
  detection), i18n, encryption-at-rest, per-folder crypto policy,
  WebDAV gateway (talks to local IPC), plugin sandbox (wasmtime
  in-process), distributed tracing (OTLP collector is generic),
  multi-account supervisor, Prometheus alert rules, DR drill scripts,
  capacity planning docs, rustdoc cleanup, fuzz harnesses, Criterion
  cold-start bench, unwrap audit, coverage CI, repro-build CI, memory
  profiling CI.

- **Generic library code** (works against any conformant backend):
  the `pcloud-rsync` codec (rolling-hash + signature + delta encoder),
  the `plan_ranges` byte-range planner, the `TransportProtocol`
  selector + `FallbackPolicy` matrix, the W3C `traceparent` parser.

These foundations are the pieces a future second-backend adapter
plugs into — they don't presume a specific server, they implement
generic primitives the adapter consumes.
