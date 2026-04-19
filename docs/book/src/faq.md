# Frequently Asked Questions

Distilled from the 20-reviewer bundle (`.reviews/01..20-*.md`) and the
R1–R10 comparative review wave (`.reviews/R1..R10-*.md`). Each answer is
under 100 words. For authoritative status see
[`STATUS.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/STATUS.md)
and the [Parity Status](./parity/status.md) chapter.

## Status & Readiness

### 1. Is the Rust rewrite production-ready?

**No.** `bd-1du.10` is still open and three of six `Partial` matrix
rows block the final parity gate — mounted-drive wiring, chunked upload
state machine, and the `UploadSession` pause/resume semantics. Until
those close, do not describe the build as "full parity", "production
ready", "enterprise ready", or "drop-in replacement". The retained
surface is strong and live-verified for auth, shares, crypto,
public links, and backup — but the FUSE write path has not had a
live host run. See [Parity Status](./parity/status.md).

### 2. Can I use it as a drop-in replacement for the C client?

Not yet. The daemon, CLI, auth, crypto, shares, and public-link paths
are usable, but the mounted-drive runtime is a shell pending
`bd-1du.4.6` wiring, and `pcloud-sdk`'s FS helpers follow from that.
Users who only need API-level upload/download/sync-root lifecycle can
evaluate the Rust path today; users who rely on the pCloud Drive
mount point should stay on the C client. Tracked under epic `bd-1du`.

### 3. What does "T1 vs T3" mean in the reviews?

Reviewer bundles tier findings by blast radius: **T1** is a user-facing
correctness or security defect that blocks release; **T2** is a
regression-class risk that needs a fix before GA; **T3** is a hardening
or polish item — welcome, not blocking. Parity work that closes a
`Partial` row is typically T1; ADR-tracked design choices (e.g. ADR
0007 crypto-password-not-persisted) are intentional T3 deltas. See
`.reviews/19-parity-honesty.md` for axis-by-axis tiering.

### 4. What counts as "Implemented" in the parity matrix?

"Implemented" means a C equivalent exists and is exercised on an
identified retained Rust code path with a test or live-run cite. It
**does not** mean "end-to-end on a live mount". The FUSE write path
is the canonical example — `FuseAdapter` methods are implemented, but
the daemon mount lifecycle wiring is still pending. See footnote
`fuse-wiring` on [Parity Status](./parity/status.md) and ADR
[0010](./adr/0010.md).

## Security

### 5. Are my auth tokens persisted by default?

No. Durable auth-token persistence is opt-in via
`PCLOUD_DURABLE_AUTH_TOKENS=1`. When enabled, the vault file is
`0600` under a `0700` parent directory with owner-only checks on both
sides. Tokens live inside `SecretString`/`SecretBytes` wrappers that
zeroize on drop. See ADR [0005](./adr/0005.md) and
`crates/pcloud-daemon/src/auth_vault.rs`. Passwords are
**never** persisted — that carve-out is recorded as ADR
[0007](./adr/0007.md).

### 6. How is the crypto password handled?

The crypto password is kept in a `SecretString` in memory only and
zeroized on drop. Unlike the legacy C client, it is not written to any
on-disk cache. Reviewer 02 and R7 flagged the C persistence behavior
as a residual risk; we intentionally do not carry it forward. See ADR
[0007](./adr/0007.md). Crypto folder metadata filenames are
deterministically encoded and sector payloads use AES-256-GCM. `change_crypto_pass`
remains tracked work.

### 7. Is the local IPC socket secure?

Yes. The daemon uses `AF_UNIX` with binary length-prefixed framing
(ADR [0002](./adr/0002.md)), owner-only runtime directory and socket
permissions, peer credential checks, and a 1 MiB frame cap to reject
oversized or malformed clients. Audit/persistence failures on the
control path surface as errors rather than being swallowed. Do not
weaken these defaults — see Security Model.

### 8. Can production traffic fall back to plaintext?

No. The production config rejects any transport downgrade away from
TLS. API-server selection parity (`set_api_server`) is local runtime
state and does not relax transport policy. Endpoint overrides must be
explicit and test-covered. Reviewer R7 confirmed the posture is
stricter than the C client's.

### 9. How do I report a security bug?

Do **not** open a public GitHub issue. Use the disclosure channel
listed in the repo's `SECURITY.md` (private advisory) with a reproducer,
affected commit, and impact assessment. Expected triage SLA follows
the project's published window. Do not include real credentials,
real auth tokens, or account-identifying data in the report — use the
daemon's `--fixture` mode or a scrubbed capture instead.

## Migration

### 10. Can I migrate my existing pCloud config to the Rust daemon?

The Rust daemon reads its own config under `$XDG_CONFIG_HOME/pcloud-rs/`
and does not parse the C client's legacy config by default. `pcloudc
login` + explicit `sync add` reproduces a typical C-side sync set.
Auth vaults are not shared: the C client stores credentials
differently and the Rust vault format is documented in ADR
[0005](./adr/0005.md). See R8 for interop notes.

### 11. Will my mounted drive keep working during migration?

If you rely on the pCloud Drive mount, stay on the C client until
`bd-1du.4.6` lands. The Rust `pcloud-fs` scaffold and `FuseAdapter`
work in isolation, but the daemon does not drive a real mount
lifecycle yet. You can run both binaries on the same host as long as
you do not mount the same path from both — the Rust daemon uses a
different control socket and runtime dir.

### 12. What happens to in-flight uploads on restart?

Partial uploads are tracked in an NDJSON journal with `fsync` on
boundary commits (`.archive/reviews/UPLOAD-SPEC-14042026.md`). On
daemon restart the single-shot path resumes from the last durable
offset. The chunked multi-write resume story is still Partial until
the state machine behind matrix row 92 lands — see the
[closure checklist](https://github.com/ezechiel203/pcloud-rs/blob/main/docs/parity/bd-1du-10-closure-checklist.md).

## Testing

### 13. How do I run the full test suite?

From ``: `cargo test`. For focused subsets used during
parity review, `cargo test -p pcloud-proto -p pcloud-daemon -p
pcloud-cli`. Live-network tests are gated behind environment flags
(`PCLOUD_LIVE_AUTH=1`, `PCLOUD_FUSE_LIVE=1`) and will skip without a
real account. Coverage CI runs weekly mutants and nightly fuzz —
see `PLAN_A_PLUS_P2_REPORT.md`. The C tree still builds with `make
-j4` from the repo root.

### 14. How do I run a live-auth verification?

Export `PCLOUD_LIVE_AUTH=1` plus `PCLOUD_TEST_USER`/`PCLOUD_TEST_PASS`
(or a token) and run `cargo test -p pcloud-daemon --test live_auth`.
TFA codes go through `stdin` prompts; use `--tfa-code` or
`--recovery-code` flags on the CLI for non-interactive flows. Do not
commit credentials, do not log secrets, and prefer a dedicated test
account. See `crates/pcloud-daemon/tests/live_auth.rs`.

### 15. How do I reproduce a bug from a user report?

Collect `pcloudc doctor` output, the daemon log at `RUST_LOG=debug`,
and the matrix row (if any) the user cites. Replay the request via
`pcloudc --ipc-trace` to capture a binary frame, then feed it into
the unit test harness. Upload traces live in the journal; FUSE mount
failures land in `/proc/self/mountinfo` and `pcloudc mount
--force-umount` cleans orphans. Do **not** paste real tokens.

## Performance

### 16. How does the Rust client compare to the C client on throughput?

The streaming download path uses a 64 KiB buffer (ADR
[0008](./adr/0008.md)) chosen to balance syscall count, memory
footprint, and TLS record size. On the same hardware the Rust path
matches C throughput on large downloads and improves on small-file
workloads due to the `Arc<Vec<u8>>` page cache and O(1) LRU
eviction. Upload throughput is currently bounded by the single-shot
daemon path; see matrix row 92. See R5 and `PERF-BASELINE-14042026.md`.

### 17. How much memory should the daemon use at rest?

Idle daemon footprint is dominated by the page-cache cap
(`pcloud_page_cache_bytes`, default 64 MiB) and the sync-root
registry. Expect ~80–120 MiB RSS idle on Linux with a handful of
sync roots. Memory is bounded, not unbounded — there is no
per-request leak path in the retained surface. If you observe
unbounded growth, capture a heap profile and file a bug.

### 18. Can I cap bandwidth?

Yes. The `BandwidthPacer` in the daemon applies a rolling-window
token bucket on transfer byte streams. Configure via
`pcloudc set bandwidth --up <bytes/s> --down <bytes/s>` or the
equivalent config key. The cap applies at the HTTP client layer,
below retries, so bursts cannot escape it. See the P4–P6 final
report for wiring details.

## Platforms

### 19. What's the platform support roadmap?

Linux is the primary target and the only platform where live-verified
flows run today. The codebase compiles on macOS and FreeBSD; a
Windows port is scoped but not yet delivered. Mounted-drive parity
(`bd-1du.4`) is Linux-first via FUSE; macOS will follow through
`macfuse`, Windows through `winfsp`, once the Linux path is proven.
See the Platforms subtree under Operations and R10.

### 20. Does the Rust client auto-update?

No. ADR [0006](./adr/0006.md) records the deliberate decision not to
mirror the C client's `psync_check_new_version*` flow. Distribution
channels (distro packages, flake, nfpm Debian package) own updates.
This is a hardening decision, not an oversight: the legacy update
path was one of the reviewer-flagged attack surfaces.

### 21. Is there a Web UI?

A `pcloud-web` MVP shipped in the P4 wave (see
`PLAN_A_PLUS_FINAL_REPORT.md`). It is usable for status, sync-root
management, and transfer observation but is **not yet** a full admin
console. Known stability gaps: concurrent session handling, TFA
re-prompt on session expiry, and live upload progress. Track
enhancements in `.reviews/20-enhancements-brainstorm.md` and epic
`bd-1du` follow-ons. Treat it as beta.

## Development

### 22. How do I add a new IPC command?

1. Extend the enum in `pcloud-proto` and round-trip via the binary
   framing. 2. Add a backend handler under `pcloud-daemon/src/*_backend.rs`.
3. Expose it on the SDK (`pcloud-sdk`) and CLI (`pcloud-cli`).
4. Add tests — unit, proptest where framing is non-trivial, and a
gated live test if it hits the network. 5. Update the parity matrix
and `C_FEATURE_PARITY_REVIEW.md` if it maps to a C function. See
Development → Adding a Command.

### 23. What's the license?

See the repo root `LICENSE` file. The legacy C tree inherits
pCloud's upstream licensing; the Rust rewrite sits under the same
terms unless a crate's Cargo.toml says otherwise. Do not add a
dependency under a copyleft license without a `cargo deny` exception
and an ADR entry. Contributor sign-off follows the CONTRIBUTING
guide.

### 24. How is the parity matrix kept honest?

Single source of truth: `C_FEATURE_PARITY_MATRIX.csv`. `STATUS.md`
reproduces the aggregate (ADR [0009](./adr/0009.md)). Reviewer 19
(`19-parity-honesty.md`) re-runs on each wave and grades; current
grade is B+, targeting A at `bd-1du.10` close. Every `Partial` row
has a tracked bead, an exit criterion with a named test, and a
justification in the [closure checklist](https://github.com/ezechiel203/pcloud-rs/blob/main/docs/parity/bd-1du-10-closure-checklist.md).
Rejected rows live in `REJECTED-RATIONALES-14042026.md`.

### 25. Where do I start if I want to contribute?

Read [Contributing](./development/contributing.md), pick an open
bead (`bd list --status=open`), and confirm scope in the bead
comment before coding. Small wins: close a Partial matrix row with
a test, harden an edge in the mounted-drive scaffold, or extend
platform coverage in `operations/platforms/`. Large wins:
`bd-1du.4.6` (FUSE mount lifecycle wiring) or matrix row 92
(chunked upload state machine). Parity claims are not moved
without evidence.
