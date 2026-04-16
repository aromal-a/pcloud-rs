# OPA/Rego Policy Layer — Landed

Status: **Landed (unit-tested).** Implementation ships in the
`pcloud-policy` crate. The `PolicyEngine` trait is object-safe and
held behind `Arc<dyn PolicyEngine>` in the daemon runtime. Live
end-to-end production enforcement is **not** claimed — the engine is
wired, the default-deny gate is enforced, the four example policies
below compile and pass their unit tests; operator rollouts still require
their own policy bundles and their own verification.

## 0. What actually landed

- `RegoPolicyEngine` in `crates/pcloud-policy/src/rego.rs` is the
  concrete engine. It links against **`regorus = "0.3"`** (pure Rust,
  no CGO, no subprocess) — the trade-off from §2.3 was kept.
- **Default deny is a safety invariant.** A bundle that contains zero
  matching rules returns `PolicyDecision::Deny { reason: "no matching
  rule" }`. There is no code path on which an empty/failed bundle
  silently allows a request; a failed hot-reload keeps the previous
  engine (see below).
- **Policy file permission guard.** Before compilation, each `.rego`
  file is checked with the same helper used for the auth vault:
  ownership must be the daemon service account (or root), symlinks that
  escape the policy directory are rejected, and any world-write bit
  (`0o022` and related) causes the file to be refused with an audit
  record. Matches the auth-vault `0600` / `0700` posture documented in §8.
- **Four example policies** ship in
  `crates/pcloud-policy/examples/policies/` and are exercised by unit
  tests:
  - `default-deny.rego` — the opinionated baseline,
  - `allow-all.rego` — test fixture; never intended for production,
  - `publink-expiry-7d.rego` — denies `publink.create` with
    `expire_days > 7`,
  - `crypto-setup-managed-device.rego` — requires
    `input.device_id ∈ data.devices.managed` for `crypto.setup`.
- **Hot-reload preserves the previous engine on failure.** Reload is
  transactional: the new bundle is compiled into a fresh
  `regorus::Engine`; the `ArcSwap` is only flipped on successful
  compile. If compile or the permission guard rejects the new bundle,
  the previous engine stays in place and the failure is audited. Under
  no circumstances does a failed reload leave the engine empty — that
  would be fail-open, which is the opposite of what operators want.
- Offline unit tests cover: default-deny on empty bundles, each
  example policy's allow and deny path, file-permission guard
  rejections (world-write, wrong owner, escaping symlink), and
  hot-reload-keeps-previous on compile failure.

Audience: operators deploying `pcloudd` in enterprise environments and
engineers extending the policy subsystem.

---

## 1. Problem Statement

The pcloud-rs daemon's current authorization model is binary: either the
caller holds a valid auth token (and can issue any `Method::*` request) or
they do not. Every authenticated user sees the full API surface — sync
configuration, crypto setup, public-link creation, backup-device removal,
account-scoped mutations, and so on.

This is unacceptable in regulated environments. A concrete set of real
requirements from enterprise deployments we have collected:

- "Only members of the `sync-admins` group may configure sync roots."
- "Public links must expire within 7 days; indefinite links are forbidden."
- "Crypto folders may only be created from managed devices whose
  `device_id` is on the allow-list."
- "Backup device removal is only permitted during business hours, EU/TZ."
- "Finance-tagged paths may never appear in a public link, regardless of
  who the caller is."
- "Every allow *and* every deny must be recorded with the rule id that
  produced the decision, for SOX/ISO-27001 audit."

None of this can be implemented with boolean authentication. We need a
declarative, externally-maintained, auditable policy language — the OPA/Rego
standard is the obvious pick. It is already the reference policy engine for
Kubernetes admission, Envoy external auth, Terraform guardrails, and most
modern zero-trust stacks; operators already know it.

## 2. Architecture

### 2.1 Where the shim sits

The policy layer is a shell that wraps daemon dispatch. The daemon today
routes each incoming IPC request through a single match on the `Method`
enum before handing it to a per-command handler. The new flow:

```
IPC request --> decode --> authenticate --> POLICY EVAL --> handler
                                              |
                                              +--> audit sink (always)
```

Every successful `authenticate` stage builds a `PolicyInput` (see crate
doc) and calls `PolicyEngine::evaluate`. Denied requests short-circuit
with a typed error before any side effect runs — no partial writes, no
network calls, no disk touches.

Critically, the policy layer is *after* authentication but *before* any
handler-specific logic. That means:

- authentication errors surface as auth errors, not policy errors,
- handlers never need policy-aware branches,
- policy is enforced uniformly across every method, including future ones,
  without per-handler boilerplate.

### 2.2 The `PolicyInput` shape

