# ADR 0017: JSON-in-`message` Response Shape for Structured IPC Payloads

- Status: Accepted
- Date: 2026-04-16

## Context

Some IPC responses return structured data rather than a simple
human-readable status string:

- `list-links` — an array of public-link records;
- `create-links` — the freshly-created link record;
- `integrity status` — counters per result kind and an `audit_drops`
  field (see the integrity sweeper design note);
- `backup snapshot-*` — manifest fragments, verification results,
  prune plans.

The IPC wire format (ADR 0002) is deliberately minimal: binary
length-prefixed frames carrying a serde-encoded `ResponseStatus`. The
status carries an `ok: bool`, an `error_code` taxonomy value, and a
`message: String`. Adding a separate `payload` field to every
structured response would:

- rev the wire format across every untouched response shape (breaking
  older SDK consumers, invalidating fuzz corpora, requiring a coordinated
  update across daemon + SDK + CLI);
- require every response site — including purely human ones like
  `doctor`, `config show`, `mount --force-umount` — to carry an
  `Option<Payload>` that is always `None`.

At the same time we must:

- keep structured responses **machine-parseable** without mandating
  post-processors like `jq` on the client side;
- keep the CLI human-readable output path unchanged;
- preserve bit-for-bit wire compatibility for every unrelated response.

## Decision

Structured IPC responses carry their payload as a **JSON document
serialised into the existing `message` field** of `ResponseStatus`.
The CLI discriminates based on `--json` (machine) vs default (human)
output and on the response's `method`.

Concretely:

1. The daemon serialises the payload (e.g.
   `IntegrityStatusPayload`, `Vec<PublicLinkRecord>`) to a compact
   UTF-8 JSON string and places it in `message`.
2. The human-readable `message` text is not sacrificed; it is either
   composed from the payload at the CLI layer (`pcloudc` formats the
   JSON into a table) or, for simple responses, the message is a
   plain string and no JSON is emitted.
3. `--json` on the CLI emits the raw JSON verbatim from `message`
   with no transformation, so downstream consumers parse the exact
   bytes the daemon produced.
4. Each structured-payload method documents its payload schema in
   `docs/book/src/reference/ipc-protocol.md` and in the corresponding
   manpage entry.
5. **Field-selector support** is native to `pcloudc --select FIELD`
   (see ADR 0018); we do not require `jq` or any external post-
   processor to extract individual fields for scripting.

## Consequences

Good:

- No wire-format rev; every untouched response shape stays
  byte-identical.
- Clients that understand the method parse the JSON; clients that
  don't still see a deterministic, UTF-8 `message`.
- `--json` gives a stable, machine-parseable stdout contract without
  the daemon learning a second output format.
- Plays well with the traceparent envelope (ADR 0012): envelope-level
  concerns stay at the envelope; payload concerns stay in `message`.

Bad:

- Clients must know which methods carry JSON payloads vs plain text.
  Mitigated by explicit schema docs per method and by the CLI being
  the primary consumer (it always knows).
- Double-encoding: the JSON payload is a string inside another
  serialised struct. Cost is negligible at observed sizes; a future
  ADR may introduce a sibling `payload: Option<Value>` once the
  wire format revs for another reason.

## Alternatives Considered

- **Add `payload: Option<serde_json::Value>` to `ResponseStatus`**:
  rejected for now — reves the wire format across every unrelated
  response, invalidates fuzz corpora, forces an SDK release. Will
  revisit when the wire revs for another reason.
- **Second response type** (`DataResponse` vs `StatusResponse`):
  rejected — doubles the dispatch surface and every CLI formatter.
- **Mandate `jq` on the client side**: rejected — adds a runtime
  dependency for scripted users, and is hostile to Windows operators
  who may not have `jq`. ADR 0018 covers native field selectors
  instead.
