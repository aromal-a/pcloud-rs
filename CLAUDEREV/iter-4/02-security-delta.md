# Security Audit — Iteration 4 Delta

Date: 2026-04-29 (iter-4 delta)
Scope: dim 2 was declared converged in iter-2 and re-confirmed in iter-3
(`CLAUDEREV/iter-3/02-security-delta.md`, "delta count: 0 new findings, 0
retractions, 0 regressions"). Iter-4 verifies the dimension **stays
converged** after the iter-3 fix campaign edits.

## Convergence: YES (third consecutive iteration)

Iter-4 walked three narrow regression checks against the iter-3 fix
edits enumerated in the prompt:

1. **deny.toml prune regression** — did pruning 12 stale `skip` entries
   accidentally unmask a real banned/duplicate version still in the dep
   graph?
2. **systemd companion-doc rewrite** — does the rewritten
   `override.conf.example` / `README.md` text contradict any hardening
   directive (`NoNewPrivileges`, `ProtectSystem`, etc.) still active in
   `pcloudd.service`?
3. **Newly-introduced credential strings** — re-grep every iter-3-edited
   file for the iter-1 secret-shape pattern.

All three checks pass.

---

## Check 1 — `cargo deny check bans` is clean

Ran `cargo deny check bans` against the current tree. Result:

```
bans ok
```

No `multiple-versions` violations, no banned crates, no skipped entries
that fail to match a real version. The 12 prunes from iter-3 did not
unmask anything. The dep graph that was previously masked by the stale
skips no longer contains those duplicates (they were resolved by the
iter-2 / iter-3 dependency unification waves), so removing the skips
is correct hygiene rather than a regression risk.

---

## Check 2 — systemd companion docs do not contradict unit hardening

Grepped `packaging/systemd/README.md`,
`packaging/systemd/override.conf.example`, and
`packaging/systemd/override-user.conf.example` for hardening-directive
names.

- `override.conf.example` — mentions **no** hardening directives at
  all. It is the egress drop-in (re-introduces the
  `IPAddressDeny=any` + `IPAddressAllow=localhost` block as opt-in,
  per iter-3). Net-additive only; cannot contradict any directive in
  the base unit.
- `override-user.conf.example` — *clears* `ProtectSystem=`,
  `ProtectHome=`, `PrivateTmp=` (lines 41-43). This is the documented
  relaxation drop-in for systemd `--user` mode where filesystem
  namespacing is unsupported. It is the inverse of a contradiction:
  the doc explicitly explains *why* it must clear those directives in
  user-mode (lines 20-22). Operators who run system-mode keep the
  strict defaults.
- `README.md` — describes `override-fuse.conf.example` as
  "Drop-in: relax `PrivateDevices=` and `SystemCallFilter=` so the
  daemon can perform FUSE mounts via `/dev/fuse`" (line 17), and at
  lines 72/76 explicitly flags this as a **relaxation** that the
  operator must opt into and that violates the strict default. This
  is honest documentation, not a contradiction.
- The base `pcloudd.service` retains every iter-3-verified hardening
  directive: `NoNewPrivileges=yes` (line 112), `ProtectSystem=strict`
  (line 55), `ProtectHome=tmpfs` (line 56), `PrivateTmp=yes` (line
  57), `CapabilityBoundingSet=` empty (line 113).

No contradiction.

---

## Check 3 — no new credential-shaped strings in iter-3-edited files

Re-grepped `STATUS.md`, `CLAUDE.md`, `packaging/systemd/*.{conf,md}`,
`deny.toml`, `C_FEATURE_PARITY_MATRIX.csv`,
`crates/pcloud-resilience/src/transport.rs`,
`crates/pcloud-proto/src/methods/shares.rs`, and
`crates/pcloud-proto/src/shares_api.rs` for the iter-1 secret-shape
pattern
`(password|token|secret|priv_key|passphrase|api_key|cookie)\s*[:=]\s*["'][^"']+["']`.

- `STATUS.md`, `CLAUDE.md`, all `packaging/systemd/*`, `deny.toml`,
  `C_FEATURE_PARITY_MATRIX.csv`, `pcloud-resilience/src/transport.rs`,
  `pcloud-proto/src/shares_api.rs`: **zero hits.**
- `pcloud-proto/src/methods/shares.rs`: 4 hits at lines 409, 436,
  455, 475 — all `auth_token: "tok".into()` literals inside `#[test]`
  / `#[cfg(test)]` fixture builders. `git log` confirms this file's
  most recent edit is `1c0c1d1` (xplat let-and-cond refactor) and
  before that `e9dae43` / `9956a79` — none of which are iter-3 fix
  commits. The `"tok"` test literals predate iter-3 (they exist in
  the iter-1 baseline) and were not introduced by the iter-3 campaign.
  They are intentional test-only mocks of opaque strings, not real
  credentials and not a regression.

No newly-introduced credential string in any iter-3-edited file.

---

## No new findings, no retractions, no regressions

Three regression-focused checks, three passes. Iter-4 has no new
security-class delta to report. Dim 2 stays converged for the third
consecutive iteration.

delta count: 0 new, 0 retractions, 0 regressions