```rust
pub struct PolicyInput {
    pub user: String,
    pub command: String,
    pub args: serde_json::Value,
    pub device_id: Option<String>,
    pub timestamp: SystemTime,
}
```

Rationale for each field:

- `user` — stable account identifier (email or numeric id). Allows
  per-user and per-group rules (groups are resolved from the user
  directory before evaluation).
- `command` — dotted canonical name (`sync.add`, `publink.create`,
  `crypto.setup`). Stable across protocol versions; the mapping from
  `Method::*` to string is defined once in the dispatch shim.
- `args` — JSON projection of the request arguments, scrubbed of any
  secret material. The scrub rule is enforced by the `PolicyInput` builder
  in the dispatch shim, not by the engine.
- `device_id` — optional; populated when the daemon can attest a stable
  device identifier (machine UUID on Linux, host hash fallback otherwise).
  Absence means "unknown device", and policies should treat it as untrusted.
- `timestamp` — wall-clock receipt time. Used by time-of-day rules and to
  defeat replay of stale audit records.

### 2.3 Evaluator choice

Two candidates were considered:

| Option                | Pros                                              | Cons                                                     |
|-----------------------|---------------------------------------------------|----------------------------------------------------------|
| `regorus` (pure Rust) | No CGO, no extra process, cross-platform, fast    | Tracks OPA semantics closely but is not OPA byte-for-byte |
| Shell out to `opa`    | Official, exact semantics, battle-tested          | Extra dependency, forking cost per request, supply chain |

We pick **`regorus`**. The pure-Rust path matches the rewrite's broader
no-CGO / no-extra-process goal, keeps the daemon self-contained, and is
fast enough for per-request evaluation. The trait [`PolicyEngine`] leaves
room to swap in an `OpaSubprocessEngine` later if a customer demands
certified OPA semantics.

### 2.4 Policy examples

```rego
package pcloud.authz
default decision := {"allow": false, "reason": "no matching rule"}

# Only sync-admins may add sync roots.
decision := {"allow": true, "rule": "sync_admins_only"} {
    input.command == "sync.add"
    input.user in data.groups.sync_admins
}

# Public links must expire within 7 days.
decision := {"allow": false, "reason": "publink expiry exceeds 7d", "rule": "publink_expiry"} {
    input.command == "publink.create"
    input.args.expire_days > 7
}

# Crypto setup only on managed devices.
decision := {"allow": true, "rule": "crypto_managed_device"} {
    input.command == "crypto.setup"
    input.device_id
    input.device_id in data.devices.managed
}
```

These are illustrative; the full ruleset ships as an opinionated default
bundle at `/usr/share/pcloud/policy/default.rego`, which operators are
expected to replace.

## 3. Deny-by-default vs allow-by-default

Two modes, both selectable from `[auth.policy].mode`:

- `allow` — the engine's `Allow` decision is required for a request to
  proceed *if* any rule matches; if no rule matches, the request is
  allowed. This is useful for staged rollouts ("observe first").
- `deny` — the engine must produce `Allow`; any other outcome (including
  a default no-match) is a denial. **Production builds lock this on.**

The production lock is enforced at daemon boot: if the build is compiled
with `cfg(feature = "production")` and `mode != "deny"`, the daemon
refuses to start. This prevents a silent misconfiguration from turning
policy off in prod.

## 4. Hot-reload

Policies change. Operators must be able to update rules without daemon
downtime.

- `SIGHUP` triggers a reload of every `.rego` file in the configured
  directory.
- Reload is transactional: the new bundle is compiled into a fresh
  `regorus::Engine`; only on successful compile is the `ArcSwap` flipped.
  Failure logs the compile error and keeps the previous bundle.
- `reload_on_sighup = false` disables the signal hook for environments
  that reserve `SIGHUP` for other uses; reload is then available via
  `pcloudc policy reload` (IPC call, admin-only).

Under no circumstances does a failed reload leave the engine empty —
that would be fail-open, which is the opposite of what operators want.

## 5. Audit

Every decision — allow *and* deny — is appended to the daemon audit log
with:

- the full `PolicyInput` (JSON),
- the matched rule id (`data.pcloud.authz.decision.rule` if set),
- the resulting `PolicyDecision`,
- monotonic sequence number to detect log gaps.

Audit failures are surfaced (not swallowed) per the project's
"no silent audit failures" rule.

## 6. Operator configuration

`config.toml`:

```toml
[auth.policy]
# Directory scanned for *.rego files. Empty => NullPolicyEngine.
path = "/etc/pcloud/policy"

# "deny" (required in prod) | "allow" (staged rollout only).
mode = "deny"

# If true, SIGHUP reloads the bundle; if false, only the admin IPC call does.
reload_on_sighup = true
```

