# Adding a Command — Tutorial

> **Status honesty:** this project is **pre-alpha**. No release tag has been
> cut, and native/package qualification is incomplete. Treat the steps below
> as the standing engineering contract, not as a stable public surface — the
> IPC enum is `#[non_exhaustive]` precisely so we can evolve it under you.

## 1. Purpose

This chapter is a hands-on tutorial for contributors adding a brand-new CLI
subcommand end-to-end. We walk through `pcloudc quota` as the worked example:
it asks the daemon for the authenticated account's storage quota and prints
either a human line or a JSON envelope.

Audience:

- **new contributors** who have never touched this workspace and want one
  coherent chain of file-edits to copy,
- **maintainers** who need a single page to point reviewers at when an
  incoming PR forgets a layer (parity matrix, manpage, field-selector
  whitelist, …).

After reading and following the steps, you will have touched every layer a
command must cross:

1. IPC surface — `pcloud-ipc::methods` (enum variant on `Method` or `Request`),
2. daemon dispatch — `pcloud-daemon::dispatch` / `runtime`,
3. backend orchestration — `pcloud-daemon/src/*_backend.rs`,
4. protocol client — `pcloud-proto/src/*_api.rs` (only when a new endpoint is
   needed),
5. CLI parser — `pcloud-cli::commands` (not `app.rs`: the tutorial doc
   previously mis-stated this; `Command` lives in `src/commands.rs:34`),
6. CLI dispatcher — `pcloud-cli::app`,
7. field-selector whitelist — `pcloud-cli::field_selector` (read-only JSON
   projection),
8. tests at all layers,
9. shell completion + manpage,
10. mdBook docs + CHANGELOG + parity bookkeeping.

Every step below has a **gate expectation** and a **reviewer-catch** line so
you know, before you push, what the review will find if you skip it.

## 2. Prerequisites

### Toolchain pin

- `rust-toolchain.toml` pins `channel = "1.91.0"` with `clippy` and `rustfmt`
  components.
- `Cargo.toml` declares `rust-version = "1.89"` and `edition = "2024"` on the
  workspace.
- Do **not** `rustup override` to a different channel without updating
  `rust-toolchain.toml` in the same PR.

### System deps

- `gpg` — for signed commits (mandatory; see `contributing.md`).
- `fuse3` headers — only if your command reaches into `pcloud-fs` (unlikely
  for most new commands).
- `mdbook` — optional, required only when you touch docs locally.
- `wiremock` and `tempfile` are pulled via `[dev-dependencies]`; no host
  install needed.

Sanity-check the environment once:

```sh
cd .
rustc --version          # must match rust-toolchain.toml
cargo clippy --version
cargo fmt --version
```

### Tracker bead

Open or claim a bead first:

```sh
bd list --status=open
bd add "quota: add pcloudc quota command" --parent bd-1du
```

Paste the ID into the PR body as `Refs bd-<id>`. CI **does not** block on
bead presence, but review does.

## 3. Conceptual preamble — where "a command" lives

A `pcloudc` command is a thin user-facing facade over a typed IPC round-trip:

```
   pcloudc <args>                       pcloudd
  ┌───────────────┐   UDS frame   ┌───────────────┐
  │  argv parse   │──────────────▶│  dispatch     │
  │  commands.rs  │               │  dispatch.rs  │
  │      │        │               │      │        │
  │  Command      │ pcloud-ipc    │   Request     │
  │  enum         │ Request enum  │               │
  │      │        │               │      ▼        │
  │  into_request │ Response      │   backend     │
  │      ▼        │◀──────────────│   orchestration│
  │  field select │               │      │        │
  │  render       │               │      ▼        │
  │  exit code    │               │   proto call  │
  └───────────────┘               └───────────────┘
```

Six invariants hold across every command:

1. **No `unwrap()` / `expect()`** on the hot path — failures surface as typed
   errors and map to an `ExitCode`.
2. **Secrets never cross boundaries as `String`** — use `SecretString` /
   `SecretBytes` from `pcloud-secret`.
