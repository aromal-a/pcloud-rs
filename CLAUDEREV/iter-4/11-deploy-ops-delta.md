# iter-4 delta — Section 11 Deployment & Operations

Scope: re-read `packaging/systemd/override.conf.example` and
`packaging/systemd/README.md` after the iter-3 fix-campaign rewrites,
verify internal coherence and systemd merge semantics, spot-check the
rest of `packaging/` for stale citations of the old default unit shape.

## Verification of iter-3 fix campaign

### override.conf.example — OPT-IN strict egress allow-list

- Header (lines 1-30) correctly documents that the shipped
  `pcloudd.service` no longer sets `IPAddressDeny`/`IPAddressAllow`,
  that this drop-in is opt-in defence-in-depth, install instructions
  for both system + user units, and how to discover API IP ranges.
- Body (lines 32-44) installs `IPAddressDeny=any` + `IPAddressAllow=localhost`
  itself (no longer "resets" something the shipped unit would carry).
  Per-cluster CIDR allow lines (eapi/api/binapi) are commented out;
  a final uncommented `IPAddressAllow=any` provides the broad-allow
  fallback path.
- Systemd merge semantics check: when this drop-in is installed at
  `/etc/systemd/system/pcloudd.service.d/api-access.conf`, systemd
  merges `[Service]` directives with the base unit. `IPAddressDeny=any`
  + `IPAddressAllow=localhost` + `IPAddressAllow=any` is a valid
  effective unit (subsequent `IPAddressAllow=` lines are additive in
  systemd, not last-write-wins, so the final allow-list is the union;
  if the operator uncomments per-cluster CIDRs and removes the broad
  `=any`, the effective egress is restricted to localhost + those
  CIDRs). State is internally consistent.

### README.md — drop-in installation guide

- Files table (line 16) correctly labels override.conf.example as
  OPT-IN and documents the post-iter-2 shipped-unit shape.
- "When to install each drop-in" matrix (lines 26-30) correctly
  recommends "No" for egress drop-in across all three deployment
  modes, citing host firewall as the gate.
- Trade-off section (lines 63-70) explicitly calls out the OPT-IN
  posture and the operational burden of CIDR maintenance.
- Audit trail dates and the iter-2/iter-3 reference (lines 22-24)
  are accurate.

### Spot check across packaging/

- `packaging/systemd/pcloudd.service` lines 119-126 carry an explicit
  comment block stating `IPAddressDeny`/`IPAddressAllow` are
  intentionally not set in the default unit, with a forward-pointer
  to `override.conf.example`. Coherent with both reviewed files.
- `grep -rn IPAddress packaging/` returns 0 stale citations of the
  old `IPAddressDeny=any` default; every remaining hit is either in
  the OPT-IN drop-in body, in the README's discussion of that
  drop-in, or in the documented-rejection comment block in the unit.
- No other file under `packaging/` references the old default unit
  shape.

## NEW FINDING

### DEPLOY-DOC-CONTRADICTION-11.3b (LOW)

`packaging/systemd/README.md` lines 4-6 still describe the shipped
`pcloudd.service` as "intentionally strict... denies all outbound
network traffic except to localhost". This is the pre-iter-2 default
unit shape. The same README at lines 22-24 and the unit itself at
lines 119-126 correctly document that `IPAddressDeny=any` is no longer
shipped by default. The intro paragraph at lines 4-6 was missed by
the iter-3 fix-campaign rewrite and now contradicts both the
"When to install each drop-in" matrix below it and the actual unit
content.

Fix: replace "denies all outbound network traffic except to localhost"
with wording that lists the sandboxing properties the shipped unit
*does* enforce (e.g. "isolates `/dev`, runs under `DynamicUser=`,
applies `ProtectSystem=strict`, and filters out privileged syscall
groups; outbound network traffic is gated by the host firewall, not
by the unit"). The "denies all outbound" phrase must come out.

Severity: LOW — operator-visible documentation drift only; no
runtime security degradation. The drop-in itself, the unit, and the
deployment-mode matrix are all correct; only the README intro
paragraph is stale.

File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/packaging/systemd/README.md:4-6`

## Carry-forward

- DEPLOY-H-11.1 still open (deferred).
- DEPLOY-H-11.2 still open (deferred).
- DEPLOY-H-11.4 still open (deferred).
- DEPLOY-DOC-REGRESSION-11.3a from iter-3 — RESOLVED by the rewrite
  of `override.conf.example` and `README.md`. The drop-in body now
  installs the deny-gate itself; the README accurately describes the
  OPT-IN posture except for the lines-4-6 intro paragraph regression
  captured above.

## Result

delta count: 1 new, 1 retractions, 0 regressions