## 7. CLI surface

- `pcloudc policy test <file.rego> --input '<json>'` — evaluate a single
  input against a local `.rego` file. Exits non-zero on deny. No daemon
  round-trip required; used by CI to gate policy changes.
- `pcloudc policy reload` — instructs the running daemon to reload its
  policy bundle. Admin-only; audited.
- `pcloudc policy show` — dumps the currently active bundle metadata
  (file list, mtimes, SHA-256 of each file, mode). Does *not* dump rule
  text (which may contain operator-sensitive group membership hints).

## 8. Trust model for policy files

- Policies live at `/etc/pcloud/policy/*.rego` by default.
- Files MUST be owned by root (or the daemon's dedicated service
  account).
- Files MUST NOT have world-write (`0002`) bits.
- Files MUST NOT be symlinks whose target escapes the policy directory.
- The daemon refuses to load any file that fails these checks and
  records the refusal in the audit log.

These checks mirror the existing auth-vault hardening (`0600` vault,
`0700` parent directory, explicit ownership checks) and use the same
helper utilities.

## 9. Why the engine trait is dependency-light

The `pcloud-policy` crate intentionally depends on only `serde`,
`serde_json`, and `thiserror`. The `regorus` integration will live behind
a feature flag (`rego`) so callers that want only the trait and the
null engine (tests, CLI `policy test`, embedded SDK consumers) do not
pull in the Rego runtime.

## 10. Open questions (tracked)

- Exact group-resolution API — probably a `GroupResolver` trait.
- How far to go with `data.*` side-load (static JSON vs. live lookups
  against the account directory).
- Whether to expose a WASM compile target for `policy test` so operators
  can embed the check in web tooling.

These are deliberately out of scope for the initial scaffold; the crate
as landed encodes enough structure to answer them incrementally without
breaking callers.

## 11. Interface / trait shape

Authoritative declarations:

- `PolicyEngine` trait — `crates/pcloud-policy/src/lib.rs:196`
- `PolicyInput` — `crates/pcloud-policy/src/lib.rs:130`
- `PolicyDecision` — `crates/pcloud-policy/src/lib.rs:145`
- `PolicyError` — `crates/pcloud-policy/src/lib.rs:159`
- `NullPolicyEngine` (allow-by-default fallback used only when the
  feature is disabled by config) — `crates/pcloud-policy/src/lib.rs:240`
- `RegoPolicyEngine` (`regorus`-backed) —
  `crates/pcloud-policy/src/lib.rs:277`
  - `::new(policy_dir)` at `:300`
  - File-perm guard (`0o022`) at `:348`
  - Atomic reload — swaps compiled engine behind a `Mutex` at `:270`
  - Default-deny on error / missing / malformed at `:367`

```rust
// Simplified; see crates/pcloud-policy/src/lib.rs:196.
pub trait PolicyEngine: Send + Sync {
    fn evaluate(&self, input: &PolicyInput) -> PolicyDecision;
    fn reload(&self) -> Result<(), PolicyError>;
}
```

## 12. Configuration reference — every key

```toml
[policy]
provider    = "rego"                 # "null" | "rego"
policy_dir  = "/etc/pcloud/policy"   # 0700 dir, 0600 files, root-owned
reload_mode = "signal"               # "signal" (SIGHUP) | "watch" | "manual"
audit_path  = "/var/log/pcloud-rs/policy-audit.log"
```

| Key            | Type        | Default                     | Purpose                                                                 | Example |
|----------------|-------------|-----------------------------|-------------------------------------------------------------------------|---------|
| `provider`     | enum string | `"null"`                    | `"null"` (allow) or `"rego"` (`RegoPolicyEngine`).                      | `"rego"` |
| `policy_dir`   | string path | `/etc/pcloud/policy`        | Directory scanned for `*.rego` files on boot and reload.                | `/opt/pcloud/policy` |
| `reload_mode`  | enum string | `"signal"`                  | `"signal"` reloads on SIGHUP, `"watch"` uses `notify` inotify, `"manual"` requires CLI. | `"signal"` |
| `audit_path`   | string path | `/var/log/pcloud-rs/policy-audit.log` | JSONL audit sink for every decision. Must be writable by service user. | `/var/log/pcloud-rs/policy.jsonl` |

Files in `policy_dir` must have mode `0o600` or stricter. `0o022`
(group-write, world-write) is rejected at load, see
`crates/pcloud-policy/src/lib.rs:348`.

## 13. Worked example — "reject upload to non-US residency"

`/etc/pcloud/policy/residency.rego`:

