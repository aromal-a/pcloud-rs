# External Security Audit — Pre-Engagement Dossier

**Status:** Pre-engagement bundle. Hand this document (and the artifacts it references) to a prospective third-party auditor so they can scope, price, and plan an external security review of the `pcloud-rs` Rust rewrite. Every section is intended to answer a question the auditor would otherwise have to ask in a discovery call.

**Point of contact:** Security engineering lead (see `SECURITY.md`).
**Repository:** `pcloud-rs` (monorepo). Audit target lives in ``.
**Target revision:** the git commit referenced in the engagement SOW. Auditors should pin to a single commit hash for reproducibility.

---

## 1. Project Overview and Scope

`pcloud-rs` is a cross-platform pCloud client originally written in C/C++. The `` subtree is a ground-up Rust rewrite that now provides the retained feature set (auth, sync-root management, transfers, public links, shares, crypto, backup/device, and an embeddable SDK). The rewrite is substantially complete, but the team has not yet claimed "full parity" or "production ready" — that gate is tracked internally as `bd-1du.10`.

### In scope for the audit

- Everything under `crates/` — **26 workspace members**, approximately **80,000 lines of Rust code** (count via `tokei crates`).
- The daemon (`pcloud-daemon`), CLI (`pcloud-cli`), SDK (`pcloud-sdk`), protocol client (`pcloud-proto`), crypto (`pcloud-crypto`), secret wrappers (`pcloud-secret`), local IPC, auth vault, and sync/transfer runtimes.
- Build and release surfaces that ship to end users: `Cargo.toml`, `deny.toml`, `release.toml`, CI workflows under `.github/workflows/`.
- Documentation that makes security claims (this mdBook, `SECURITY.md`, `security/*.md`).

### Out of scope

- `packaging/` — distribution shims (deb/rpm/brew/msi recipes) are not Rust and carry no secrets.
- `fuzz/` — the fuzz harness itself is a test artifact, not a shipped surface. Auditors may of course *use* it.
- Legacy C code at the repo root (`pclsync/`, `main.cpp`, `control_tools.cpp`, `pclsync_lib.cpp`). It is retained only for parity comparison and is being retired.
- Third-party crates beyond the configured `cargo-deny` ruleset. We do expect supply-chain review (see §7) but not a line-by-line audit of upstream dependencies.

### Build and run

```bash
cd .
cargo check --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo deny  check
```

Minimum Rust toolchain: stable `1.78` (pinned in `rust-toolchain.toml`). No nightly features on the release path.

---

## 2. Architecture Summary

Full narrative: [`architecture/overview.md`](../architecture/overview.md). Condensed for the auditor:

- **Daemon (`pcloud-daemon`)** is the long-running process. It owns state, auth vault, transfer queue, sync engine, IPC listener, and the plugin registry. Crates of interest: `pcloud-daemon/src/bootstrap.rs`, `runtime.rs`, `auth_backend.rs`, `auth_vault.rs`, `sync_backend.rs`, `transfer_backend.rs`, `public_link_backend.rs`, `shares_backend.rs`, `backup_backend.rs`, `account_backend.rs`.
- **CLI (`pcloud-cli`)** is a thin client. It does not talk to pCloud directly; it speaks the local IPC protocol to the daemon.
- **SDK (`pcloud-sdk`)** is the embeddable Rust API. It can either drive a daemon over IPC or run a process-local engine for library consumers.
- **Protocol client (`pcloud-proto`)** wraps the pCloud HTTPS API with typed request/response structs. Split by feature family (`auth_api.rs`, `transfer_api.rs`, `public_links_api.rs`, `shares_api.rs`, `backup_api.rs`, `account_api.rs`).
- **Engine (`pcloud-engine`)** executes sync work: scans local trees, reconciles against the server, and schedules uploads/downloads.
- **Filesystem (`pcloud-fs`)** is the mounted-drive layer. Mount scaffolding, policy validation, RAII mount handles, in-memory read path, staging/journal/writeback helpers are present. The FUSE wiring is the last open P0 (see §6).
- **Crypto (`pcloud-crypto`)** implements Crypto Folder: setup, lock/unlock, sector encryption, key derivation, metadata filename encoding, password rotation, fingerprint verification, and the crypto-aware share temppass flow.
- **Secrets (`pcloud-secret`)** provides `SecretString` and `SecretBytes` with zeroize-on-drop and redacted `Debug`.
- **Store (`pcloud-store`)** is SQLite-backed persistence (sync roots, queued work, audit).