3. **`--json` output is always available** — the `json_output` envelope
   (`{kind, command, status, message, exit_code, error?}`) is mandatory.
4. **Field selector is whitelist-only** — `FieldSelector::apply` projects
   from the already-sanitised response message (`field_selector.rs` docstring
   at lines 20–30 pins this invariant with a test).
5. **`Method` enum is `#[non_exhaustive]`** — every downstream `match` must
   have a fallthrough arm (`methods.rs:34`).
6. **Dispatch is async and non-blocking** — no `std::fs::read_to_string`, no
   `reqwest::blocking`. Use the tokio runtime already threaded through.

## 4. Detailed walkthrough

### Step 1 — CLI parser

**File:** `crates/pcloud-cli/src/commands.rs`
**Anchor:** `pub enum Command {` at line **34**.

> ⚠️ The earlier draft of this page named `app.rs` here. That is wrong:
> `Command` is declared in `commands.rs`. `app.rs` only routes from the
> parsed `Command` to an IPC `Request` (via `Command::into_request`,
> declared at `commands.rs:512`).

```rust
// crates/pcloud-cli/src/commands.rs — near line 34
pub enum Command {
    // … existing variants …

    /// `pcloudc quota` — show storage quota and usage for the
    /// authenticated account. Daemon handler: [`pcloud_ipc::Method::Quota`].
    /// JSON: `{kind:"success",command:"quota",message:{…},…}`.
    Quota,
}
```

**Gate expectation:** `cargo check -p pcloud-cli --locked` must compile;
`Command` derives `Debug, Clone, PartialEq, Eq` so any non-`Eq` field (e.g.
`f64`) breaks the build instantly.

**Reviewer catch:** missing doctype comment — each variant documents the
daemon handler it maps to. The whole enum is doc-comment-reviewed; reviewers
reject silent variants.

### Step 2 — `Command::into_request`

**File:** same file, `impl Command` at line **510**, method `into_request` at
**512**.

```rust
// crates/pcloud-cli/src/commands.rs — inside fn into_request(...)
Command::Quota => Request::Plain { method: Method::Quota },
```

`Request::Plain { method }` is the argumentless dispatch wrapper
(`methods.rs:25` documents this). Commands that take arguments build a
dedicated `Request` variant instead.

**Gate expectation:** `cargo test -p pcloud-cli --lib` — the existing
into_request_* tests round-trip each `Command → Request` pair.

**Reviewer catch:** if you add the `Command` variant but forget
`into_request`, the compiler flags a non-exhaustive `match` immediately.

### Step 3 — IPC surface

**File:** `crates/pcloud-ipc/src/methods.rs`
**Anchors:**

- `pub enum Method { … }` at line **35**,
- `pub enum Request { … }` at line **188**,
- constants at `protocol.rs:39`: `IPC_PROTOCOL_VERSION: u16 = 1`.

```rust
// crates/pcloud-ipc/src/methods.rs — inside #[non_exhaustive] enum Method
/// Storage quota + used bytes snapshot. Mirrors `pcloudc quota`. Retries
/// follow the standard `ResponseStatus` classification (auth errors are
/// non-retryable; transport errors are retryable).
Quota,
```

Do **not** bump `IPC_PROTOCOL_VERSION` for an additive `Method` variant —
`#[non_exhaustive]` already gives forward-compatible decoding. The version
bump is reserved for frame-layout changes.

A rich response (structured quota) requires a typed variant on `Response`,
not `Response::Message(String)`. Grep `ResponseStatus` (`methods.rs:924`) to
see the classification rules before choosing a shape.

**Codec test** — add to the same file's `#[cfg(test)] mod tests`:

```rust
#[test]
fn method_quota_roundtrip_json() {
    let req = Request::Plain { method: Method::Quota };
    let bytes = serde_json::to_vec(&req).unwrap();
    let back: Request = serde_json::from_slice(&bytes).unwrap();
    assert!(matches!(back, Request::Plain { method: Method::Quota }));
}
```

