# ADR 0018: Native Field-Selector Syntax (Not `jq`)

- Status: Accepted
- Date: 2026-04-16

## Context

Operators scripting `pcloudc` routinely want a single field out of a
structured response — "give me just the public-link URL from
`create-link`", "give me just the `audit_drops` counter from
`integrity status`". The three realistic options are:

1. Make the operator pipe into `jq`:

   ```sh
   pcloudc list-links --json | jq -r '.[0].code'
   ```

2. Teach `pcloudc` a small native field-selector syntax:

   ```sh
   pcloudc list-links --select '[0].code'
   ```

3. Ship multiple per-field convenience flags for every method.

Constraints:

- operators on Windows, BSD, and minimal container images often lack
  `jq` (and certainly lack a specific `jq` version);
- scripting the CLI is a first-class use case — doctor checks, CI
  pipelines, fleet-agent hooks, packaging post-install tests;
- adding per-field flags to every method is combinatorial and ages
  poorly;
- the output must remain deterministic so downstream consumers can
  parse it without regex surgery.

## Decision

`pcloudc` ships a **native field-selector** flag, `--select <EXPR>`,
implemented in the CLI layer on top of the JSON payload described in
ADR 0017.

Syntax (intentionally a subset of the `jq` path grammar, not full
`jq`):

- `.field` — object field access.
- `[N]` — integer array index, zero-based; supports negative
  indices (`[-1]` is last).
- Chaining — `.records[0].url`, `[0].nested.field`.
- `--raw-select` — when the selected value is a string, print the
  unquoted bytes (no surrounding quotes, no JSON escape).

Behavioural rules:

1. When the selector misses, `pcloudc` exits `6 Unavailable` with a
   structured error on stderr, not a silent empty line.
2. When the selector matches a composite value, the CLI emits that
   value as canonical JSON (sorted keys, no extra whitespace).
3. `--select` composes with `--json`: `--json --select FOO` emits the
   selected value as JSON. `--select FOO` alone emits the human-
   oriented projection (unquoted strings for scalars).
4. `--select` is CLI-local. The daemon never sees the selector; it
   always returns the full payload via the ADR 0017 shape.
5. No `jq` dependency is introduced anywhere in the runtime, tests,
   or packaging.

## Consequences

Good:

- No `jq` runtime dependency — works identically on Linux, macOS,
  Windows, and the BSDs, under any packaging (minimal containers,
  AppImage, MSI, `.pkg`).
- Scripts stay stable: the selector grammar is part of the CLI
  surface and we version it the same way we version any other CLI
  flag.
- Deterministic missing-selector handling: `6 Unavailable` lets CI
  pipelines fail loudly instead of silently returning empty strings.
- The grammar is small enough to document in the manpage in a
  paragraph and small enough to implement without pulling in a
  general-purpose expression evaluator.

Bad:

- We own the selector parser. If operators request new syntax
  (filters, maps, pipes), we have to choose between implementing
  them and redirecting to `jq`. The current charter is "selectors
  only"; filters and pipes are out of scope by default.
- Operators already fluent in `jq` must learn the subset boundary.
  Documented in the manpage with a side-by-side examples section.

## Alternatives Considered

- **Mandate `jq`**: rejected — hostile to Windows, BSD, and minimal
  container operators, and binds us to a specific external tool
  version for test determinism.
- **Per-field convenience flags** (`--code`, `--url`, `--count`, …):
  rejected — combinatorial, ages poorly, and means every new
  structured response grows the CLI surface.
- **Embed a full `jq` runtime** (`jaq`, `rsjsonpath`): considered;
  rejected for pre-alpha. Full `jq` semantics are a large dependency
  surface we do not need for the current scripting load. Revisit if
  operators ask for filter/map/pipe semantics beyond selectors.
