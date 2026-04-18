# Section 8: CLI & SDK Surface — Sonnet Audit

**Auditor**: Sonnet (independent, cross-validating with Opus)
**Date**: 2026-04-18
**Scope**: `crates/pcloud-cli/` and `crates/pcloud-sdk/src/lib.rs`

---

## MEDIUM — upload `from-file` offset parameter silently ignored

**File**: `crates/pcloud-daemon/src/runtime.rs:2491-2492`

`upload_write_from_file_ipc` documents: "offset is recorded for audit but the
implementation always uploads from the beginning of the file." The CLI surface
(`pcloudc upload from-file <SESSION_ID> <LOCAL_PATH> [OFFSET]`) accepts an
offset positional, lowers it into the IPC envelope, and it reaches the daemon —
but the daemon discards it and always reads from byte 0. Callers who supply a
non-zero offset will silently receive wrong behavior. The C `upload_writefromfile`
semantics allow an offset into the source file. This is an undocumented
behavioral gap on a newly-landed surface.

**Remediation**: Either honour the offset in `upload_write_from_file_ipc` (slice
the read buffer at `offset` before calling `upload_bytes`), or reject non-zero
offsets with `ResponseStatus::InvalidRequest` and update CLI help to reflect the
limitation.

---

## MEDIUM — new CLI surfaces absent from shell-completion tree

**File**: `crates/pcloud-cli/src/completion.rs:209-221`

The `build_cli()` completion tree registers `upload create/pause/resume/cancel/list`
subcommands under the `upload` group but does **not** register `from-file`
(`Command::UploadFromFile`). Similarly, `create-tree-from-paths`
(`Command::CreateTreeLinkFromPaths`) is absent from the completion tree even
though both commands are fully wired through `normalize_args` and `into_request`.

Tab-completion users will not discover these surfaces; scripts relying on
completion-generated argument lists will silently omit them.

**Remediation**: Add `.subcommand(sub("from-file", "Upload bytes from a local
file into an existing upload session"))` under the `upload` group in
`build_cli()`. Add `create-tree-from-paths` under the `publink` group (currently
only `send` is listed there).

---

## MEDIUM — `dispatch` method exposes `pcloud_ipc` types on SDK public surface without re-export

**File**: `crates/pcloud-sdk/src/lib.rs:1312`

`EmbeddedDaemon::dispatch` takes `pcloud_ipc::Request` and returns
`pcloud_ipc::Response`. These types are not re-exported from `pcloud-sdk`, so
callers must add a direct `pcloud-ipc` dependency and `use pcloud_ipc::{Request,
Response}`. The SDK doc-example at line 1308 confirms this. This is a semver
discipline gap: any `pcloud-ipc` internal type change silently becomes a
semver-breaking change for all SDK consumers even if they only depend on
`pcloud-sdk`. Standard practice is to either re-export the types from the SDK
crate or wrap them in SDK-owned newtypes.

**Remediation**: Add `pub use pcloud_ipc::{Request, Response, ResponseStatus,
Method};` to `pcloud-sdk/src/lib.rs` and update SDK examples to import from
`pcloud_sdk` only.

---

## MEDIUM — `tls-native` feature flag documented but not implemented

**File**: `crates/pcloud-sdk/Cargo.toml:9-16`

A TODO comment acknowledges that the `tls-native` / `tls-rustls` feature flags
are not wired. The SDK ships without feature-gated TLS backend selection. The
section-8 audit criterion "feature flag combinations all compile" cannot be
satisfied for the `tls-native` path. Any downstream that embeds the SDK and needs
a system-TLS backend (FIPS environments, OS-certificate-store integration) has no
supported path.

**Remediation**: Wire the feature flags as documented in the TODO:
`tls-rustls = ["pcloud-proto/tls-rustls"]`, `tls-native =
["pcloud-proto/tls-native"]`. Gate CI matrix on both combinations.

---

## LOW — `--version` git-SHA falls back to "unknown" when `GIT_HASH` env is absent

**File**: `crates/pcloud-cli/src/main.rs:61`, `crates/pcloud-cli/build.rs`

`version_banner()` uses `option_env!("GIT_HASH")` which evaluates to `None`
when the build is performed outside a git checkout or without the CI env var.
The resulting banner (`pcloudc 0.1.0 (unknown, release)`) is correct in
structure but cannot be used for reproducibility audits or support triage. The
`build.rs` silently soft-fails rather than failing the build or writing a
deterministic placeholder.

**Remediation**: Low severity — the existing behaviour is intentional and
documented. For enterprise builds consider making `GIT_HASH` a required
`cargo:rustc-env` via `cargo:warning` when neither git nor the env var is
present, so build-pipeline operators notice missing provenance.

---

## LOW — argv-password warning goes to stderr only; no structured audit event

**File**: `crates/pcloud-cli/src/app.rs:1556, 1579, 1602, 3160`

When a password is supplied on argv without `--allow-argv-password`, the CLI
prints a stderr warning and continues. This is better than the C baseline but
the exposure event is not surfaced to the daemon's hash-chained audit log. An
attacker who rotates `/proc/<pid>/cmdline` between the warning print and the
daemon RPC has no forensic trail.

**Remediation**: After accepting an argv password (with or without the explicit
acknowledgment flag), emit a `security.argv_password_accepted` audit event via
the daemon IPC before the credential is forwarded.

---

## LOW — `create-tree-from-paths` absent from help text

**File**: `crates/pcloud-cli/src/app.rs:291-298` (PUBLIC LINKS section)

The `create-tree-link` form (id-based) is documented. The newer
`create-tree-from-paths` variant (path-based, daemon-side resolver, row 149
parity surface) is not mentioned in the hand-written help block. Help consumers
relying solely on `pcloudc --help` will not discover path-based tree-link
creation.

**Remediation**: Add one line under the PUBLIC LINKS section documenting
`create-tree-from-paths <NAME> <PATHS...>` and the note that path resolution
happens daemon-side under the authenticated context.

---

## Summary

| Severity | Count | Key area |
|----------|-------|----------|
| CRITICAL | 0 | — |
| HIGH     | 0 | — |
| MEDIUM   | 4 | offset silently ignored; completion gaps; semver leakage; TLS feature flags |
| LOW      | 2 | version provenance; argv-password audit gap |

**New surfaces verified**:

- `upload from-file` (`UploadFromFile` / `Request::UploadWriteFromFile`): IPC
  variant exists, daemon handler exists, CLI parser wired, proptest roundtrip
  exists. Functional gap: offset parameter discarded silently (MEDIUM above).
- `publink create-tree-from-paths` (`CreateTreeLinkFromPaths` /
  `Request::CreateTreePublicLinkFromPaths`): IPC variant exists, daemon handler
  exists, CLI parser wired, proptest roundtrip exists. Fully wired end-to-end.

**SDK completeness**: 84 public methods, all documented with `///` and
`no_run` examples. `#[deny(missing_docs)]` enforced. No internal-crate type
leakage via `pub use`. `dispatch` semver coupling is the only structural concern.
