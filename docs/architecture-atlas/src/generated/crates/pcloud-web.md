# `pcloud-web`

**Maturity:** Evolving product surface

**Version:** `0.1.0`

**Directory:** `crates/pcloud-web`

**Manifest:** [`crates/pcloud-web/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/Cargo.toml)

MVP Web UI scaffold for the pcloud-rs daemon (P4.5). Axum-based HTTP surface with loopback defaults and IPC-backed status views. Not the final Leptos SSR app.

## Feature-family profile

**Why it exists.** Offer a browser-readable local status/control surface without giving the browser direct access to pCloud credentials.

**What it is good for.** Loopback-first health, status, sync, and simple UI routes backed by daemon IPC.

**Why it is good at that job.** Host validation, loopback defaults, limited routes, and IPC delegation keep the MVP surface small and authority in pcloudd.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_web` | lib | [`crates/pcloud-web/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs) |
| `pcloud-web` | bin | [`crates/pcloud-web/src/main.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs) |
| `binary_coverage` | test | [`crates/pcloud-web/tests/binary_coverage.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/binary_coverage.rs) |
| `health` | test | [`crates/pcloud-web/tests/health.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/health.rs) |
| `serve_coverage` | test | [`crates/pcloud-web/tests/serve_coverage.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/serve_coverage.rs) |
| `ui` | test | [`crates/pcloud-web/tests/ui.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs) |

## Direct dependencies

`axum`, `getrandom`, `libc`, `log`, `pcloud-ipc`, `pcloud-model`, `pcloud-secret`, `serde`, `serde_json`, `subtle`, `tempfile`, `thiserror`, `tokio`, `tower`, `zeroize`

## Cargo features

No declared package features.

## File inventory (10)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-web/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-web/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/README.md) | documentation | pcloud-web |
| [`crates/pcloud-web/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs) | library root | pcloud-web |
| [`crates/pcloud-web/src/main.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs) | binary root | \[derive(Debug)\] |
| [`crates/pcloud-web/src/routes.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs) | Rust module | HTTP routes for the single-user Web UI. |
| [`crates/pcloud-web/src/templates.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/templates.rs) | Rust module | Inline HTML rendering for the MVP web UI. |
| [`crates/pcloud-web/tests/binary_coverage.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/binary_coverage.rs) | test | \[test\] |
| [`crates/pcloud-web/tests/health.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/health.rs) | test | Integration test: start the MVP web server on an ephemeral loopback |
| [`crates/pcloud-web/tests/serve_coverage.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/serve_coverage.rs) | test | \[tokio::test\] |
| [`crates/pcloud-web/tests/ui.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs) | test | Integration tests for the expanded pcloud-web UI (P4.5+). |

## Rust declaration index (143 total; 12 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `routes` | `private` | mod | [`crates/pcloud-web/src/lib.rs:113`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L113) | Read the source/rustdoc for the exact contract. |
| `templates` | `private` | mod | [`crates/pcloud-web/src/lib.rs:114`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L114) | Read the source/rustdoc for the exact contract. |
| `DEFAULT_BIND_ADDR` | `pub` | const | [`crates/pcloud-web/src/lib.rs:122`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L122) | Default bind address for the MVP web UI. Intentionally loopback by default. Use an explicit `--bind` / \[`WebC… |
| `generate_web_token` | `pub` | fn | [`crates/pcloud-web/src/lib.rs:133`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L133) | Generate a cryptographically random 64-hex-char session token. Returns an error string if the kernel CSPRNG i… |
| `generate_web_token_or_panic` | `pub` | fn | [`crates/pcloud-web/src/lib.rs:151`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L151) | Generate a session token, panicking if the kernel RNG is unavailable. This is a convenience wrapper around \[`… |
| `WebConfig` | `pub` | struct | [`crates/pcloud-web/src/lib.rs:188`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L188) | Runtime configuration for \[`serve`\]. Construct explicitly or via \[`WebConfig::default`\] (loopback default bin… |
| `fmt` | `private` | fn | [`crates/pcloud-web/src/lib.rs:223`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L223) | Read the source/rustdoc for the exact contract. |
| `default` | `private` | fn | [`crates/pcloud-web/src/lib.rs:235`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L235) | Read the source/rustdoc for the exact contract. |
| `WebError` | `pub` | enum | [`crates/pcloud-web/src/lib.rs:254`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L254) | Errors returned by \[`serve`\]. The variants distinguish *pre-serve* failures (bind) from *serve-time* failures… |
| `AppState` | `pub(crate)` | struct | [`crates/pcloud-web/src/lib.rs:284`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L284) | Shared application state passed to every request handler. Wraps the daemon socket path in an \[`Arc`\] so cloni… |
| `write_web_token_to_runtime_dir` | `private` | fn | [`crates/pcloud-web/src/lib.rs:315`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L315) | Write the web session token to `$XDG_RUNTIME_DIR/pcloud-daemon/web-token` with mode 0600. Returns the path th… |
| `write_web_token_to_dir` | `private` | fn | [`crates/pcloud-web/src/lib.rs:330`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L330) | Read the source/rustdoc for the exact contract. |
| `create_new_token_temp` | `private` | fn | [`crates/pcloud-web/src/lib.rs:363`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L363) | Read the source/rustdoc for the exact contract. |
| `random_hex_suffix` | `private` | fn | [`crates/pcloud-web/src/lib.rs:387`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L387) | Read the source/rustdoc for the exact contract. |
| `validate_owner_only_dir` | `private` | fn | [`crates/pcloud-web/src/lib.rs:400`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L400) | Read the source/rustdoc for the exact contract. |
| `validate_owner_only_file` | `private` | fn | [`crates/pcloud-web/src/lib.rs:436`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L436) | Read the source/rustdoc for the exact contract. |
| `sync_dir` | `private` | fn | [`crates/pcloud-web/src/lib.rs:469`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L469) | Read the source/rustdoc for the exact contract. |
| `serve` | `pub` | fn | [`crates/pcloud-web/src/lib.rs:491`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L491) | Start the Web UI MVP on the configured bind address. Creates internal shared state from the supplied configur… |
| `bind_for_test` | `pub` | fn | [`crates/pcloud-web/src/lib.rs:553`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L553) | Bind a listener without starting serving. Intended for integration tests that need to pick an ephemeral port… |
| `tests` | `private` | mod | [`crates/pcloud-web/src/lib.rs:575`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L575) | Read the source/rustdoc for the exact contract. |
| `default_bind_is_loopback` | `private` | fn | [`crates/pcloud-web/src/lib.rs:581`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L581) | Read the source/rustdoc for the exact contract. |
| `non_loopback_bind_is_allowed_for_testing` | `private` | fn | [`crates/pcloud-web/src/lib.rs:587`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L587) | Read the source/rustdoc for the exact contract. |
| `web_config_debug_redacts_token` | `private` | fn | [`crates/pcloud-web/src/lib.rs:604`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L604) | Read the source/rustdoc for the exact contract. |
| `web_token_file_is_atomic_owner_only_file` | `private` | fn | [`crates/pcloud-web/src/lib.rs:616`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L616) | Read the source/rustdoc for the exact contract. |
| `web_token_writer_rejects_symlink_token_dir` | `private` | fn | [`crates/pcloud-web/src/lib.rs:633`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L633) | Read the source/rustdoc for the exact contract. |
| `web_token_writer_rejects_group_readable_token_dir` | `private` | fn | [`crates/pcloud-web/src/lib.rs:648`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/lib.rs#L648) | Read the source/rustdoc for the exact contract. |
| `HELP` | `private` | const | [`crates/pcloud-web/src/main.rs:17`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs#L17) | Read the source/rustdoc for the exact contract. |
| `Cli` | `private` | struct | [`crates/pcloud-web/src/main.rs:42`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs#L42) | Read the source/rustdoc for the exact contract. |
| `TokenSource` | `private` | enum | [`crates/pcloud-web/src/main.rs:52`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs#L52) | Read the source/rustdoc for the exact contract. |
| `Mode` | `private` | enum | [`crates/pcloud-web/src/main.rs:60`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs#L60) | Read the source/rustdoc for the exact contract. |
| `default` | `private` | fn | [`crates/pcloud-web/src/main.rs:67`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs#L67) | Read the source/rustdoc for the exact contract. |
| `main` | `private` | fn | [`crates/pcloud-web/src/main.rs:80`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs#L80) | Read the source/rustdoc for the exact contract. |
| `run` | `private` | fn | [`crates/pcloud-web/src/main.rs:90`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs#L90) | Read the source/rustdoc for the exact contract. |
| `parse` | `private` | fn | [`crates/pcloud-web/src/main.rs:123`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs#L123) | Read the source/rustdoc for the exact contract. |
| `next_value` | `private` | fn | [`crates/pcloud-web/src/main.rs:218`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs#L218) | Read the source/rustdoc for the exact contract. |
| `parse_bind` | `private` | fn | [`crates/pcloud-web/src/main.rs:224`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs#L224) | Read the source/rustdoc for the exact contract. |
| `set_token_source` | `private` | fn | [`crates/pcloud-web/src/main.rs:230`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs#L230) | Read the source/rustdoc for the exact contract. |
| `push_allowed_host` | `private` | fn | [`crates/pcloud-web/src/main.rs:238`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs#L238) | Read the source/rustdoc for the exact contract. |
| `resolve_token` | `private` | fn | [`crates/pcloud-web/src/main.rs:247`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs#L247) | Read the source/rustdoc for the exact contract. |
| `default_socket_path` | `private` | fn | [`crates/pcloud-web/src/main.rs:263`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs#L263) | Read the source/rustdoc for the exact contract. |
| `non_empty_env_path` | `private` | fn | [`crates/pcloud-web/src/main.rs:284`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/main.rs#L284) | Read the source/rustdoc for the exact contract. |
| `CSP` | `private` | const | [`crates/pcloud-web/src/routes.rs:70`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L70) | Content-Security-Policy applied to every HTML response. `default-src 'self'; script-src 'none'; style-src 'se… |
| `CSRF_COOKIE` | `private` | const | [`crates/pcloud-web/src/routes.rs:73`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L73) | Cookie name for the double-submit CSRF token. |
| `WEB_SESSION_COOKIE` | `private` | const | [`crates/pcloud-web/src/routes.rs:75`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L75) | Cookie name for the browser session copy of the web token. |
| `CSRF_HEADER` | `private` | const | [`crates/pcloud-web/src/routes.rs:77`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L77) | Request header the caller must echo the cookie value into. |
| `WEB_TOKEN_HEADER` | `private` | const | [`crates/pcloud-web/src/routes.rs:80`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L80) | Request header that mutating routes require for session authentication. The value must match the token logged… |
| `router` | `pub(crate)` | fn | [`crates/pcloud-web/src/routes.rs:83`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L83) | Build the Axum router with the provided shared \[`AppState`\]. |
| `enforce_allowed_host` | `private` | fn | [`crates/pcloud-web/src/routes.rs:105`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L105) | Read the source/rustdoc for the exact contract. |
| `health` | `private` | fn | [`crates/pcloud-web/src/routes.rs:126`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L126) | `GET /health` — liveness probe. Never touches the daemon. Intentionally unauthenticated: orchestrators (Kuber… |
| `livez` | `private` | fn | [`crates/pcloud-web/src/routes.rs:139`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L139) | `GET /livez` — Kubernetes-style liveness probe. Returns 200 `"ok"` unconditionally: if the process is running… |
| `readyz` | `private` | fn | [`crates/pcloud-web/src/routes.rs:151`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L151) | `GET /readyz` — Kubernetes-style readiness probe. Returns 200 `"ok"` when the daemon has completed initializa… |
| `index` | `private` | fn | [`crates/pcloud-web/src/routes.rs:160`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L160) | `GET /` — HTML landing page + CSRF cookie issuance. |
| `api_status` | `private` | fn | [`crates/pcloud-web/src/routes.rs:176`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L176) | `GET /api/status` — JSON mirror of the landing page. Requires a valid `X-PCloud-Web-Token` header because thi… |
| `sync_list` | `private` | fn | [`crates/pcloud-web/src/routes.rs:196`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L196) | `GET /sync` — list sync roots + add form. |
| `SyncAddForm` | `private` | struct | [`crates/pcloud-web/src/routes.rs:225`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L225) | Form payload for `POST /sync`. |
| `sync_add` | `private` | fn | [`crates/pcloud-web/src/routes.rs:237`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L237) | `POST /sync` — add a sync root. Session token and CSRF required. |
| `sync_remove` | `private` | fn | [`crates/pcloud-web/src/routes.rs:272`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L272) | `DELETE /sync/{id}` — remove a sync root. Session token and CSRF required. |
| `publinks_list` | `private` | fn | [`crates/pcloud-web/src/routes.rs:295`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L295) | `GET /publinks` — list active public links + create form. |
| `PublinkCreateForm` | `private` | struct | [`crates/pcloud-web/src/routes.rs:314`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L314) | Form payload for `POST /publinks`. |
| `drop` | `private` | fn | [`crates/pcloud-web/src/routes.rs:331`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L331) | Read the source/rustdoc for the exact contract. |
| `publinks_create` | `private` | fn | [`crates/pcloud-web/src/routes.rs:338`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L338) | `POST /publinks` — create a public link. Session token and CSRF required. |
| `publinks_revoke` | `private` | fn | [`crates/pcloud-web/src/routes.rs:404`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L404) | `DELETE /publinks/{code}` — revoke a public link. Session token and CSRF required. |
| `activity` | `private` | fn | [`crates/pcloud-web/src/routes.rs:458`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L458) | `GET /activity` — last-100 audit events. Content type is negotiated: `Accept: application/json` (or `applicat… |
| `settings` | `private` | fn | [`crates/pcloud-web/src/routes.rs:494`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L494) | `GET /settings` — redacted config view. |
| `metrics` | `private` | fn | [`crates/pcloud-web/src/routes.rs:507`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L507) | `GET /metrics` — placeholder. The `metrics` feature is not compiled in for this crate; the route always retur… |
| `StatusSummary` | `pub(crate)` | struct | [`crates/pcloud-web/src/routes.rs:526`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L526) | Summary of the daemon's reported status, used by both the HTML and JSON renderers on `GET /`. |
| `fetch_status` | `private` | fn | [`crates/pcloud-web/src/routes.rs:539`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L539) | Read the source/rustdoc for the exact contract. |
| `parse_status` | `private` | fn | [`crates/pcloud-web/src/routes.rs:558`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L558) | Read the source/rustdoc for the exact contract. |
| `call_ipc` | `private` | fn | [`crates/pcloud-web/src/routes.rs:596`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L596) | Read the source/rustdoc for the exact contract. |
| `raw_and_online` | `private` | fn | [`crates/pcloud-web/src/routes.rs:608`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L608) | Read the source/rustdoc for the exact contract. |
| `ipc_redirect_response` | `private` | fn | [`crates/pcloud-web/src/routes.rs:615`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L615) | Read the source/rustdoc for the exact contract. |
| `ipc_plain_response` | `private` | fn | [`crates/pcloud-web/src/routes.rs:644`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L644) | Read the source/rustdoc for the exact contract. |
| `HostAuthority` | `private` | struct | [`crates/pcloud-web/src/routes.rs:681`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L681) | Read the source/rustdoc for the exact contract. |
| `require_allowed_host` | `private` | fn | [`crates/pcloud-web/src/routes.rs:687`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L687) | Read the source/rustdoc for the exact contract. |
| `require_same_origin` | `private` | fn | [`crates/pcloud-web/src/routes.rs:698`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L698) | Read the source/rustdoc for the exact contract. |
| `request_host` | `private` | fn | [`crates/pcloud-web/src/routes.rs:725`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L725) | Read the source/rustdoc for the exact contract. |
| `parse_host_authority` | `private` | fn | [`crates/pcloud-web/src/routes.rs:732`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L732) | Read the source/rustdoc for the exact contract. |
| `normalize_host` | `private` | fn | [`crates/pcloud-web/src/routes.rs:778`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L778) | Read the source/rustdoc for the exact contract. |
| `authority_is_allowed` | `private` | fn | [`crates/pcloud-web/src/routes.rs:782`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L782) | Read the source/rustdoc for the exact contract. |
| `is_loopback_host` | `private` | fn | [`crates/pcloud-web/src/routes.rs:797`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L797) | Read the source/rustdoc for the exact contract. |
| `port_matches_bind` | `private` | fn | [`crates/pcloud-web/src/routes.rs:804`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L804) | Read the source/rustdoc for the exact contract. |
| `origin_matches_host` | `private` | fn | [`crates/pcloud-web/src/routes.rs:808`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L808) | Read the source/rustdoc for the exact contract. |
| `origin_authority` | `private` | fn | [`crates/pcloud-web/src/routes.rs:818`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L818) | Read the source/rustdoc for the exact contract. |
| `effective_port` | `private` | fn | [`crates/pcloud-web/src/routes.rs:834`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L834) | Read the source/rustdoc for the exact contract. |
| `host_reject` | `private` | fn | [`crates/pcloud-web/src/routes.rs:842`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L842) | Read the source/rustdoc for the exact contract. |
| `origin_reject` | `private` | fn | [`crates/pcloud-web/src/routes.rs:854`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L854) | Read the source/rustdoc for the exact contract. |
| `existing_or_new_csrf` | `private` | fn | [`crates/pcloud-web/src/routes.rs:872`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L872) | Read the caller's existing CSRF cookie (if valid) or mint a fresh one. Tokens are 128 bits of OS randomness h… |
| `mint_csrf_token` | `private` | fn | [`crates/pcloud-web/src/routes.rs:881`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L881) | Read the source/rustdoc for the exact contract. |
| `is_valid_token` | `private` | fn | [`crates/pcloud-web/src/routes.rs:897`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L897) | Read the source/rustdoc for the exact contract. |
| `read_cookie` | `private` | fn | [`crates/pcloud-web/src/routes.rs:901`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L901) | Read the source/rustdoc for the exact contract. |
| `require_csrf` | `private` | fn | [`crates/pcloud-web/src/routes.rs:920`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L920) | Double-submit check: the `X-CSRF-Token` header or hidden form token MUST match the `pcw_csrf` cookie and MUST… |
| `csrf_reject` | `private` | fn | [`crates/pcloud-web/src/routes.rs:955`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L955) | Read the source/rustdoc for the exact contract. |
| `require_web_token` | `private` | fn | [`crates/pcloud-web/src/routes.rs:976`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L976) | Session-token gate for daemon-backed routes. Compares either the `X-PCloud-Web-Token` header value or the `pc… |
| `html_response_with_csrf` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1005`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1005) | Read the source/rustdoc for the exact contract. |
| `append_set_cookie` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1031`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1031) | Read the source/rustdoc for the exact contract. |
| `json_response` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1037`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1037) | Read the source/rustdoc for the exact contract. |
| `page_shell` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1053`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1053) | Read the source/rustdoc for the exact contract. |
| `render_sync_page` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1076`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1076) | Read the source/rustdoc for the exact contract. |
| `render_publinks_page` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1105`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1105) | Read the source/rustdoc for the exact contract. |
| `render_activity_page` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1126`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1126) | Read the source/rustdoc for the exact contract. |
| `redact_settings` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1141`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1141) | Redact secret-bearing keys from a settings view. The pcloud-web process holds no secrets itself — this is def… |
| `render_settings_page` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1153`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1153) | Read the source/rustdoc for the exact contract. |
| `_enum_type_parity` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1176`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1176) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-web/src/routes.rs:1179`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1179) | Read the source/rustdoc for the exact contract. |
| `minted_csrf_is_valid` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1190`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1190) | Read the source/rustdoc for the exact contract. |
| `malformed_csrf_rejected` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1196`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1196) | Read the source/rustdoc for the exact contract. |
| `route_helper_edge_matrix_covers_status_authority_and_csrf_shapes` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1203`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1203) | Read the source/rustdoc for the exact contract. |
| `redact_settings_hides_secrets` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1307`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1307) | Read the source/rustdoc for the exact contract. |
| `web_token_gate_rejects_missing_token` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1322`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1322) | Read the source/rustdoc for the exact contract. |
| `web_token_gate_rejects_wrong_token` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1328`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1328) | Read the source/rustdoc for the exact contract. |
| `web_token_gate_admits_correct_token` | `private` | fn | [`crates/pcloud-web/src/routes.rs:1335`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/routes.rs#L1335) | Read the source/rustdoc for the exact contract. |
| `escape` | `pub(crate)` | fn | [`crates/pcloud-web/src/templates.rs:16`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/templates.rs#L16) | Escape a string for safe interpolation into HTML text/attribute content. Minimal allow-list for the five XML… |
| `render_index` | `pub(crate)` | fn | [`crates/pcloud-web/src/templates.rs:32`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/templates.rs#L32) | Render the plain-HTML status page. |
| `tests` | `private` | mod | [`crates/pcloud-web/src/templates.rs:88`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/templates.rs#L88) | Read the source/rustdoc for the exact contract. |
| `escape_handles_entities` | `private` | fn | [`crates/pcloud-web/src/templates.rs:92`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/templates.rs#L92) | Read the source/rustdoc for the exact contract. |
| `render_offline_page_contains_expected_markers` | `private` | fn | [`crates/pcloud-web/src/templates.rs:98`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/templates.rs#L98) | Read the source/rustdoc for the exact contract. |
| `render_online_page_shows_counts` | `private` | fn | [`crates/pcloud-web/src/templates.rs:113`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/src/templates.rs#L113) | Read the source/rustdoc for the exact contract. |
| `binary` | `private` | fn | [`crates/pcloud-web/tests/binary_coverage.rs:10`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/binary_coverage.rs#L10) | Read the source/rustdoc for the exact contract. |
| `run` | `private` | fn | [`crates/pcloud-web/tests/binary_coverage.rs:14`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/binary_coverage.rs#L14) | Read the source/rustdoc for the exact contract. |
| `unused_addr` | `private` | fn | [`crates/pcloud-web/tests/binary_coverage.rs:23`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/binary_coverage.rs#L23) | Read the source/rustdoc for the exact contract. |
| `help_version_and_invalid_cli_inputs_exit_deterministically` | `private` | fn | [`crates/pcloud-web/tests/binary_coverage.rs:31`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/binary_coverage.rs#L31) | Read the source/rustdoc for the exact contract. |
| `serve_mode_binds_and_exposes_health_and_readiness` | `private` | fn | [`crates/pcloud-web/tests/binary_coverage.rs:89`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/binary_coverage.rs#L89) | Read the source/rustdoc for the exact contract. |
| `health_endpoint_returns_200_ok` | `private` | fn | [`crates/pcloud-web/tests/health.rs:16`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/health.rs#L16) | Read the source/rustdoc for the exact contract. |
| `index_sends_csp_and_reports_offline_without_socket` | `private` | fn | [`crates/pcloud-web/tests/health.rs:50`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/health.rs#L50) | Read the source/rustdoc for the exact contract. |
| `unused_addr` | `private` | fn | [`crates/pcloud-web/tests/serve_coverage.rs:9`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/serve_coverage.rs#L9) | Read the source/rustdoc for the exact contract. |
| `public_serve_path_writes_token_serves_and_reports_bind_conflicts` | `private` | fn | [`crates/pcloud-web/tests/serve_coverage.rs:17`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/serve_coverage.rs#L17) | Read the source/rustdoc for the exact contract. |
| `start` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:23`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L23) | Fire up the server, return (addr, web_token, join_handle). Handle is aborted by the caller at end of test. |
| `start_with_socket` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:38`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L38) | Read the source/rustdoc for the exact contract. |
| `raw_request` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:55`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L55) | Read the source/rustdoc for the exact contract. |
| `extract_cookie` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:65`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L65) | Extract a `Set-Cookie: name=value` cookie from a raw HTTP response. Returns the cookie value or panics with t… |
| `extract_csrf_cookie` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:81`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L81) | Read the source/rustdoc for the exact contract. |
| `successful_daemon_response` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:85`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L85) | Read the source/rustdoc for the exact contract. |
| `sync_list_renders_html_with_csrf_token` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:115`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L115) | Read the source/rustdoc for the exact contract. |
| `hostile_host_header_is_rejected` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:146`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L146) | Read the source/rustdoc for the exact contract. |
| `daemon_backed_get_routes_reject_missing_web_token` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:162`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L162) | Read the source/rustdoc for the exact contract. |
| `sync_add_rejects_request_without_web_token` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:185`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L185) | Read the source/rustdoc for the exact contract. |
| `sync_add_rejects_request_without_csrf` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:208`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L208) | Read the source/rustdoc for the exact contract. |
| `publink_create_then_delete_round_trip` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:235`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L235) | Read the source/rustdoc for the exact contract. |
| `cross_origin_mutation_is_rejected` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:310`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L310) | Read the source/rustdoc for the exact contract. |
| `browser_like_form_post_uses_hidden_csrf_and_session_cookie` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:346`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L346) | Read the source/rustdoc for the exact contract. |
| `activity_returns_json_when_accept_is_application_json` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:383`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L383) | Read the source/rustdoc for the exact contract. |
| `settings_redacts_secret_fields` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:422`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L422) | Read the source/rustdoc for the exact contract. |
| `online_daemon_routes_and_mutations_succeed_end_to_end` | `private` | fn | [`crates/pcloud-web/tests/ui.rs:447`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-web/tests/ui.rs#L447) | Read the source/rustdoc for the exact contract. |

## Usage guidance

This is product code but not a frozen external library contract. Check current status and native qualification before deployment claims.
