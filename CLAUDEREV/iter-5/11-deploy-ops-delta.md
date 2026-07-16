# Iter-5 delta — Section 11: Deployment & Operations

**Date:** 2026-04-29
**Scope:** Re-verification after iter-4 fix of DEPLOY-DOC-CONTRADICTION-11.3b
in `packaging/systemd/README.md` L1-12.

## Delta task results

### 1. Coherence of new intro paragraph (L1-12)

Re-read `packaging/systemd/README.md` L1-12 against:

- the matrix at L29-33 ("When to install each drop-in"),
- the trade-offs at L66-73 ("Security trade-offs"),
- and the shipped unit at `packaging/systemd/pcloudd.service` L119-126.

**Verdict: internally coherent.**

| Claim in intro (L4-11)                                                | Cross-check                                                                                                                                                  | Match? |
|-----------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------|--------|
| "shipped `pcloudd.service` is sandbox-strict"                         | unit ships `DynamicUser=`, `ProtectSystem=strict`, `PrivateDevices=yes`, `SystemCallFilter=` exclusions — confirmed in `pcloudd.service`                     | yes    |
| "Outbound network traffic is gated by the host firewall, not the unit"| L25-27 restates the same; L66-73 trade-offs frame `override.conf` as OPT-IN cgroup-level addition; unit comment L119-126 explicitly drops `IPAddressDeny=`   | yes    |
| "deliberate change as of 2026-04-30"                                  | same date is cited at L25-27, L69, and unit comment L119-126                                                                                                 | yes    |
| "FUSE deployments still need `override-fuse.conf.example`"            | matrix row "Mounted pCloud filesystem" at L33 says "Yes" for FUSE drop-in                                                                                    | yes    |
| "egress allow-listing via `override.conf.example` is OPT-IN"          | matrix L31-33 marks egress drop-in "No" across all modes (defence-in-depth note); trade-offs L66-73 frame it as opt-in                                       | yes    |

No contradictions found. The intro paragraph is consistent with the rest of
the file and with the unit file itself.

### 2. Final coherence sweep across `packaging/`

Grepped `packaging/` for `IPAddressDeny`, `denies all outbound`, and
`outbound network`.

| File                                          | Mention                                                                              | Coherent? |
|-----------------------------------------------|--------------------------------------------------------------------------------------|-----------|
| `packaging/README.md` L98                     | `PCLOUD_PLUGIN_ALLOW_NETWORK` env var description — unrelated to systemd policy      | yes       |
| `packaging/systemd/README.md` L19, L25-27, L66| All consistently describe `override.conf` as OPT-IN, unit no longer sets the default | yes       |
| `packaging/systemd/override.conf.example` L4, L35 | File is the opt-in drop-in; sets `IPAddressDeny=any` only when installed         | yes       |
| `packaging/systemd/pcloudd.service` L119-126  | Comment block explicitly documents the iter-2 deletion of `IPAddressDeny=` and refers operators to the opt-in drop-in | yes |

No drift between docs and shipped artifacts. No stale "denies all outbound"
language remains anywhere under `packaging/`.

### 3. Open prior findings still deferred

- **DEPLOY-H-11.1** — open (deferred)
- **DEPLOY-H-11.2** — open (deferred)
- **DEPLOY-H-11.4** — open (deferred)
- **DEPLOY-DOC-CONTRADICTION-11.3b** — closed in iter-4; verified in iter-5

## Findings

| ID | Severity | Status |
|----|----------|--------|
| (none) | — | Section 11 is coherent and converged on the intro/matrix/trade-off axis. |

**delta count: 0 new, 0 retractions, 0 regressions**
