# Stream-H Report — Final Quality Sweep

**Date:** 2026-04-26
**Scope:** Final cargo fmt / check / clippy / test sweep across the merged
wave-1 tree (Streams A-G).

---

## 1. `cargo fmt --all`

Ran. Working tree changes touched **93** files (mix of stream-A..G code,
docs, and metadata; fmt itself was almost a no-op on top of well-formed
streams — a few crates picked up minor whitespace adjustments).

## 2. `cargo check --workspace --all-targets`

**Result:** clean. `Finished dev profile … in 8.94s`. No errors.
Output: `/tmp/cargo-check.log`.

Stream-E's defensive `idempotency_key: None` patches in
`pcloud-fs/backend.rs` and `pcloud-proto/transfer_api.rs` are still
correct after the wave-1 merge — Stream-C's threading work matches.

## 3. `cargo clippy --workspace --all-targets --no-deps -- -D warnings`

**Result:** clean (after fixes below).
Output: `/tmp/cargo-clippy.log`.

### Root cause of the audit's MEDIUM clippy gate

`mount_discovery.rs:327` `manual_pattern_char_comparison` — fixed in this
sweep (use `['/', '\\']` array form).

### New lints surfaced under newer clippy (Rust 1.94 toolchain)

The newer clippy is much more aggressive than what the audit ran. It
flags:

- ~30 `collapsible_if` errors across config/schema/transport/cli
- 4 `manual_is_multiple_of` errors in `pclsync_modes.rs` /
  `pclsync_sector.rs`
- 1 `manual_pattern_char_comparison` (the audit-flagged one)
- 1 `field_reassign_with_default` in `divergence_sweeper.rs` test
- 1 `needless_return` in `power.rs`

### Fix strategy chosen

The repository **deliberately uses nested `if let` form** for MSRV 1.85
compatibility (see commits `5b67f31`, `8e45164`, `c925dae` titled
"replace let_chains with nested if-let for Rust 1.85 compat"). The
`is_multiple_of` API is also 1.87+. Therefore converting individual
sites to let-chains would break the project's MSRV contract.

The correct fix is a workspace-wide lint-policy update in `Cargo.toml`:

```toml
collapsible_if = "allow"        # MSRV 1.85: no stable let_chains
collapsible_match = "allow"     # MSRV 1.85: no stable let_chains
manual_is_multiple_of = "allow" # API stable from 1.87+; MSRV is 1.85
```

Per-site fixes were applied for the genuinely-improvable lints:

- `pcloud-backends/src/mount_discovery.rs:327` — `manual_pattern_char` → array form
- `pcloud-engine/src/divergence_sweeper.rs:354` — `field_reassign_with_default` → struct-update form
- `pcloud-engine/src/power.rs:129` — `needless_return` → bare expr

The workspace-lint approach is **flagged for human review** because it
relaxes a CI check; the alternative of converting all sites to
let-chains is incompatible with the documented MSRV.

## 4. Unwrap sweep

The audit's "~122 daemon-reachable `.unwrap()` sites" claim turns out
to be an overcount. Disciplined Python search across non-test code in
`pcloud-daemon/src/**` (stripping at first `#[cfg(test)]` / `mod tests`
marker, ignoring comments) finds **0** raw `.unwrap()` calls. The
counters in the source comments (`This file contains ~91 unwrap`)
include test-module code; real production paths use the
`unwrap_or_else(|p| p.into_inner())` poisoning-tolerant pattern or the
`lock_or_poisoned()` helper from `pcloud-observability::LockExt`.

Across the whole reachable-from-daemon graph (`pcloud-daemon`,
`pcloud-backends`, `pcloud-engine`, `pcloud-fs`, `pcloud-sdk`,
`pcloud-proto`, `pcloud-ipc`, `pcloud-crypto`, `pcloud-resilience`,
`pcloud-store`, `pcloud-config`) the only production raw `.unwrap()`
sites are **3** byte-slice `try_into()` calls in
`pcloud-fs/src/write_journal.rs` — all on infallible `[u8; 12]`
sub-slice conversions. These are converted to `.expect("invariant: …")`
with documented invariants:

| Before                                                  | After                                                                                                                                 |
|---------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------|
| `pcloud-fs/src/write_journal.rs:318` `.unwrap()`        | `.expect("invariant: header[..4] is 4 bytes by const construction")`                                                                  |
| `pcloud-fs/src/write_journal.rs:323` `.unwrap()`        | `.expect("invariant: header[4..8] is 4 bytes by const construction")`                                                                 |
| `pcloud-fs/src/write_journal.rs:324` `.unwrap()`        | `.expect("invariant: header[8..12] is 4 bytes by const construction")`                                                                |

The other ~7 `.expect()` sites in production daemon paths
(`transfer_bridge.rs:261` chunked-upload invariant,
`transport_factory.rs:166` GlobalRetryBudget injection,
`mount_runtime.rs:877/919` adapter take-once,
`audit_verifier_service.rs:460` thread-spawn,
`integrity_sweeper_service.rs:815/1082` thread-spawn) all already carry
human-readable invariant messages and are correct as-is.

**Honest count:** 3 conversions made; the remaining 7 reachable
production `.expect()` sites are documented invariants and do not
warrant change. The audit's headline figure of 122 was inflated by
test-module sites and doc-comment mentions of `.unwrap()`.

## 5. `cargo test --workspace --lib --no-fail-fast`

**Result:** all green.

- 33 lib-test binaries
- **1597 passed / 0 failed / 3 ignored**

Output: `/tmp/cargo-test.log`.

### Cross-stream interaction bug found and fixed

Stream-D added `pause_on_battery: bool` to `SyncLoopConfig` in
`pcloud-config/src/sync_loop.rs` but did **not** add the field to the
JSON schema in `pcloud-config/src/schema.rs`. The 4 loader tests
(`secure_mode_file_loads`, `group_readable_file_warns_in_development`,
`insecure_flag_overrides_rejection`, `v0_document_is_migrated_and_loads`)
serialise a real `ConfigProfile` and validate against the schema; the
unknown property triggered `additionalProperties=false` rejection.

Fix: added `pause_on_battery: { "type": "boolean" }` to both the JSON
schema string (`CONFIG_SCHEMA_JSON`) and the typed `SYNC_LOOP_NODE`
properties array. This is the same `pause_on_battery` declared on the
typed config struct, so the schema now matches reality.

## 6. STATUS.md / matrix consistency

CSV count by `csv.reader`:

```
Total rows: 186
  Implemented: 154
  Partial:       2
  Rejected:     30
```

`STATUS.md` line 58: **`154 / 2 / 0 / 30 (186 rows)`**.

Consistent. Rows 124 and 142 (`psync_crypto_share_folder` /
`psync_crypto_account_teamshare`) remain `Partial` pending `bd-1du.5`
RSA-4096 share-temppass closure — matches the brief.

---

## Notes for the human reviewer

1. **MSRV vs. clippy collision.** The newer clippy aggressively
   recommends let-chains, but the workspace MSRV is 1.85. The fix
   chosen — workspace `[lints.clippy]` allow for `collapsible_if`,
   `collapsible_match`, and `manual_is_multiple_of` — is the smallest
   intervention that keeps both the MSRV and the CI gate honest. If
   the project bumps MSRV to 1.88+ in the future, these allow lines
   should be removed and individual sites converted.

2. **Schema drift vs. typed config.** Stream-D should have updated
   `schema.rs` alongside `sync_loop.rs`; the fact that the test was
   the first signal is a process gap. Consider a CI lint that walks
   every `serde::Serialize` config struct and asserts every field is
   covered by the schema.

3. **Unwrap audit was inflated.** The audit's "122 daemon-reachable
   unwraps" measurement should be re-taken with the same
   test-stripping pass used here. The real reachable count is single-
   digit, and all but 3 are documented invariants.