**Gate expectation:** `cargo test -p pcloud-ipc` green. Framer property
tests (`proptest_framer`, 4 properties × 128 cases) automatically cover the
new variant because they operate on `Vec<u8>` payloads, not typed variants.

**Reviewer catch:** a reviewer who sees a direct `Request::Quota` where
`Request::Plain { method: Method::Quota }` would do will ask why. Rich
request variants exist (see the link/sync families), but adding one costs
three additional places in the dispatch tree.

### Step 4 — Daemon dispatch

**File:** `crates/pcloud-daemon/src/dispatch.rs`
**Anchors:**

- `match request { … }` at line **100** (inbound routing),
- `pub fn dispatch(…)` at line **263**.

```rust
// crates/pcloud-daemon/src/dispatch.rs — inside the match at line 100
Method::Quota => {
    let handle = runtime.auth_handle().ok_or(UnifiedError::Unauthenticated)?;
    let snap   = runtime.account_backend().quota(handle).await?;
    Ok(Response::Message(format!(
        "quota: quota={} used={} free={}",
        snap.quota,
        snap.used,
        snap.quota.saturating_sub(snap.used),
    )))
}
```

Rules on this arm:

- **never panic** — use typed errors; `UnifiedError` carries auth, network,
  conflict, and retryability signalling,
- **never block** — the dispatch task is single-threaded per connection,
- **never log secrets** — `SecretString::Debug` is redacted by construction,
  but don't format-string a bearer token even once.

**Gate expectation:** `cargo clippy -p pcloud-daemon --all-targets -- -D
warnings` must be clean. `clippy::await_holding_lock` and
`clippy::blocks_in_conditions` catch the common dispatch bugs.

**Reviewer catch:** any `.unwrap()`, any `.expect("should never fail")`, any
synchronous I/O call — three hard reject signals on this layer.

### Step 5 — Backend

**File:** `crates/pcloud-daemon/src/account_backend.rs`

```rust
impl AccountBackend {
    /// Fetch a quota snapshot, cached for 30 s to absorb CLI retries.
    pub async fn quota(&self, handle: AuthHandle) -> Result<QuotaSnapshot, UnifiedError> {
        if let Some(snap) = self.quota_cache.get_fresh(Duration::from_secs(30)) {
            return Ok(snap);
        }
        let token = self.auth.token(handle).await?;
        let api   = self.proto.userinfo(token.expose_secret()).await?;
        let snap  = QuotaSnapshot {
            quota: api.quota,
            used:  api.usedquota,
            publink_quota: api.publink_quota,
        };
        self.quota_cache.put(snap.clone());
        Ok(snap)
    }
}
```

`QuotaSnapshot` is the internal daemon-side shape; keep it distinct from any
IPC wire type so the two can evolve independently.

**Reviewer catch:** backend methods that return raw `reqwest::Response` or
`serde_json::Value` are rejected — the backend owns the domain model, the
`pcloud-proto` layer owns the wire.

### Step 6 — Protocol method (only when a new endpoint is needed)

`/userinfo` already exists in `pcloud-proto/src/account_api.rs`; quota reuses
it. For a genuinely new endpoint:

```rust
// crates/pcloud-proto/src/account_api.rs
impl AccountApi {
    pub async fn userinfo(&self, token: &str) -> Result<UserInfoReply, ProtoError> {
        let reply: UserInfoReply = self
            .http
            .get("/userinfo")
            .bearer(token)
            .send_json()
            .await?;
        reply.check_result()?;
        Ok(reply)
    }
}
```

`check_result()` translates the pCloud envelope's `result != 0` into a
typed `ProtoError`. Do not treat an HTTP 200 with `result != 0` as success.

**Reviewer catch:** unchecked `result` field, missing wiremock coverage on
the new endpoint, or a bare `reqwest::Error` leaking out of the crate
boundary.

### Step 7 — Field-selector whitelist (read-only commands)

**File:** `crates/pcloud-cli/src/field_selector.rs`

