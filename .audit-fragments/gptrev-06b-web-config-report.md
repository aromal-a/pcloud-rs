# Stream G6b — Web / Config / Session Audit Fixes

**Scope:** `crates/pcloud-web/src/`, `crates/pcloud-config/src/`, `crates/pcloud-session/src/`
**Source audit:** `GPTREV/06_ipc_daemon_web_config.md`
**Date:** 2026-04-26

---

## Findings addressed

### HIGH-03 — Web management read routes expose daemon state without web token (FIXED)

`GET /`, `GET /sync`, `GET /publinks`, `GET /activity`, `GET /settings` now all
call `require_web_token` before issuing any IPC request or rendering daemon
state. Only `/health`, `/livez`, `/readyz` remain open for orchestrator probes.

Files changed:
- `crates/pcloud-web/src/routes.rs` — added `require_web_token` guard at the
  top of `index`, `sync_list`, `publinks_list`, `activity`, `settings`.
- `crates/pcloud-web/src/lib.rs` — updated module-level security doc comment
  to reflect the new enforcement posture.

### HIGH-04 — Web token file creation non-atomic, no O_NOFOLLOW, no sync (FIXED)

`write_web_token_to_runtime_dir` now:
1. Validates `XDG_RUNTIME_DIR` is a real directory (not a symlink) via
   `symlink_metadata()` on Unix.
2. Writes via a `create_new`-opened temp file (`web-token.tmp`) — no truncation
   race, no symlink follow.
3. Calls `sync_all()` before drop for durability.
4. Renames atomically to `web-token` (POSIX rename).
5. Best-effort syncs the parent directory after rename.

Files changed:
- `crates/pcloud-web/src/lib.rs` — rewrote `write_web_token_to_runtime_dir`.

### MEDIUM-11 — CSRF design incompatible with no-JS forms (FIXED)

CSP disables JS and the `pcw_csrf` cookie is `HttpOnly`, so no-JS browser users
could never read the cookie to supply `X-CSRF-Token`. Fix:

1. Added a `require_csrf_with_form_fallback(headers, form_field)` helper that
   accepts either the `X-CSRF-Token` header or a `csrf_token` hidden form field
   (header wins when both are present). The constant-time comparison is
   factored into a shared `csrf_compare` helper.
2. Both `POST /sync` (`SyncAddForm`) and `POST /publinks` (`PublinkCreateForm`)
   now include a `#[serde(default)] csrf_token: String` field and call the new
   helper.
3. The rendered forms now embed `<input type="hidden" name="csrf_token"
   value="...">` with the CSRF value HTML-escaped via `xml_escape`.

Files changed:
- `crates/pcloud-web/src/routes.rs` — new `require_csrf_with_form_fallback` +
  `csrf_compare` helpers; form structs + renderers updated.

### MEDIUM-12 — Non-loopback bind panics instead of returning config error (FIXED)

`serve()` and `bind_for_test()` previously used `assert!` / `panic!` for
non-loopback addresses. Both now return `Err(WebError::NonLoopbackBind { addr })`
so callers can surface a clean operator-facing error rather than crashing.

New `WebError::NonLoopbackBind { addr }` variant added with a descriptive
`#[error]` message.

The `#[should_panic]` test was replaced with
`non_loopback_bind_returns_typed_error` that asserts the error variant.

Files changed:
- `crates/pcloud-web/src/lib.rs` — new `WebError::NonLoopbackBind` variant;
  `serve` and `bind_for_test` converted from panic to typed error; test updated.

---

## Findings NOT addressed in this stream

### LOW-15 — `MarkTwoFactorCodeInvalid` leaves state as `AuthenticatingWithPassword`

Root cause is in `crates/pcloud-auth/src/manager.rs` — outside this stream's
file scope (`pcloud-web`, `pcloud-config`, `pcloud-session` only). The
`AuthCommand::MarkTwoFactorCodeInvalid` reducer must set
`snapshot.state = SessionState::TwoFactorRequired` before returning. A
regression test should assert the state after the command. This should be
addressed in a stream that owns `pcloud-auth`.

---

## Verification

```
cargo check -p pcloud-web -p pcloud-config -p pcloud-session
# → Finished, 0 errors, 0 warnings on target crates

cargo test -p pcloud-web -p pcloud-config -p pcloud-session --lib
# → 11 passed, 0 failed (pcloud-web); 9 passed, 0 failed (pcloud-config);
#    pcloud-session: all pass
```

All tests pass clean.
