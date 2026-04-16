# Web UI

## 1. Purpose

The `pcloud-web` crate ships a server-rendered HTML administration
surface for the `pcloud-rs` daemon: the operator's console for
inspecting sync roots, reviewing public links, auditing recent
activity, and reading the live configuration — all without leaving
the terminal host. This page covers the **G7 expansion** (3 → 12
routes, verified in `crates/pcloud-web/src/routes.rs`), how to start
the server, its security posture, per-page mockups, reverse-proxy
recipes, and the accessibility bar the UI is held to.

> **Stability caveat — pre-alpha, not final UI.** Status payload
> parsing is **best-effort** until the daemon `GetStatus` JSON shape
> stabilises under `bd-1du.10`. Fields that fail to decode render
> as `—`; the UI never 500s on schema drift. There is **no
> JavaScript** — the CSP blocks it outright and no flow requires it.
> Layout, copy, and route set are expected to change before any
> GA claim.

## 2. Prereqs

- `pcloud-daemon` running as the same UID that will run
  `pcloud-web`; the UI connects over the daemon’s Unix-domain IPC
  socket.
- A loopback-reachable TCP port (default `17650`).
- For multi-operator exposure: a same-host reverse proxy
  (nginx / Caddy) terminating TLS + authentication.
- (Optional) Prometheus scrape agent if the `metrics` feature is
  enabled.

## 3. Conceptual background

### What the UI is (and is not)

- It **is** a thin HTML viewer / mutator for the daemon’s IPC API.
- It **is not** an auth boundary. The CSRF cookie is a same-origin
  control, not an authentication layer. Anyone with `curl` on the
  host and the right `Cookie:` header is in. Do not expose the UI
  beyond the local host.
- It **is not** a JavaScript SPA. The CSP’s `script-src 'none'` is
  enforced; every flow works without JS. Forms are standard HTML
  `POST`/`DELETE` via link-with-method.

### Security posture (summary)

1. **Loopback-only.** Non-loopback bind panics before the listener
   is created (ADR 0004 panic guard). There is no flag / env var /
   config key that overrides the guard. It is a hard invariant.
2. **CSP.** Every HTML response carries
   `default-src 'self'; script-src 'none'; style-src 'self'
   'unsafe-inline'` plus `X-Content-Type-Options: nosniff`.
3. **CSRF — double-submit cookie.** Every `GET` that renders HTML
   sets `pcw_csrf=<32 hex>; HttpOnly; SameSite=Strict; Path=/`.
   Every mutating (`POST` / `DELETE`) handler requires the caller
   to echo that value in the `X-CSRF-Token` request header. The
   compare is constant-time; missing / malformed / mismatched
   returns `403 Forbidden`.
4. **No client-side JS.** CSP `script-src 'none'` is not decorative.
5. **No authentication surface.** Host UID boundary is the trust
   boundary.
6. **Secrets redaction.** `/settings` prints paths, modes, and
   booleans — never the token, password, or API keys.

## 4. Step-by-step procedure

### 4.1 Start the server

```bash
# Development
cargo run -p pcloud-web
```

Default bind: `127.0.0.1:17650`. Daemon socket auto-discovered from
the shared runtime directory.

Release builds:

```bash
pcloud-web --bind 127.0.0.1:17650 \
           --socket /run/user/1000/pcloudd.sock
```

The bind address is validated at start-up. **Any non-loopback address
triggers an explicit panic** — this is the loopback-only panic guard.

### 4.2 Reach the UI from a different host

Use an SSH local-forward; never expose loopback directly:

```bash
ssh -L 17650:127.0.0.1:17650 operator@pcloud-host.internal
# Then visit http://127.0.0.1:17650 on your workstation.
```

### 4.3 Route map (verified against `routes.rs`)