The field selector is a **tiny jq subset** (`key.key.0.key`) that projects
from `Response::message` parsed into `serde_json::Value`. It is
**whitelist-only by construction**: it cannot reach `SecretString` /
`SecretBytes` because those types do not implement `serde::Serialize` for
their protected payload. The test `assert_no_secret_in_value` (line 23 of
the module doc pins this) asserts the invariant.

For `quota`, no additional code is needed — the message already parses via
`parse_message_to_json` (line 222) which handles the `userinfo: quota=…,
used=…` legacy flat form. Add a regression test in the same file:

```rust
#[test]
fn quota_field_selector_projects_quota() {
    let msg = "quota: quota=10737418240 used=3221225472 free=7516192768";
    let v = parse_message_to_json(msg);
    let got = FieldSelector::parse("quota").apply(&v).unwrap();
    assert_eq!(got, serde_json::json!(10_737_418_240u64));
}
```

**Reviewer catch:** proposing a new field that bypasses
`parse_message_to_json` is rejected — the selector must *only* see values
that have already been sanitised.

### Step 8 — CLI dispatcher

**File:** `crates/pcloud-cli/src/app.rs`

`app.rs` wires the parsed `Command` to the IPC transport and renders the
response. Most new commands do not need changes here — `Command::into_request`
handles the lowering and the common renderer covers plain-text + JSON. Only
touch `app.rs` when your command needs custom progress rendering or a
multi-step interactive flow.

## 5. Tests

### 5.1 IPC codec

`crates/pcloud-ipc/src/methods.rs` — inline `#[cfg(test)]` round-trip.

### 5.2 CLI parse

`crates/pcloud-cli/src/commands.rs` — inline `into_request` mapping test.

### 5.3 Backend (mocked API)

Inline in `account_backend.rs`, using a stubbed `AccountApi`. Assert cache
hit after the first call.

### 5.4 Dispatch round-trip

`crates/pcloud-daemon/tests/dispatch_quota.rs` — spawn a runtime shell with
a mocked API and assert `dispatch(Request::Plain { method: Method::Quota })`
yields the expected `Response::Message`.

### 5.5 CLI integration

`crates/pcloud-cli/tests/cmd_quota.rs` — `assert_cmd::Command::cargo_bin
("pcloudc")` against a `TestDaemon::spawn()` helper; assert
`stdout.contains("quota")` for plain, `stdout.starts_with("{")` for
`--json`.

### 5.6 Doctest

Every public function new-to-this-PR ships at least one runnable doctest.
Doctests are part of the workspace gate (`cargo test --workspace --locked`
runs `--doc`).

### 5.7 Live-gated (optional)

If the command's semantics are only observable against a real pCloud account
(e.g. crypto state, pending transfers), add a case to
`crates/pcloud-live-e2e/`. These tests are tag-gated and do not run on
vanilla PRs — see `testing.md §7`.

## 6. Gate checklist

Run locally **before** pushing:

```sh
cd .
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo check  --workspace --all-targets --locked
cargo test   --workspace --locked
cargo doc    --workspace --no-deps --document-private-items
cargo audit  --deny warnings
cargo deny   check
```

Each line is a CI gate. `clippy -D warnings` is non-negotiable — the
workspace has held zero warnings across every reconciliation wave.

## 7. Common mistakes

- **Declared `Command` in `app.rs`.** *How reviewers catch it:* `Command` is
  namespaced from `pcloud_cli::commands`; a duplicate declaration in `app.rs`
  either compiles by shadowing (in which case `into_request` wiring silently
  breaks) or collides on import. Reviewers grep for the symbol.
- **Forgot to implement `into_request`.** *How reviewers catch it:* the
  `impl Command` match becomes non-exhaustive and `cargo check` fails with a
  line number pointing at `commands.rs:512`.
- **Bumped `IPC_PROTOCOL_VERSION` for an additive `Method` variant.** *How
  reviewers catch it:* `protocol.rs:39` touches show up in `git diff` and a
  reviewer asks for the wire-layout change justification; there isn't one.
