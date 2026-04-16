# ADR 0013: OPA/Rego Policy Evaluation via `regorus` (Not a Custom DSL)

- Status: Accepted
- Date: 2026-04-16

## Context

Enterprise deployments want to express policy over daemon actions:
"only the `managed-device` fleet may set up crypto folders",
"public links must expire within 7 days", "deny sync-root add when the
folder is outside the allowed residency region". The rewrite needs a
policy layer with the following properties:

- operator-writable rules, not compiled Rust code;
- testable with a golden set before rollout;
- default-deny on empty or unmatched bundles;
- hot-reloadable without daemon restart;
- standard enough that a SOC team can review it without learning a
  bespoke DSL;
- no CGO dependency, no subprocess, no non-Rust runtime on the hot
  path — `pcloudd` must stay a single static binary.

The path-of-least-resistance options were:

1. Invent a small in-house DSL (YAML-ish with a match/allow grammar).
2. Embed the reference OPA engine via CGO or subprocess.
3. Use a pure-Rust Rego evaluator.

## Decision

The daemon evaluates policy with `regorus = "0.3"`, a pure-Rust Rego
engine, wrapped in `pcloud-policy::RegoPolicyEngine`.

Invariants landed in `pcloud-policy`:

1. **Default-deny** on empty bundles or unmatched rules. Missing
   `allow` is never treated as "allow".
2. **File-permission guard** on policy-file load: rejects world-write
   (`0o022`), non-root-owned, and escaping-symlink policy files before
   the engine ever sees their contents.
3. **Transactional hot-reload**: the previous engine is retained in
   full until a new bundle compiles cleanly. A syntactically invalid
   reload never unseats a working policy.
4. **Object-safe** `PolicyEngine` trait so enterprise consumers can
   swap the implementation without a rebuild of the daemon.
5. Four example bundles ship under
   `crates/pcloud-policy/examples/policies/`: `default-deny`,
   `allow-all`, `publink-expiry-7d`, `crypto-setup-managed-device`.

## Consequences

Good:

- Operators write standard Rego. SOC review tooling, OPA test
  harnesses, the upstream Rego language server, and existing policy
  libraries are immediately applicable.
- `pcloudd` stays a single static binary — no CGO, no sidecar, no
  subprocess, no separate service to manage.
- Hot-reload path is safe by construction: partial updates cannot
  create an "allow everything" window.
- Default-deny keeps the failure mode conservative: a broken bundle
  denies rather than silently opens.
- Policy engine is swappable via the trait, so an enterprise tier can
  plug in a remote PDP if one is ever required.

Bad:

- `regorus` is younger than the reference OPA Go engine; we track its
  versions closely and gate upgrades on a vendored test corpus.
- Rego has a learning curve for operators who have never used OPA.
  Mitigated by the four ready-to-adopt example bundles and
  `docs/enterprise/policy.md`.
- The pure-Rust evaluator may lag behind the reference OPA on very
  new language features. For the policies shipped (allow/deny over
  request attributes) that is acceptable.

## Alternatives Considered

- **Custom YAML DSL**: rejected — writing a DSL is the easy part;
  writing a test harness, a language server, documentation, and a
  security-review track for it is not. Every minute spent on a
  bespoke DSL is a minute not spent on parity.
- **CGO-bound OPA**: rejected — breaks the "single static binary"
  shape, complicates cross-platform builds, and introduces a Go
  runtime to the daemon's failure domain.
- **OPA as a subprocess / sidecar**: rejected — turns every policy
  check into a subprocess round-trip, adds lifecycle complexity,
  and creates a new trust boundary to secure.
- **Hard-coded Rust policy**: rejected — makes every rule change a
  daemon release; defeats the purpose.