### Trust boundaries

1. **Process boundary:** daemon vs. CLI vs. SDK consumer — crossed by Unix-domain IPC (Linux/macOS/BSD) or a named pipe (Windows). Owner-only permissions are enforced; peer UID/PID checks happen on accept.
2. **Local filesystem:** auth vault (`0600`), runtime dir (`0700`), sync roots (user-owned), mount point (FUSE-managed).
3. **Network boundary:** HTTPS to pCloud API endpoints. TLS is mandatory in production config; downgrade is rejected.
4. **Crypto boundary:** Crypto Folder keys are derived client-side and never leave the process in cleartext.

---

## 3. Threat Model Summary

Full model: [`security/threat-model.md`](./threat-model.md). Attacker classes we design against:

- **Local unprivileged user on the same host.** Must not read the auth vault, must not impersonate the daemon over IPC, must not recover secrets from memory via core dumps or swap.
- **Local malicious process running as the same UID.** Acknowledged partial threat — same-UID is inside the trust boundary by Unix convention. We still reduce blast radius via owner-only IPC, redacted logging, and zeroize-on-drop.
- **Network attacker, passive or active, on the path to the pCloud API.** TLS is mandatory; no plaintext downgrade; no unvalidated endpoint override on the release path.
- **Malicious pCloud API response.** All responses flow through typed deserializers with bounded sizes. No `serde_json::Value` free-form parsing on the hot path.
- **Supply-chain attacker against a build dependency.** Addressed via `cargo-deny`, pinned lockfile, SBOM emission, and reproducible-build documentation (§7, `development/reproducible-builds.md`).
- **Lost or stolen device.** Auth token persistence is opt-in; Crypto Folder password is not persisted; disk encryption is the user's responsibility but we do not weaken it.

Explicitly **not** in the threat model: kernel-level attackers, attackers with root, attackers who control the upstream pCloud service, and attackers with physical access during an active session.

---

## 4. Unsafe Inventory

The workspace currently contains **54 `unsafe` blocks**. Every single one carries a `// SAFETY:` comment explaining the invariant the author is asserting. The canonical inventory is the wave-02 review artifact `REVIEW_FULL_02.md` (section "Unsafe audit — R2 inventory"), which enumerates each block by `file:line`, the surrounding function, the justification class (FFI, lifetime extension, transmute, raw pointer), and whether the SAFETY comment was judged sufficient.

Auditors should:

1. Regenerate the inventory from the pinned commit:
   ```bash
   rg -n --no-heading 'unsafe\s*\{' crates
   rg -n --no-heading '//\s*SAFETY:'  crates
   ```
2. Diff against `REVIEW_FULL_02.md` to confirm the 54-block count and confirm no unannotated blocks slipped in.
3. Pay particular attention to FUSE FFI in `pcloud-fs/` (the largest concentration) and the zeroize paths in `pcloud-secret/`.

House rule: any new `unsafe` block requires an ADR. See `adr/` and `development/contributing.md`.

---

## 5. Cryptographic Primitives and Parameters

Full discussion: [`security/secrets.md`](./secrets.md). Summary for the auditor:

| Purpose | Primitive | Parameters |
|---|---|---|
| Sector encryption (Crypto Folder) | **AES-256-GCM** | 256-bit key, **12-byte nonce**, 16-byte tag, per-sector unique nonce |
| Password-based key derivation | **Argon2id** | `m = 19456 KiB`, `t = 2`, `p = 1`, 32-byte salt, 32-byte output |
| Message / vault integrity | **HMAC-SHA256** | 32-byte key, 32-byte tag |
| Constant-time comparison | **`subtle::ConstantTimeEq`** | used for tag verification, password equality, vault MAC check |
| RNG | **`rand::rngs::OsRng`** | OS CSPRNG; never a seeded PRNG on the release path |
| TLS | **`rustls`** | modern cipher suites, no downgrade, no plaintext |

All primitives come from well-reviewed crates (`aes-gcm`, `argon2`, `hmac`, `sha2`, `subtle`, `rustls`). No hand-rolled crypto. No raw `openssl` usage.

Key handling invariants:

- Secrets are wrapped in `SecretString` / `SecretBytes` (`pcloud-secret/src/*.rs`), which `zeroize` on drop and redact `Debug`.
- Auth tokens are persisted only when the user opts in; the vault file is `0600` and the parent dir is `0700`; ownership and mode are re-validated on every read.
- Crypto Folder passwords are **never** persisted. This is a deliberate divergence from the C client (documented in ADR-0007).