- **Used `String` for a bearer token.** *How reviewers catch it:* secret
  handling is audited by grep — `rg 'token: String'` on the daemon tree
  surfaces the regression immediately.
- **`.unwrap()` on the dispatch arm.** *How reviewers catch it:*
  `clippy::unwrap_used` is enabled on the daemon crate; CI blocks merge.
- **No `--json` output mode.** *How reviewers catch it:* a missing
  `json_output::success(...)` call fails the integration test harness that
  parses the envelope.
- **Field-selector reads from an unsanitised blob.** *How reviewers catch
  it:* the `assert_no_secret_in_value` pinning test catches it; the reviewer
  confirms the path is via `parse_message_to_json`.
- **No parity matrix update.** *How reviewers catch it:* the PR template has
  a mandatory checkbox; parity-matrix CI diff surfaces the omission.
- **No manpage section.** *How reviewers catch it:* the packaging CI job
  runs `ronn --roff` and fails if a referenced subcommand has no section.
- **No bead.** *How reviewers catch it:* reviewer bounces the PR; tracker
  hygiene is in `contributing.md`.

## 8. Shell completion + manpage

### Completion

`clap_complete` generates completions from the `Command` enum
automatically; a verification build confirms emission:

```sh
cargo build -p pcloud-cli --features completions
ls target/debug/build/pcloud-cli-*/out/completions/
# expect: pcloudc.bash, pcloudc.zsh, pcloudc.fish, _pcloudc
```

### Manpage

`packaging/man/pcloudc.1.ronn`, add a `## QUOTA` section:

```ronn
## QUOTA

  `pcloudc quota`

  Show storage quota and usage for the authenticated account.
  Output is a single line of the form `quota: quota=… used=… free=…`.
  With `--json`, emits the standard envelope.

  Exit codes: 0 on success, 3 on auth failure, 4 on transport error.
```

Regenerate:

```sh
ronn --roff packaging/man/pcloudc.1.ronn
```

## 9. mdBook docs, CHANGELOG, parity bookkeeping

- `docs/book/src/cli/reference.md` — add `quota` to the subcommand table.
- `CHANGELOG.md` — one-line entry under `[Unreleased] → Added` (there is no
  release tag yet; every entry lives under `[Unreleased]`).
- `C_FEATURE_PARITY_MATRIX.csv` — flip the relevant row to `Implemented`
  with a file:line citation. Reconcile totals in `STATUS.md` **first**, then
  update `CLAUDE.md` if the handoff state changed materially.
- Do **not** claim "parity" in release notes until `bd-1du.10` closes.

## 10. FAQ

**Q: Do I need a new `Request::*` variant, or `Request::Plain { method }`?**
A: Argumentless commands use `Plain`. Commands that carry non-trivial args
(paths, IDs, policy enums, secrets) get a dedicated `Request::*` variant so
the wire shape is typed end-to-end.

**Q: Do I need to bump `IPC_PROTOCOL_VERSION`?**
A: No, not for additive `Method` variants. `#[non_exhaustive]` makes the
decoder tolerant. Bump only when the **frame layout** changes.

**Q: Can I reach into `SecretString` from the renderer?**
A: No. The renderer receives a `serde_json::Value` produced from the
daemon's sanitised `Response::message`. Secrets never leave their wrapper.

**Q: Why put the tutorial on `quota` specifically?**
A: It is read-only, reuses an existing endpoint, exercises the field
selector, and round-trips through every layer — the minimum non-trivial
surface.

**Q: Do I need chaos or mutation-testing coverage?**
A: Not per-command. Both run on a fixed crate allow-list (`pcloud-ipc`,
`pcloud-crypto`, `pcloud-auth`, `pcloud-resilience`, `pcloud-secret`). If
your new command crosses one of those boundaries, the existing crate-level
coverage catches regressions automatically.

**Q: Can I skip the JSON envelope for a trivial command?**
A: No. Every command ships both `human` and `json`. Scripts depend on the
envelope invariant.