```rego
package pcloud.policy

default decision = {"allow": true, "reason": "allow"}

decision = {"allow": false, "reason": reason} {
    input.action == "upload"
    not allowed_region
    reason := sprintf("residency denied: account region=%v", [input.account.region])
}

allowed_region {
    input.account.region == "us-east-1"
}
allowed_region {
    input.account.region == "us-west-2"
}
```

Demo:

```bash
# CI gate — evaluate before deploying the bundle.
pcloudc policy test /etc/pcloud/policy/residency.rego \
  --input '{"action":"upload","account":{"region":"eu-west-1"}}'
# exit 2; stdout: deny: residency denied: account region=eu-west-1

pcloudc policy test /etc/pcloud/policy/residency.rego \
  --input '{"action":"upload","account":{"region":"us-east-1"}}'
# exit 0; stdout: allow
```

## 14. Onboarding recipe

### Beginner — deploy in 5 steps

1. `sudo install -d -m 0700 -o root -g root /etc/pcloud/policy`
2. Copy your `.rego` files to `/etc/pcloud/policy/`, then:
   `sudo chmod 0600 /etc/pcloud/policy/*.rego`
3. Add `[policy] provider = "rego"` to `pcloud-rs.toml`.
4. Validate: `pcloudc policy test` on each file with a benign input.
5. `sudo systemctl reload pcloudcd` (SIGHUP) and watch the audit log
   for `policy.loaded bundle_sha256=...`.

### Expert — Terraform / Ansible

```yaml
- name: deploy pcloud policy bundle
  hosts: pcloud_clients
  tasks:
    - file: { path: /etc/pcloud/policy, state: directory, mode: '0700', owner: root }
    - copy: { src: 'policies/', dest: /etc/pcloud/policy/, mode: '0600', owner: root }
    - command: pcloudc policy test {{ item }} --input '{{ ci_probe_json }}'
      loop: "{{ lookup('fileglob', 'policies/*.rego', wantlist=True) }}"
    - service: { name: pcloudcd, state: reloaded }
```

Gate the Terraform/Ansible run on the `policy test` exit code; a parse
or probe failure must block the deploy before SIGHUP reaches the daemon.

## 15. Verification

1. **Default-deny on empty bundle** — move `policy_dir` out of the way,
   SIGHUP, issue any action, expect deny with reason
   `"default deny"`.
2. **Parse-failure keeps previous policy** —
   `cargo test -p pcloud-policy reload_keeps_previous_on_parse_error`
   exercises the atomic swap (see
   `crates/pcloud-policy/src/lib.rs:270`).
3. **Permission guard** — `chmod 0666 /etc/pcloud/policy/foo.rego` then
   SIGHUP: daemon must refuse and audit `policy.refuse reason=mode`.
4. **Probe action** — `pcloudc policy probe --action upload --region
   eu-west-1` returns deny JSON; `--region us-east-1` returns allow.

## 16. Failure modes + remediation

| Symptom                                    | Root cause                           | Remediation |
|--------------------------------------------|--------------------------------------|-------------|
| All actions deny after reload              | `.rego` parse failure / missing dir  | `journalctl -u pcloudcd | grep policy.reload.failed`; the *previous* bundle is still in memory — fix the file and SIGHUP again. |
| `PolicyError::InsecurePermissions`         | File is group/world-writable         | `chmod 0600`, `chown root:root`, SIGHUP. |
| `PolicyError::InputShape`                  | Caller passed malformed JSON         | The evaluation **defaults to deny** (safe). Inspect caller. |
| High p99 evaluation latency                | Rule set fans out excessively        | Split large bundles; `regorus` recompiles per-SIGHUP, not per-call. |

## 17. Extension points

- **New decision shape.** Add fields on `PolicyInput` (see
  `crates/pcloud-policy/src/lib.rs:130`). Keep it serde-stable; callers
  pass JSON. Never add secret values to the input (they travel through
  the audit log).
- **New engine backend.** Implement `PolicyEngine`
  (`crates/pcloud-policy/src/lib.rs:196`) for your runtime (e.g. WASM,
  CEL). Guard it behind a Cargo feature — the default build stays on
  `NullPolicyEngine`.
- **Group resolver.** Planned `GroupResolver` trait for live directory
  lookups is tracked in §10. Contributors should not land ad-hoc
  lookups inside `decide()`; they must go through the trait.

## 18. Cross-refs

- CLI: `docs/book/src/cli/policy.md`
- Runbook — policy rollback: `docs/runbooks/policy-rollback.md`
- Audit log schema: `docs/book/src/observability/audit.md`
- Secret-handling rules: `crates/pcloud-secret/src/secret_string.rs`
- Parity row: `C_FEATURE_PARITY_MATRIX.csv`
  (`policy.*` rows — `Rejected` on legacy C, net-new in Rust)