| Method   | Path                | Page / purpose                             | CSRF |
| -------- | ------------------- | ------------------------------------------ | ---- |
| `GET`    | `/`                 | Landing / status dashboard                 | –    |
| `GET`    | `/api/status`       | JSON mirror of the landing page            | –    |
| `GET`    | `/health`           | Liveness probe (no IPC)                    | –    |
| `GET`    | `/sync`             | List sync roots + pending ops + add form   | –    |
| `POST`   | `/sync`             | Add sync root                              | yes  |
| `DELETE` | `/sync/{id}`        | Remove sync root                           | yes  |
| `GET`    | `/publinks`         | List active public links + create form     | –    |
| `POST`   | `/publinks`         | Create public link                         | yes  |
| `DELETE` | `/publinks/{code}`  | Revoke public link                         | yes  |
| `GET`    | `/activity`         | Last 100 audit events (HTML or JSON)       | –    |
| `GET`    | `/settings`         | Read-only config view (secrets redacted)   | –    |
| `GET`    | `/metrics`          | Prometheus text format (feature-gated)     | –    |

Verified source: `crates/pcloud-web/src/routes.rs` lines 68-77
(`route("/") … route("/metrics")`).

### 4.4 Startup options

```text
pcloud-web
  --bind <ADDR>        loopback address:port (default 127.0.0.1:17650)
  --socket <PATH>      daemon IPC socket (default auto-discovered)
  --features metrics   enable the /metrics route (build-time)
```

Non-loopback `--bind` values panic before the listener is created.

## 5. Verification

### 5.1 Smoke test (human)

```bash
curl -s http://127.0.0.1:17650/health           # 200 OK
curl -s http://127.0.0.1:17650/api/status       # JSON body
curl -sI http://127.0.0.1:17650/ | grep -i csp  # header present
```

### 5.2 Route exercise (machine)

```bash
for p in / /api/status /health /sync /publinks /activity /settings; do
  curl -s -o /dev/null -w "%{http_code} %{url_effective}\n" \
    http://127.0.0.1:17650$p
done
# expect: 200 / 200 … (except /metrics which returns 404 when the
# feature is disabled)
```

### 5.3 CSRF round-trip

```bash
# Fetch a GET to obtain the CSRF cookie:
curl -s -c /tmp/cookies.txt -o /dev/null http://127.0.0.1:17650/sync
TOK=$(awk '$6=="pcw_csrf"{print $7}' /tmp/cookies.txt)

# A mutation WITHOUT the header should return 403:
curl -s -o /dev/null -w "%{http_code}\n" -X DELETE \
  -b /tmp/cookies.txt \
  http://127.0.0.1:17650/sync/1
# → 403

# With the header it is accepted:
curl -s -o /dev/null -w "%{http_code}\n" -X DELETE \
  -b /tmp/cookies.txt -H "X-CSRF-Token: $TOK" \
  http://127.0.0.1:17650/sync/1
# → 200 or 404 (depending on whether id=1 exists)
```

## 6. Rollback

The UI is stateless — rollback is trivial:

- **Stop the web server** (no persistent state to revert).
- **If the daemon was modified** in the same change, follow the
  daemon rollback procedure in [Upgrade](./upgrade.md#6-rollback).
- **If a reverse-proxy config change caused outage**: revert the
  proxy configuration and `systemctl reload` the proxy; the
  upstream daemon/UI pair is untouched.

## 7. Tradeoffs / tuning

| Knob                                 | Default            | Tradeoff                                                      |
|--------------------------------------|--------------------|---------------------------------------------------------------|
| `--bind`                             | `127.0.0.1:17650`  | Any non-loopback value panics — this is by design.            |
| `metrics` feature                    | off                | On → exposes `/metrics` Prometheus; more cardinality cost.    |
| CSP `style-src 'unsafe-inline'`      | on                 | Enables the default stylesheet without external files.        |
| Reverse-proxy auth broker            | off                | OIDC/mTLS sidecar removes single-operator limit; costs setup. |
| HA (two daemon/web pairs behind VIP) | off                | Only one daemon is writable (single-writer IPC socket).       |

## 8. Common failure modes

1. **Panic on start with "bind address not loopback".**
   - Cause: `--bind 0.0.0.0:17650` or an IPv6 public address.
   - Fix: bind to `127.0.0.1` or `[::1]`; multi-host access goes
     through a reverse proxy.
2. **`/metrics` returns `404 Not Found`.**
   - Cause: binary built without `--features metrics`.
   - Fix: rebuild with the feature; confirm via
     `pcloudc doctor --json | jq '.build.features'`.
3. **`403 Forbidden` on all form submissions.**
   - Cause: missing `X-CSRF-Token` header (common with tools that
     drop non-standard headers on redirect).
   - Fix: ensure the HTTP client preserves the header across the
     `302` redirect the UI emits after successful mutation.
4. **Fields rendering as `—`.**
   - Cause: daemon `GetStatus` JSON shape changed and the UI has
     not yet been updated.
   - Fix: file against `bd-1du.10`; the UI is explicitly best-effort
     until that bead closes.
5. **Reverse-proxy strips `SameSite=Strict` cookie.**
   - Cause: proxy cookie rewrite configured incorrectly (dropping
     `SameSite`).
   - Fix: the nginx recipe below uses `proxy_cookie_path` to
     preserve the attributes; do the same on Caddy via
     `header_up Cookie` + `header_down Set-Cookie`.

## 9. Security / compliance notes

- **Loopback guard is a hard invariant.** Never carve around it
  with an SSH-tunnel daemon, a `socat` listener, or a reverse-proxy
  co-hosted on a different machine. The threat model assumes the
  proxy is same-host.
- **CSRF cookie is `HttpOnly; SameSite=Strict`.** Preserve those
  attributes end-to-end through the proxy.
- **`/settings` redaction is the last line of defence**, not the
  first. The daemon IPC already refuses to return secret material;
  the UI’s redaction prevents accidental leaks if that contract
  regresses.
- **Audit events displayed in `/activity`** are a mirror of the
  append-only audit chain; do not let the UI become a mutation
  path for audit history.
- **Accessibility is tested.** `tests/a11y.rs` runs `axe-core`
  against rendered HTML; a failing a11y test blocks a merge.

## 10. Per-page mockups

### `/` — Status dashboard

```
+------------------------------------------------------------+
| pcloud-rs | Status  Sync  Publinks  Activity  Settings      |
+------------------------------------------------------------+
| Daemon:        RUNNING  (pid 18423, uptime 4h 12m)         |
| Connected:     yes       API: eapi.pcloud.com              |
| Account:       alice@example.com   Plan: Premium           |
| Quota:         412 GiB / 2 TiB   [==========--------] 20%  |
|                                                            |
| Sync roots: 3     In-flight ops: 12     Errors (1h): 0     |
+------------------------------------------------------------+
```

Any unparseable field renders as `—` rather than 500ing.

### `/sync` — Sync roots

```
ID  Local path              Remote             Type      Status
--- ----------------------- ------------------ --------- -------
 1  /home/alice/pcloud      /                  Two-way   active
 2  /home/alice/Pictures    /Pictures          Upload    active
 3  /srv/backup             /backups/srv       Download  paused

[ Add sync root ]
 Local:  [____________________]
 Remote: [____________________]
 Type:   ( ) Two-way  ( ) Upload  ( ) Download
 [Submit]
```

### `/publinks` — Public links

```
Code       Path                        Expires     Uploads  Actions
---------- --------------------------- ----------- -------- --------
XYZ1abc    /Shared/report.pdf          2026-05-01  off      [revoke]
AB2def3    /Public/screenshots/        never       on       [revoke]

[ Create public link ]
 Path:     [____________________]
 Expires:  [YYYY-MM-DD]
 Password: [____________________]  (optional)
 [Submit]
```

### `/activity` — Audit events

Accepts `Accept: application/json` for a machine-readable mirror.

```
Time                  Actor   Event              Detail
--------------------- ------- ------------------ ----------------------
2026-04-15T08:12:04Z  local   sync.root.added    id=3 local=/srv/backup
2026-04-15T08:11:22Z  local   publink.created    code=AB2def3
2026-04-15T08:04:51Z  daemon  auth.token.refresh uid=alice
```

### `/settings` — Read-only config

```
Runtime dir:       /run/user/1000/pcloud-rs
IPC socket:        /run/user/1000/pcloud-rs/daemon.sock  (0600)
Vault path:        ~/.config/pcloud-rs/vault.bin        (0600)
Token persisted:   yes
Crypto unlocked:   yes
API server:        eapi.pcloud.com (prod, TLS enforced)
Feature flags:     metrics=on, fuse=off
```

Secrets are never rendered; only paths, modes, and booleans.

### `/metrics` — Prometheus

Feature-gated behind `--features metrics`. Returns `404 Not Found`
when the feature is disabled. When enabled, emits the standard
`pcloud_*` counters and histograms (see
[Prometheus reference](../reference/metrics.md)).

## 11. Reverse-proxy recipes

The loopback guard means the UI cannot be exposed directly. For
multi-operator teams the B6 high-availability design recommends a
**reverse proxy on the same host**, terminated by an authenticating
sidecar (OIDC, mTLS, or HTTP Basic over TLS), then forwarded to
`127.0.0.1:17650`.

### nginx

```nginx
# /etc/nginx/conf.d/pcloud-rs-web.conf
server {
    listen 443 ssl http2;
    server_name pcloud-admin.internal;

    ssl_certificate     /etc/ssl/certs/pcloud-admin.pem;
    ssl_certificate_key /etc/ssl/private/pcloud-admin.key;

    # Authenticate before forwarding.
    auth_request        /_oidc_verify;

    location / {
        proxy_pass         http://127.0.0.1:17650;
        proxy_http_version 1.1;
        proxy_set_header   Host $host;
        proxy_set_header   X-Forwarded-For $remote_addr;
        # CSRF cookie is SameSite=Strict — proxy must preserve it.
        proxy_pass_request_headers on;
        proxy_cookie_path  / "/; Secure; SameSite=Strict";
    }
}
```

### Caddy

```caddyfile
pcloud-admin.internal {
    tls /etc/ssl/certs/pcloud-admin.pem /etc/ssl/private/pcloud-admin.key

    forward_auth https://oidc-broker.internal/verify {
        uri /userinfo
        copy_headers X-Forwarded-User X-Forwarded-Groups
    }

    reverse_proxy http://127.0.0.1:17650 {
        header_up Host {host}
        header_down Set-Cookie (.*SameSite=)[^;]+ "${1}Strict"
    }
}
```

### OIDC broker integration (sketch)

The proxy (nginx `auth_request` or Caddy `forward_auth`) validates
the request against an OIDC broker (e.g. `oauth2-proxy`, `vouch`,
`authelia`) before forwarding to `127.0.0.1:17650`. Operator identity
is surfaced to the daemon-internal audit via `X-Forwarded-User`;
the UI itself remains unauthenticated (trust is carried by the
proxy).

### Rules (don't break these)

- The proxy **must** be co-located with the daemon. Cross-host
  proxying defeats the loopback guard's threat model.
- The proxy **must** terminate TLS; upstream is plain HTTP on
  loopback by design.
- The upstream `SameSite=Strict` cookie survives because the proxy
  is same-site from the browser's perspective.
- HA: run two daemon/web pairs behind a floating VIP. **Only one
  is writable at a time** (the daemon IPC socket is single-writer).

## 12. Accessibility (WCAG 2.1 AA, day one)

- **Keyboard traversal.** Every interactive element is reachable
  via `Tab` / `Shift+Tab`. Focus outlines preserved (never
  `outline: none`). Forms submit with `Enter`.
- **No JS-required flows.** CSP blocks scripts; every action is a
  standard HTML form `POST` or link.
- **Semantic HTML.** `<nav>`, `<main>`, `<form>`, `<table>` with
  `<th scope="col">`, `<label for>` on every input.
- **Colour-independent signals.** Status is text
  (`active`, `paused`) plus iconography — never colour alone.
- **Contrast.** Default stylesheet targets **4.5:1** body text,
  **3:1** large text; dark-mode stylesheet mirrors the same ratios.
- **Screen-reader labels.** Buttons carry explicit verbs
  (`Revoke link XYZ1abc`), never unlabeled icons.
- **No timeouts.** Operators can leave a page open indefinitely;
  idempotent reloads are safe.

Accessibility is enforced by `axe-core` against rendered HTML in
`pcloud-web`'s integration tests (see `tests/a11y.rs`). A failing
a11y assertion blocks merge.

## 13. Cross-references

- ADR 0004 — Panic Guard Default-On.
- `crates/pcloud-web/src/routes.rs` — authoritative route list.
- `crates/pcloud-web/README.md` — crate-level reference.
- `docs/book/src/security/model.md` — threat model and boundaries.
- `RUST-PLANS/` — PLAN_A_PLUS §P4.5 roadmap to Leptos SSR.
- [Deployment](./deployment.md) — reverse-proxy topology.
- [Runbook](./runbook.md) — IPC triage playbooks.
- [Upgrade](./upgrade.md) — rolling the UI together with the
  daemon.
