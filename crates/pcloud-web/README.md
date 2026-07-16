# pcloud-web

MVP Web UI scaffold for the `pcloud-rs` daemon, tracked under
PLAN_A_PLUS §P4.5. Gate **G7** expanded the surface from 3 routes
(`/`, `/api/status`, `/health`) to the 12 routes documented below,
covering sync-root, public-link, activity, settings, and metrics
views plus CRUD mutations for sync and publinks.

> **Caveat (honest).** The landing-page and `/api/status` payload
> parsing is **best-effort** until the daemon `GetStatus` JSON shape
> stabilises (tracked under `bd-1du.10`). Fields that fail to decode
> are rendered as `—` / omitted rather than 500ing. No JS framework
> is used; every page is server-rendered HTML.

## What this is

A single-user Axum HTTP server that binds to `127.0.0.1` by default and
exposes the routes listed below. Every page is server-rendered plain HTML —
no client-side JS (the CSP blocks it outright).

### Route map

| Method   | Path                  | Purpose                                           | CSRF |
| -------- | --------------------- | ------------------------------------------------- | ---- |
| `GET`    | `/`                   | Status landing page, issues CSRF cookie           | n/a  |
| `GET`    | `/api/status`         | JSON mirror of the landing page                   | n/a  |
| `GET`    | `/health`             | Liveness probe (no IPC)                           | n/a  |
| `GET`    | `/sync`               | List sync roots + pending ops + add form         | n/a  |
| `POST`   | `/sync`               | Add a sync root (form: local, remote, type)       | yes  |
| `DELETE` | `/sync/{id}`          | Remove a sync root                                | yes  |
| `GET`    | `/publinks`           | List active public links + create form            | n/a  |
| `POST`   | `/publinks`           | Create publink (path, expiry, password)           | yes  |
| `DELETE` | `/publinks/{code}`    | Revoke publink (accepts numeric id or code)       | yes  |
| `GET`    | `/activity`           | Last 100 audit events (HTML or JSON via `Accept`) | n/a  |
| `GET`    | `/settings`           | Read-only config view (secrets redacted)          | n/a  |
| `GET`    | `/metrics`            | 404 unless the `metrics` feature is enabled       | n/a  |

### CSRF — double-submit cookie

Every `GET` that renders HTML sets `pcw_csrf=<32 hex>; HttpOnly;
SameSite=Strict; Path=/` and renders the same value into hidden form
fields. Every mutating handler accepts either a matching
`X-CSRF-Token` request header or the hidden form value. The two values
are compared constant-time. Missing/malformed/mismatched tokens return
`403 Forbidden`.

Because the cookie is `HttpOnly; SameSite=Strict`, and mutations also
require same-origin `Origin` or `Referer`, browser form submissions work
without JavaScript or custom headers while cross-origin posts are rejected.

## What this is not

This crate is **not** the final Leptos SSR application described in
PLAN_A_PLUS §P4.5. It is the scaffold on which that work will be
built. There is no client-side JS (deliberately blocked by CSP), no
external auth surface, and no real templating yet.

## How to run

```bash
cargo run -p pcloud-web
cargo run -p pcloud-web -- --help
```

By default the server binds to `127.0.0.1:17650` and resolves the daemon
IPC socket from `PCLOUD_ROOT` or the XDG runtime/cache directories. Use
`--socket <PATH>` to override it, `--web-token-file <PATH>` to reuse an
existing token, and `--allow-host <HOST>` for a reverse proxy or LAN test
host.

For lab testing from another host, bind to all IPv4 interfaces and allow the
Host header your browser will send:

```bash
cargo run -p pcloud-web -- \
  --bind 0.0.0.0:17650 \
  --allow-host 192.0.2.10:17650
```

Do not use this as an enterprise auth boundary. The web/session token and
CSRF cookie are still the only HTTP-layer controls; production exposure should
go through a TLS/auth reverse proxy.

## Security posture

- Localhost by default; all-interface/LAN binds are allowed only when
  explicitly configured for testing or behind a controlled proxy.
- No CORS, same-origin only.
- Host allow-list enforcement: loopback/local `Host` values are accepted;
  additional reverse-proxy or LAN test hosts must be configured explicitly.
- Mutating routes require same-origin `Origin` or `Referer`.
- Every HTML response carries a minimal CSP:
  `default-src 'self'; script-src 'none'; style-src 'self' 'unsafe-inline'`.
- `X-Content-Type-Options: nosniff` on HTML responses.
- Daemon-backed routes require `X-PCloud-Web-Token` or the HttpOnly
  `pcw_session` cookie issued after a token-authenticated HTML GET.
- The web process runs as the same local user as the daemon; IPC
  permission enforcement lives in `pcloud-ipc`.
- All-interface binds such as `0.0.0.0:17650` are supported for testing,
  but remote browser hosts must be listed with `--allow-host`.

## Accessibility

The UI targets **WCAG 2.1 AA** from day one:

- All flows work with JavaScript disabled (CSP blocks scripts anyway).
- Full keyboard traversal; visible focus outlines preserved.
- Semantic HTML (`<nav>`, `<main>`, `<form>`, `<table>` with
  `<th scope>` headers); no `div`-only layouts.
- Form inputs carry explicit `<label for>` associations.
- Colour is never the sole signal; status uses text + iconography.
- See `docs/book/src/operations/web-ui.md` for the full checklist.

## Roadmap to Leptos SSR

1. Replace `templates.rs` string rendering with Leptos SSR components.
2. Pin a stable IPC status payload shape (coordinate with
   `bd-1du.10`) and drop the best-effort JSON parsing in `routes.rs`.
3. Add authenticated write actions (pause/resume sync, mount/unmount)
   gated by a local CSRF token bound to the IPC socket peer UID.
4. Harden the CSP (drop `'unsafe-inline'`, use hashed styles).
5. Ship a systemd user unit and a CLI flag on `pcloud-daemon` to
   launch the web UI alongside the daemon.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).
