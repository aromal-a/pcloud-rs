# Security Audit — Iteration 3 Delta

Date: 2026-04-29 (iter-3 delta)
Scope: dim 2 was declared converged in iter-2 (`CLAUDEREV/iter-2/02-security-delta.md`,
"delta count: 0 new findings, 0 retractions"). Iter-3 verifies the dimension
**stays converged** after the iter-2 fix campaign
(`CLAUDEREV/iter-2-fixes.md`).

## Convergence: YES (still)

Iter-3 walked three narrow security-only checks:

1. **Did the iter-2 fix campaign introduce any new credential-bearing
   `String` field, log a secret, or weaken a secret-handling boundary?**
2. **Did the iter-2 systemd-unit edit (DEPLOY-H-11.3 fix) loosen any
   other hardening directive?**
3. **Are the 4 deferred SEC-H-1..H-4 migration sites still in their
   original (unchanged) state?**

All three checks pass.

---

## Check 1 — iter-2-fix files do not introduce credential-bearing fields

Files modified in `iter-2-fixes.md`:

- `STATUS.md`, `API-REFERENCE.md`, `CHANGELOG.md`, `README.md`, `CLAUDE.md`,
  `SECURITY.md` (workspace docs)
- `docs/book/src/getting-started/install.md`,
  `docs/book/src/SUMMARY.md`,
  `docs/book/src/adr/index.md`, `docs/book/src/adr/0011..0018.md`
  (mdBook docs and ADR stubs)
- `packaging/systemd/pcloudd.service` (DEPLOY-H-11.3 fix)
- 3 rustdoc-only edits in `pcloud-proto/src/methods/shares.rs`,
  `pcloud-proto/src/shares_api.rs`, `pcloud-config/src/sync_loop.rs`
  (intra-doc-link → plain code-span)

Re-grepped these files for the iter-1/iter-2 secret-shape pattern
`(password|token|secret|priv_key|passphrase|api_key|cookie)\s*[:=]\s*["'][^"']+["']`.
**Zero hits.** No credential-shaped string was introduced. The doc
files reference token *names* (e.g. `auth_token`, `web_token`) only
as code-span identifiers in narrative prose, never as values. The
3 rustdoc edits convert intra-doc links to code spans — no behavioral
or type change.

---

## Check 2 — systemd unit edit does not weaken other hardening

Verified the iter-2 fix in `packaging/systemd/pcloudd.service` removed
**only** the `IPAddressDeny=any` + `IPAddressAllow=localhost` block (now
documented as a moved-to-`override.conf.example` opt-in at lines
119-128). Every other hardening directive iter-1 enumerated as present
is still present and unweakened:

- `NoNewPrivileges=yes` (line 112) — present
- `ProtectSystem=strict` (line 55) — present
- `ProtectHome=tmpfs` (line 56) — present
- `PrivateTmp=yes` (line 57) — present
- `PrivateDevices=yes` (line 58) — present
- `PrivateUsers=yes` (line 115) — present
- `CapabilityBoundingSet=` (empty) (line 113) — present
- `AmbientCapabilities=` (empty) (line 114) — present
- `ProtectKernelTunables/Modules/Logs/ControlGroups/Clock/Hostname/Proc=`
  — all present (lines 59-65)
- `LockPersonality=yes`, `RestrictSUIDSGID=yes`, `RemoveIPC=yes` —
  all present (lines 67-69)
- `UMask=0077` — present
- `RuntimeDirectoryMode=0700`, `StateDirectoryMode=0700`,
  `LogsDirectoryMode=0700` — all present
- `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6` (line 118) —
  present (egress restriction is now via host firewall, not the unit;
  the AF allow-list is still strict — no `AF_NETLINK`, `AF_PACKET`,
  `AF_BLUETOOTH`, etc.)
- `SystemCallArchitectures=native` + `SystemCallFilter=@system-service`
  with `~@privileged @resources @obsolete @mount @debug @cpu-emulation
  @raw-io @reboot @swap` deny-list (lines 131-133) — present
- `KeyringMode=private`, `RestrictNamespaces=yes`,
  `RestrictRealtime=yes` (lines 146-148) — present
- `LimitCORE=0` (line 143) — present (no core dumps)

The DEPLOY-H-11.3 fix is **net-neutral** on hardening: the removed
directives were silently breaking egress, not adding security against
a real threat (a daemon already running with `CapabilityBoundingSet=`
empty, `PrivateUsers=yes`, and `NoNewPrivileges=yes` cannot bind
arbitrary ports anyway). Operators wanting strict egress filtering
opt in via the documented drop-in.

---

## Check 3 — deferred SEC-H-1..H-4 sites unchanged

Verified each deferred site is still in the exact state iter-1
reported:

- **SEC-H-1**: `crates/pcloud-proto/src/auth_api.rs:114`
  `auth_token: String` — unchanged
- **SEC-H-1**: `crates/pcloud-proto/src/auth_api.rs:123`
  `challenge_token: String` — unchanged
- **SEC-H-2**: `crates/pcloud-proto/src/account_api.rs:100`
  `pub auth_token: String` — unchanged
- **SEC-H-2**: `crates/pcloud-ipc/src/methods.rs:1129`
  `verify_token: String` — unchanged
- **SEC-H-3**: `crates/pcloud-web/src/lib.rs:209`
  `pub web_token: String` — unchanged (note: `AppState::web_token`
  in `lib.rs:280` is `Arc<SecretString>`, so the request-handler
  path is already correct; `WebConfig.web_token` remains the public
  surface to migrate)
- **SEC-H-4**: `crates/pcloud-config/src/api.rs:74-77`
  `impl Default for TlsRevocationCheck { ... TlsRevocationCheck::Disabled }`
  — unchanged

All four are explicitly listed as deferred in `iter-2-fixes.md`
(`SEC-H-1..H-3`: "touches IPC wire shape … needs careful migration.
Defer."; `SEC-H-4`: "already tracked under `pcloud-rs-t9o`. Bead-tracked,
defer."). No partial migration was attempted; no test was loosened to
accommodate one. The deferral is clean.

---

## No new findings, no retractions, no regressions

Three checks, three passes. Iter-3 has no new security-class delta to
report. Dim 2 stays converged.

delta count: 0 new findings, 0 retractions, 0 regressions
