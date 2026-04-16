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

A single-user Axum HTTP server that binds to `127.0.0.1` and exposes
the routes listed below. Every page is server-rendered plain HTML —
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
SameSite=Strict; Path=/`. Every mutating handler requires the caller
to echo that value in the `X-CSRF-Token` request header. The two
values are compared constant-time. Missing/malformed/mismatched tokens
return `403 Forbidden`.

Because the cookie is `HttpOnly; SameSite=Strict` only same-origin
(loopback-only) callers can observe and re-submit it.

## What this is not

This crate is **not** the final Leptos SSR application described in
PLAN_A_PLUS §P4.5. It is the scaffold on which that work will be
built. There is no client-side JS (deliberately blocked by CSP), no
auth surface, no write actions, and no real templating yet.

## How to run

```bash
cargo run -p pcloud-web
```

By default the server binds to `127.0.0.1:17650`. The daemon IPC
socket path is configured via `WebConfig::socket_path` — callers that
embed `pcloud-web` are expected to reuse the daemon's configured
runtime-dir socket.

Embedders may override the bind address via a `--bind` CLI flag (or
the `WebConfig::bind_addr` field). **Overriding to a non-loopback
address is refused with a startup panic.** Do not attempt to expose
this surface on a LAN or public IP: it is unauthenticated beyond the
CSRF cookie, and the CSRF cookie is a same-origin control only —
not an auth layer. If you must reach the UI from a different host,
use an SSH port-forward (`ssh -L 17650:127.0.0.1:17650 host`).

## Security posture

- Localhost-only. Attempting to bind to any non-loopback address
  panics at startup.
- No CORS, same-origin only.
- Every HTML response carries a minimal CSP:
  `default-src 'self'; script-src 'none'; style-src 'self' 'unsafe-inline'`.
- `X-Content-Type-Options: nosniff` on HTML responses.
- The web process runs as the same local user as the daemon; IPC
  permission enforcement lives in `pcloud-ipc`.
- **Loopback-only panic guard**: `WebConfig::bind_addr` is validated
  at startup; any non-loopback address (including `0.0.0.0`, `::`,
  LAN IPs, or public IPs) triggers an explicit panic before the
  listener is created. See ADR 0004.

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
