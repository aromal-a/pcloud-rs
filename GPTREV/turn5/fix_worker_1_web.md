# Turn 5 Fix Worker 1 - pcloud-web

## Scope

Owned files only:

- `crates/pcloud-web/**`
- `README.md` pcloud-web references
- `docs/book/src/operations/web-ui.md`
- `GPTREV/turn5/fix_worker_1_web.md`

## Fixes

- Added a runnable `pcloud-web` binary with manual argument parsing:
  `--bind`, `--socket`, `--web-token`, `--web-token-file`,
  `--allow-host`, `--ready`, `--not-ready`, `--help`, and `--version`.
  The binary resolves the default daemon socket from `PCLOUD_ROOT` or XDG,
  validates loopback bind addresses, builds `WebConfig`, and calls
  `pcloud_web::serve`.
- Added Host allow-list enforcement for all routes. Loopback/local hosts
  are accepted by default; additional reverse-proxy hosts are configured
  through `WebConfig::allowed_hosts` / `--allow-host`.
- Added mutating-route `Origin` / `Referer` enforcement. `POST` and
  `DELETE` routes reject missing or cross-origin origin metadata.
- Made rendered POST forms submit without custom JavaScript headers:
  token-authenticated HTML GETs set an HttpOnly `pcw_session` cookie, forms
  render hidden `csrf_token` fields, and mutating handlers accept either
  `X-CSRF-Token` or that hidden form field.
- Added integration coverage for hostile `Host`, cross-origin mutation,
  and browser-like form POST using cookies + hidden CSRF with no custom
  web-token or CSRF headers.
- Updated pcloud-web README and operator docs for the binary, token/session
  cookie, Host/Origin enforcement, and browser-like CSRF flow.

## Verification

- `cargo test -p pcloud-web --tests` - passed
- `cargo run -p pcloud-web -- --help` - passed

Initial parallel cargo runs failed once on the pre-fix Origin URI borrow
error; reruns after the fix passed.
