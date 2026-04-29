# Stream F Report — Documentation Backfill (§12 HIGH)

**Date:** 2026-04-26
**Owner:** Stream F (docs-only)
**Scope:** Audit fragment `01-12-parity-and-docs.md` § 12 (Documentation Quality)
HIGH-severity findings on missing troubleshooting guide, missing
end-to-end deployment guide, and missing user-facing security
operations doc.

## Files created

| Path | Purpose |
|------|---------|
| `docs/book/src/operations/troubleshooting.md` | Cross-cutting failure-mode catalogue: FUSE mount errors (`ENOTCONN`, missing `fusermount3`, `allow_other`, stale-mount, `/dev/fuse` perms), vault corruption / locked, sync queue stuck, TLS pinning errors, TFA failures + error codes, crypto unlock distinguishing `WrongPassword` vs `BackendMismatch` (`PclsyncCompat` vs `Enhanced`), socket / vault / mount permission errors, and a diagnostics-capture appendix. |
| `docs/book/src/operations/deployment-guide.md` | Single-host end-to-end install per platform (Linux Tier-1 with FUSE override drop-in, macOS Tier-2 with launchd + notarisation, Windows Tier-2 with honest `pcloudd-svc` stub status, FreeBSD Tier-3) plus full systemd hardening matrix table cross-referencing every directive in `packaging/systemd/pcloudd.service`, log rotation (journald + logrotate snippet with `SIGUSR1` postrotate hook), SELinux module install, backup/restore procedure, in-place upgrade with SQLite migration check, and vault-format version handshake. |
| `docs/book/src/operations/security-operations.md` | User-facing distillation of `CLAUDE.md` § Security Rules: secret discipline table (which secrets are persisted vs in-memory only), vault location/posture per platform, IPC peer-cred enforcement (Unix `SO_PEERCRED`, Windows DACL — with honest Tier-2 callout that the accept loop is in flight), TLS-only production policy, audit-log inspection + tamper detection (`pcloud-cli audit verify`), hardening checklist, ADR cross-references. |

## Files modified

| Path | Change |
|------|--------|
| `docs/book/src/SUMMARY.md` | Wired the three new chapters under the **Operations** section so they appear in the rendered mdBook. |
| `README.md` | Added an "Operator-facing guides" link block immediately after the CLI reference pointer. No other content changed. |

## Honesty audit (self-check)

Grepped all three new docs for forbidden claim phrases (`production
ready`, `full parity`, `drop-in replacement`, `enterprise ready`).
Only hit was a deliberate negation in
`security-operations.md:255` ("this is **not** a 'production-ready'
claim"). The hardening checklist explicitly avoids the term. Each
chapter carries a top-of-page "Honesty callout" that links to
`STATUS.md` and the parity status chapter.

Tier ladder is honest throughout:

- Linux Tier-1, live-tested.
- macOS Tier-2, hardware verification pending.
- Windows Tier-2, named-pipe accept loop in flight, `pcloudd-svc`
  documented as a no-op stub for now.
- FreeBSD Tier-3, community best-effort, `continue-on-error: true`
  CI noted.

## Cross-references inside the new docs

Each chapter links inward (to `platforms/`, `security/`, `adr/`,
`reference/`) and outward (to `STATUS.md`, `CLAUDE.md`, the
`packaging/systemd/` README, ADR 0004 / 0005 / 0007 / 0015 / 0016).
None of the links target files that do not exist in the tree
(verified by spot-checking each ADR and platform path).

## Out of scope (not done)

- mdBook build verification (`mdbook build`) was **not** run; the
  audit fragment recommended adding this to CI but Stream F's scope
  is content authoring, not CI gating.
- SDK rustdoc inline examples (audit fragment § 12 HIGH item 3) —
  out of scope; that finding mutates `crates/pcloud-sdk/src/lib.rs`
  source code, which Stream F is forbidden from touching.
- README "Contributing & Reporting" section (audit fragment § 12 LOW
  item 1) — not requested in this stream's scope.

## Lines authored

- `troubleshooting.md`: ~420 lines.
- `deployment-guide.md`: ~520 lines.
- `security-operations.md`: ~280 lines.
- Total: ~1220 lines of new operator-facing prose, fully cross-linked.
