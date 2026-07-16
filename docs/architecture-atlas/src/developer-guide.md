# Developer navigation and extension guide

## Find the owner before editing

```text
user syntax?             → pcloud-cli
stable external Rust API?→ pcloud-sdk-public
IPC schema/transport?    → pcloud-ipc
process lifecycle?       → pcloud-daemon
remote drive semantics?  → pcloud-backends::remote_fs
pCloud method encoding?  → pcloud-proto
sync planning?           → pcloud-engine
kernel filesystem?       → pcloud-fs
durable relational state?→ pcloud-store
secret handling?         → pcloud-secret / daemon vault
metrics/audit/redaction? → pcloud-observability
```

The generated [crate catalog](generated/crates/index.md) gives the complete
package list, targets, direct dependencies, file inventory, and public items.

## Add a daemon-mediated operation

The usual vertical slice is:

1. Define a typed request/response in `pcloud-ipc`.
2. Add the daemon dispatch/runtime mapping.
3. Put reusable business behavior in the owning backend.
4. Add or extend the typed protocol method if remote API work is needed.
5. Add CLI and/or public SDK exposure only when the surface belongs there.
6. Add unit tests at the owner and an integration test across the IPC/runtime
   boundary.
7. Update parity/status/docs if it changes a public capability claim.

For drive-like operations, extend RemoteFs first so CLI, SDK, sync, mount, and
gateways do not diverge.

## Add durable state

1. Define an additive store migration.
2. Add a typed repository rather than spreading SQL through runtime code.
3. Decide transaction boundaries and crash ordering.
4. Define replay/idempotency behavior.
5. Add migration-forward, restart, partial-write, and corruption tests.
6. Update backup/restore and operator documentation.

## Add a platform adapter

Keep the portable trait and policy in the owning core crate. The native module
should translate:

- native identities into the common peer identity;
- native mount callbacks into portable filesystem operations;
- native vault behavior into the vault trait;
- native service lifecycle into ordinary `pcloudd serve`.

Test cleanup and failure paths, not only successful construction. Native
support claims require native execution.

## Test layers

| Layer | Location / tool | Purpose |
|---|---|---|
| Unit | next to Rust modules | local invariants and error mapping |
| Crate integration | `crates/*/tests` | public crate boundaries |
| Mock protocol | `pcloud-mockserver` | deterministic remote replies |
| Fault injection | `pcloud-chaos` | retry, partial I/O, crash behavior |
| Filesystem native | `pcloud-fs` tests/workflows | kernel adapter and cleanup |
| Live remote | `pcloud-live-e2e` | credentialed pCloud behavior |
| Fuzz | root and crate fuzz targets | parsers, frames, crypto/protocol surfaces |
| DR drills | `tests/dr_drill` | operator recovery workflows |
| Packaging | packaging scripts/workflows | artifact lifecycle |

## Documentation workflow

The regular handbook under `docs/book` is the product/operator manual. This
atlas is the exhaustive architecture and source-navigation view. Update
hand-authored atlas chapters when ownership or supported paths change, then
regenerate the catalogs:

```bash
python3 docs/architecture-atlas/tools/generate.py
mdbook build docs/architecture-atlas
```

Do not hand-edit `src/generated/`.
