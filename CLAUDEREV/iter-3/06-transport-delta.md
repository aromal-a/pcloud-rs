# Dim 6 — Transport (HTTP API) & Network Resilience — iter-3 delta

**Status:** converged (0 new, 0 retractions, 0 regressions)

## Verification

- **TRANSPORT-H-1** (binary-API hot path bypasses `ResilientTransport`):
  re-checked. `crates/pcloud-resilience/src/transport.rs` defines the
  resilient client; production binary call sites in `pcloud-proto` /
  `pcloud-backends` continue to use the lower-level transport directly.
  No fix landed (consistent with `CLAUDEREV/iter-2-fixes.md` —
  TRANSPORT-H-1 explicitly deferred). **Still open. No change.**

- **Sampled 3 reqwest::Client sites in production code**, all are in
  out-of-scope crates per iter-2 §5 (`pcloud-fleet`, `pcloud-kms`,
  `pcloud-idp`):
  1. `crates/pcloud-kms/src/lib.rs:576-579` — `Client::builder()
     .use_rustls_tls().build()` — **no timeout, no connect_timeout**.
     Out-of-scope for dim 6 (Vault sidecar client, not pCloud API
     egress). Note for dim coverage if KMS becomes in-scope later.
  2. `crates/pcloud-idp/src/oidc.rs:76-80` — `connect_timeout(30s)`,
     `timeout(30s)`, `https_only(true)`. **Hardened.** No issue.
  3. `crates/pcloud-fleet/src/lib.rs:522-528` — preconfigured TLS,
     `tls_built_in_root_certs(false)`, `https_only(true)`, optional
     configurable `request_timeout`. **Hardened.** No issue.
  4. (extra) `crates/pcloud-idp/src/exchange.rs:159-162` —
     `connect_timeout(30s)`, `timeout(30s)`, conditional `https_only`
     gated on URL scheme. Acceptable.

  No production reqwest client on the **in-scope pCloud API egress
  path** uses unsafe defaults. Cookie-store hardening N/A — no cookie
  store is enabled on any sampled client (reqwest default is off).

- **TLS revocation bead (`pcloud-rs-t9o`):** tracker now reports
  `closed` (was open in iter-2). `bd list` output:
  `pcloud-rs-t9o [P3] [task] closed - P3: TLS CRL/OCSP stapling for
  FedRAMP-style dynamic revocation`. This is a tracker-status
  transition for an item that iter-1/iter-2 explicitly classified as
  LOW / tracked-elsewhere; it was never an open dim-6 finding in
  CLAUDEREV. **Not a new finding, not a regression** — just an
  upstream housekeeping change. The original LOW classification in
  `06-transport.md` is unaffected because that file's "tracked under
  pcloud-rs-t9o" annotation referenced where the work lived, not its
  open/closed status.

## Diff against iter-2 inventory

| Item                                  | iter-2     | iter-3     | Delta |
|---------------------------------------|------------|------------|-------|
| TRANSPORT-H-1 (HIGH)                  | open       | open       | none  |
| TRANSPORT-M-1/M-2/M-3 (MED)           | open       | open       | none  |
| TRANSPORT-L-1/L-2/L-3 (LOW)           | open       | open       | none  |
| TLS revocation bead (pcloud-rs-t9o)   | live-open  | live-closed| info  |
| Production reqwest defaults (in-scope)| safe       | safe       | none  |

## Conclusion

Dim 6 remains converged. No code changes touched the in-scope
transport surface between iter-2 and iter-3. The `pcloud-rs-t9o`
closure is informational tracker hygiene, not a finding shift.

**delta count: 0 new, 0 retractions, 0 regressions**
