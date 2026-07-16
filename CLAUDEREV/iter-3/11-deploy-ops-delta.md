# Dimension 11 — Deployment & Operations — Iter-3 Delta

**Re-audit date:** 2026-04-29
**Mode:** read-only.
**Iter-1 totals:** 0 CRITICAL · 4 HIGH · 7 MEDIUM · 6 LOW.
**Iter-2 delta:** 4 new findings (CLAUDE.md Windows posture stale; duplicate
BSD init artefacts; no OCI image pipeline; no Helm chart).
**Iter-2 fixes landed:** DEPLOY-H-11.3 — removed default `IPAddressDeny=any`
+ `IPAddressAllow=localhost` block from `packaging/systemd/pcloudd.service`.

---

## Regression check on the iter-2 DEPLOY-H-11.3 fix

### Unit functionality after the edit — PASS

`packaging/systemd/pcloudd.service` re-read at lines 117–129. The
`[Service]` section is internally consistent:

- `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6` (line 118) is
  intact and is the only network-family gate left in the unit. AF_INET
  and AF_INET6 are both permitted, so egress to the pCloud API is
  allowed at the syscall layer.
- `PrivateNetwork=` is **not** set anywhere in the unit. The host
  network stack is therefore visible to the daemon (correct — required
  to reach `*.pcloud.com`).
- `Restart=on-failure` (line 39), `RestartSec=5s`, `TimeoutStopSec=30s`
  are unchanged. They do not depend on the removed
  `IPAddressDeny`/`IPAddressAllow` directives in any way.
- No other directive in the unit references `IPAddressDeny` or
  `IPAddressAllow`. Removal is structurally clean.

The replacement comment block at lines 119–128 correctly explains the
prior breakage and points operators at `packaging/systemd/override.conf.example`.

**Verdict on the fix itself:** the unit is functional and the rationale
comment is accurate at the unit-file level. **No regression in the unit.**

### Companion docs are now stale — REGRESSION (1 new finding)

The fix did not propagate to the two companion files that document the
shipped unit's behaviour. Both still claim the unit ships with
`IPAddressDeny=any`, which is no longer true:

- **`packaging/systemd/override.conf.example:1`** —
  > "pcloudd.service drop-in override — broaden IPAddressAllow for pCloud API"
  Lines 3–5: "The shipped unit restricts outbound to localhost only, which
  blocks access to the pCloud API endpoints… Operators MUST install a
  drop-in to allow the necessary traffic." This is now false; the
  shipped unit has no `IPAddressDeny` at all and routes via the host
  firewall.
  Line 26: "The default shipped unit also has IPAddressDeny=any before
  the Allow list; this drop-in resets and replaces both stanzas." Stale.
  The drop-in's own body at lines 31 (`IPAddressDeny=`) and 32
  (`IPAddressAllow=localhost`) followed by line 40 (`IPAddressAllow=any`)
  is still functionally correct *if* installed, but the documentation
  framing now misleads operators into thinking they MUST install it to
  get egress (they don't).

- **`packaging/systemd/README.md:21`** —
  > "The shipped unit enforces `IPAddressDeny=any` with only `localhost`…"
  Line 63: "`override.conf.example` removes the `IPAddressDeny=any` +
  `IPAddressAllow=localhost`…" Both bullets describe a unit shape that
  no longer exists in this fork.

This is a **documentation regression** introduced by the iter-2 fix:
the unit is correct, but two operator-facing docs in the same directory
contradict it. An operator following the iter-2-vintage README will
install a drop-in they do not need, and may be confused that
`IPAddressDeny=` (an empty-reset directive) appears without a prior
`IPAddressDeny=any` to reset.

**DEPLOY-DOC-REGRESSION-11.3a (LOW)** —
`packaging/systemd/override.conf.example:1-27` and
`packaging/systemd/README.md:21,63` need to be reworded to match the
post-iter-2 unit (default = no IP filtering; drop-in is OPT-IN strict
allow-listing). Mechanical text-only fix; no behavioural change.

---

## Carry-forward status of open findings

| ID | Status | Notes |
|---|---|---|
| DEPLOY-H-11.1 (Windows MSI no-op) | OPEN, deferred | Iter-2 already noted CLAUDE.md is stale; serve path is wired. No new evidence. |
| DEPLOY-H-11.2 (.deb/.rpm not in CI) | OPEN, deferred | No new evidence; `.github/workflows/` not re-scanned this iter. |
| DEPLOY-H-11.3 (default `IPAddressDeny=any`) | **CLOSED** in unit, but spawned DEPLOY-DOC-REGRESSION-11.3a above. |
| DEPLOY-H-11.4 (FIPS not gated) | OPEN, deferred | Out of scope this iter. |
| Iter-2 #1 (CLAUDE.md Windows posture stale) | OPEN | Confirmed still stale by spot-check; CLAUDE.md still says `serve_with_shutdown` returns `Unsupported` on Windows. |
| Iter-2 #2 (duplicate BSD init artefacts) | OPEN | `packaging/{bsd,freebsd,netbsd,openbsd}` all still present. |
| Iter-2 #3 (no OCI image pipeline) | OPEN | `packaging/docker/` exists but no published-image CI hook re-checked. |
| Iter-2 #4 (no Helm chart) | OPEN | `packaging/` has no `helm/` or `charts/` directory. |

---

## Summary

The iter-2 unit edit is structurally sound — `pcloudd.service` is
functional, no internal directive depends on the removed block, and
`Restart` / `RestrictAddressFamilies` are intact. However the fix
forgot to update two companion files that describe the unit's
network-filtering posture, producing a **documentation regression**
(LOW). One new finding this iter; no retractions.

---

**delta count: 1 new (LOW), 0 retractions, 1 doc regression spawned by iter-2 fix**