Auditor asks for this section: verify nonce uniqueness per key, confirm constant-time usage on every secret-comparison path, confirm no `Display`/`Debug`/`serde::Serialize` implementation leaks a secret, and confirm Argon2 parameters are tuned for the target deployment profile.

---

## 6. Known-Gap List (Honest)

We will not pretend these are closed. An auditor should see them before quoting.

- **`bd-1du.4.6` — FUSE daemon wiring.** The `pcloud-fs` crate has mount scaffolding, policy validation, RAII handles, signal-aware unmount, in-memory read path, staging, journal, and writeback helpers. It is **not** yet wired into the daemon runtime as a live mounted-drive. Real readdir/open/read/write/flush/fsync against a mounted point is pending.
- **`bd-1du.10` — Final parity gate.** The parity matrix
  (`C_FEATURE_PARITY_MATRIX.csv`) is tracked in
  [`STATUS.md`](../../../../STATUS.md) — the single source of truth
  for counts. All retained rows are `Implemented`; the remaining gate
  items are Reviewer-19 regrade and the closing-commit SHA. Until
  `bd-1du.10` closes, marketing wording ("full parity", "production
  ready", "drop-in replacement") remains banned.
- **Platform coverage.** Live-host verification has been performed on Linux. macOS and Windows have been exercised in CI (unit + integration) but have **not** had a real end-to-end live-host run against a production pCloud account. Auditors on those platforms should treat them as "CI-green, not live-proven".
- **Residual Partials in transfers/SDK.** A handful of SDK breadth rows (streaming edge cases, resumable-upload edge conditions) are reconciled under `bd-1du.10`.
- **Missing C surfaces we will not ship.** `change_crypto_pass` family, `send_change_user_private`, `priv_key_flags`, `psync_send_publink`, and the update-check declarations are either pending or deliberately rejected. See `REJECTED-RATIONALES-14042026.md`.

---

## 7. SBOM Hand-off

- **CI job:** `.github/workflows/` → the `sbom` job runs on every tagged release.
- **Tool:** `cargo-cyclonedx` (CycloneDX 1.5 JSON + XML).
- **Artifact:** attached to each GitHub Release as `pcloud-rs-<version>-sbom.cdx.json` and `.cdx.xml`.
- **License scan:** `cargo-deny check licenses` is part of the same workflow; policy lives in `deny.toml`.
- **Reproducibility:** `development/reproducible-builds.md` documents the exact toolchain, flags, and `SOURCE_DATE_EPOCH` handling so an auditor can reproduce a release artifact bit-for-bit.

Auditors should regenerate the SBOM at the pinned commit and diff against the published artifact.

---

## 8. Fuzz Harness Inventory and Coverage

Location: `fuzz/fuzz_targets/`. Harnesses cover:

- `fuzz_ipc_frame` — local IPC framing and length-prefix parser (`pcloud-daemon` IPC decoder).
- `fuzz_proto_response` — pCloud API response deserializer (`pcloud-proto`).
- `fuzz_crypto_metadata` — metadata filename decoder and sector-layout parser (`pcloud-crypto`).
- `fuzz_vault_decode` — auth vault file format (`pcloud-daemon/auth_vault`).
- `fuzz_public_link_url` — public-link URL and token parser (`pcloud-proto/public_links_api`).
- `fuzz_engine_journal` — sync engine journal record decoder (`pcloud-engine`).

Run with `cargo fuzz run <target> -- -max_total_time=3600`. Coverage artifacts (`coverage/fuzz/*.profdata`) are produced by the nightly coverage job and are available on request; they are not published as release artifacts because the corpus contains synthesized inputs only.

---

## 9. Property-Test Inventory

`proptest` suites live alongside the code they exercise:

- `pcloud-proto/tests/prop_roundtrip.rs` — request/response round-trip for every typed protocol struct.
- `pcloud-crypto/tests/prop_sector.rs` — encrypt/decrypt round-trip across random plaintext lengths, including zero-length and sector-boundary cases.
- `pcloud-crypto/tests/prop_filename.rs` — metadata filename encode/decode round-trip on arbitrary UTF-8 names.
- `pcloud-secret/tests/prop_zeroize.rs` — post-drop memory is zero for `SecretBytes` of varying length.
- `pcloud-store/tests/prop_sqlite.rs` — CRUD invariants over arbitrary sync-root insertions.
- `pcloud-engine/tests/prop_reconcile.rs` — reconciliation converges for arbitrary local/remote tree pairs.

Run with `cargo test --workspace`. Defaults to 256 cases per property; raise with `PROPTEST_CASES=8192` for an audit run.

---

## 10. Chaos-Test Inventory

Chaos suites live under `tests/chaos/` in the relevant crate:

- `pcloud-daemon/tests/chaos_ipc.rs` — slow client, malformed frame flood, half-open connection, peer-UID spoofing attempt.
- `pcloud-daemon/tests/chaos_vault.rs` — vault file corrupted, mode weakened, owner changed, parent dir weakened.
- `pcloud-engine/tests/chaos_sync.rs` — power-loss simulation (kill between journal write and rename), disk-full, read-only mount.
- `pcloud-proto/tests/chaos_network.rs` — TLS handshake abort, truncated response, oversized response, rate-limit storm.
- `pcloud-fs/tests/chaos_mount.rs` — unmount-under-load, signal-mid-write, journal replay after crash.

These run in the `cargo test --workspace` default set but are feature-gated with `--features chaos-slow` for the long-running variants.

---

## 11. Prior Self-Review Findings

Two internal review waves have already been performed by the engineering team. Auditors should read both before starting — they set the baseline.

- **Wave 1:** `REVIEW_FULL_01.md` — initial architectural and memory-safety sweep. Findings were triaged, fixed, and landed before wave 2.
- **Wave 2:** `REVIEW_FULL_02.md` — deeper sweep including the 54-block `unsafe` inventory (§4), secret-handling review, IPC hardening, and cargo-deny policy. Remaining open items are the ones listed in §6.

Both files are archived under the `archive/` section of this mdBook for traceability.

---

## 12. RFQ — Scope Asks

We are soliciting a fixed-scope engagement covering the following five workstreams. Bidders may propose bundling or splitting.

1. **Critical- and High-severity bug hunt** across the 26-crate workspace. Full-tree read, targeted review of the daemon, IPC, auth vault, and crypto. Deliverable: CVSS-scored finding list.
2. **Constant-time audit of the cryptographic paths.** Verify `subtle::ConstantTimeEq` is used wherever a secret comparison occurs; verify no early-return leaks; verify no branching on secret bytes in `pcloud-crypto` and `pcloud-secret`. Deliverable: per-path attestation or finding.
3. **Supply-chain attack simulation.** Attempt a typosquat / dependency-confusion / malicious-update scenario against the build. Evaluate `cargo-deny` policy, lockfile discipline, SBOM accuracy, and reproducible-build claims. Deliverable: simulation report and policy recommendations.
4. **IPC fuzz campaign.** Extend `fuzz_ipc_frame` with structure-aware inputs; run for a minimum of 72 CPU-hours; report any crash, hang, or auth-bypass. Deliverable: corpus, coverage delta, and findings.
5. **Formal proof of lock ordering in the circuit breaker and sync runtime.** Model the lock graph (likely `loom` or a lightweight TLA+ spec) and prove absence of deadlock under the documented workload. Deliverable: model, proof artifact, and any counter-examples.

Out-of-scope asks we will *not* fund: review of upstream pCloud service security, review of the legacy C client beyond diff-for-parity, and penetration testing of the pCloud production API.

---

## 13. Commercial Terms

- **Engagement length:** **4 to 6 weeks** of auditor time, wall-clock. We can extend by mutual agreement.
- **Budget envelope:** **USD 20,000 to 50,000** depending on workstream count (§12) and depth. Fixed-fee preferred; T&M considered with a cap.
- **Deliverable shape:**
  - CVSS v3.1 scored findings with reproduction steps, affected file:line, and suggested remediation.
  - Executive summary (one page) suitable for a release-notes attestation.
  - Remediation window: **30 days** for Critical, **60 days** for High, **90 days** for Medium, best-effort for Low.
  - Re-test of remediated findings included in the base fee (one round).
- **Disclosure:** coordinated. We will publish a public summary post-remediation; findings may be referenced by CVE where appropriate.
- **Legal:** mutual NDA on request; auditor indemnity and professional-liability evidence required before engagement start.

To proceed, please respond with: staffing plan, proposed schedule against the 4–6 week window, fixed fee per workstream, and any scope clarifications. Target kickoff within 30 days of acceptance.
