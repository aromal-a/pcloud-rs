> **Pre-alpha scaffold — not live / not production-verified.** This document
> describes design and unit-tested code that has not been validated against a
> real production deployment. Do not treat it as a shippable capability.
> See `CLAUDE.md` and `docs/enterprise/README.md` for the honesty rules.

# Enterprise OIDC Identity Broker — Landed (pluggable exchanger)

Status: **Landed (unit-tested) with pluggable trusted-issuer exchanger.**
Crate `pcloud-idp` ships the `IdpBroker` trait, a concrete
`OidcAuthorizationCodeBroker` implementor, and a pluggable
`PcloudTokenExchanger` trait for the pCloud-side half of the handshake.
The trait is object-safe (held behind `Arc<dyn IdpBroker>` in the daemon
runtime plugin registry). Live pCloud trusted-issuer interop is **not**
claimed — pCloud's public API does not document a trusted-issuer
exchange endpoint. See §10 for the honest caveat.

Previously: the three `IdpBroker` methods on `UnimplementedBroker`
panicked with `unimplemented!()`. They now return
`IdpError::NotConfigured(&'static str)`, so the daemon surfaces a typed,
operator-actionable error instead of aborting.

## 0. What actually landed

The Rust broker is a **hand-rolled** OIDC Authorization Code + PKCE
implementor. We deliberately did **not** take on the `openidconnect`
crate dependency; the audit surface of that crate pulls in transitive
crypto we did not want to vendor, and we needed tight control over
JWKS caching and `alg=none` rejection semantics. The live code:

- `OidcAuthorizationCodeBroker` in `crates/pcloud-idp/src/oidc.rs`
  drives discovery, PKCE, the loopback callback, token exchange against
  the IdP, and ID-token verification.
- PKCE uses **S256 only**; `plain` is rejected at config parse and at
  runtime. The `code_verifier` is 256 bits of OS CSPRNG, held in
  `SecretString` for its lifetime and consumed by `complete_authorization`.
- JWKS verification accepts **RS256 only**. `alg=none`,
  symmetric-family algorithms (`HS*`), and any unlisted algorithm are
  rejected **before** signature verification runs — this is the
  canonical OIDC `alg=none` and algorithm-confusion defence.
- JWKS is cached in-memory with a **1-hour TTL**; expiry forces a
  rediscovery round-trip. Trust-on-first-use pinning behaviour from §4
  is enforced on top of the TTL cache.
- Every token (`id_token`, `access_token`, `refresh_token`,
  `code_verifier`) is wrapped in `pcloud_secret::SecretString` and
  never logged. Audit records carry `iss`, `sub`, `exp` only.
- The broker is fully exercised by offline unit tests against a stub
  IdP (discovery document, JWKS, token endpoint) — no network calls in
  CI. Tests cover: PKCE round-trip, `alg=none` rejection, RS256
  signature validation, JWKS cache TTL expiry, `nonce` mismatch,
  `state` mismatch, and refresh rejection surfacing
  `IdpError::RefreshRejected`.

## 1. Problem

pCloud's native authentication is a username + password flow, optionally
augmented with TFA. That shape is incompatible with how enterprises gate
access to third-party SaaS tools. Every non-trivial enterprise deployment we
have seen at pCloud-compatible scale imposes at least one of:

- **No third-party password logins.** Users do not have a usable pCloud
  password at all; their identity is held in an IdP (Okta, Azure AD, Ping,
  Google Workspace, Keycloak, ADFS, Duo SSO, OneLogin, JumpCloud).
- **Federation mandated via SAML 2.0 or OIDC.** SSO is a compliance control:
  SOC 2 CC6.1, ISO 27001 A.9, and every large-customer security
  questionnaire asks whether the tool supports SAML or OIDC federation.
- **Conditional access.** Access depends on device posture, IP range,
  geolocation, MFA freshness, and sometimes hardware-bound WebAuthn. These
  checks are enforced inside the IdP's authorization UI, not at the SaaS.
- **Directory-backed lifecycle.** Account provisioning and deprovisioning
  flow from the IdP via SCIM or LDAP. A terminated user must lose SaaS
  access the moment the IdP revokes their session, without the SaaS holding
  a long-lived independent password.
- **Kerberos / LDAP legacy.** A nontrivial minority of on-prem shops still
  front-end AD directly. They typically run a federation gateway (ADFS,
  Keycloak with an LDAP user federation, or Azure AD Connect) that exposes
  OIDC to modern clients. A minority still expose only LDAP.

pCloud's `pcloud-rs` today offers none of that. This design introduces a
broker layer that makes pCloud usable inside these environments without
asking pCloud engineering to reimplement every federation protocol inside
the pCloud session endpoint.

## 2. Architecture

The broker lives on the client side, inside `pcloudc` / `pcloudcd`, and
brokers trust between two independent systems:

```
 ┌─────────────┐       ┌───────────────┐        ┌────────────────┐
 │ Enterprise  │  (1)  │  pcloud-idp   │  (2)   │  pCloud API    │
 │    IdP      │──────▶│    broker     │───────▶│ trusted-issuer │
 │ (Okta/AAD)  │◀──JWT │  (pcloud-cli) │◀──tok──│   exchange     │
 └─────────────┘       └───────────────┘        └────────────────┘
```

1. The user runs:

   ```
   pcloudc login --idp=oidc --issuer=https://corp.okta.com
   ```

   The CLI resolves the `[auth.idp]` section of `config.toml`, constructs an
   `IdpConfig`, and hands it to an `IdpBroker` implementation.
2. The broker fetches `https://corp.okta.com/.well-known/openid-configuration`,
   pins the JWKS, generates PKCE verifier / `state` / `nonce`, and opens the
   system browser on the authorization URL.
3. The user authenticates at their IdP (password, WebAuthn, push,
   conditional-access prompts). The IdP redirects back to a loopback URI
   handled by the broker.
4. The broker exchanges the authorization code for an ID token (JWT) +
   refresh token, verifies the signature against the pinned JWKS, and checks
   `iss`, `aud`, `exp`, and `nonce`.
5. The broker POSTs the ID token to pCloud's **trusted-issuer exchange
   endpoint** — this endpoint is a stub today; pCloud support will need to
   enable it. The endpoint returns a normal `auth` token that the rest of
   `pcloud-daemon` uses exactly as today.
6. The daemon persists only the short-lived derivative pCloud token
   (following existing `auth_vault` rules: opt-in, 0600, 0700 parent dir).
   The IdP refresh token is stored alongside, secret-wrapped.

Crucially, pCloud never sees the enterprise password or any long-lived
credential beyond the short-lived ID token. The broker is the only layer
that speaks OIDC.

## 3. Supported Flows

The crate enumerates four flows (`IdpFlow`). Only the first lands as a
first-class implementation initially; the others are staged.

### 3.1 OIDC Authorization Code + PKCE (primary)

RFC 6749 §4.1 + RFC 7636. Public-client flow — no client secret is ever
stored on disk. The broker:

- generates a 256-bit `code_verifier` from the OS CSPRNG,
- derives `code_challenge = base64url(sha256(verifier))`,
- advertises `code_challenge_method=S256`,
- binds the callback to a loopback redirect URI (`http://127.0.0.1:<port>`)
  per RFC 8252, using an ephemeral port picked at runtime.

PKCE closes the interception attack on public clients. S256 is mandatory;
plain is rejected.

### 3.2 Device Authorization Grant (headless hosts)

RFC 8628. When `--idp=oidc --flow=device`, the broker requests a device
code from the IdP, prints a short URL and user code, and polls the token
endpoint until the user authorizes on a second device. This is how
`pcloudc` will log in from CI agents, servers, and SSH sessions without a
browser. The polling cadence respects `interval` and honors `slow_down`.

### 3.3 SAML 2.0 (via bridge)

Direct SAML is explicitly out of scope for `pcloud-idp`. Rationale:

- SAML is XML + XMLDSig + XML-ENC; the Rust ecosystem has no hardened
  implementation we can audit against XSW attacks.
- Every major IdP already exposes an OIDC façade; deployments that
  nominally mandate SAML for browser SSO can enable OIDC for desktop
  clients as a separate app registration. Okta, Azure AD, Ping, and ADFS
  all support this.
- For true SAML-only shops, we recommend Keycloak or Authentik as a
  SAML-to-OIDC bridge. This is a common enterprise pattern and moves the
  SAML attack surface out of `pcloud-rs`.

The `IdpFlow::SamlBridge` variant exists so the operator config surface can
document this choice; the broker implementation in that branch simply
delegates to the OIDC flow against the bridge's issuer URL.

### 3.4 LDAP (legacy seeding only)

The LDAP flow is intentionally restricted. A simple bind against
`ldap://ds.corp` is used **once**, only to seed a local enrollment record,
and only when explicitly configured. After first login, the broker migrates
the user to an OIDC enrollment against the configured issuer. This avoids
holding LDAP credentials on the client and keeps the long-term code path on
OIDC. Simple bind over cleartext LDAP is rejected: LDAPS or STARTTLS is
mandatory.

## 4. Trust & JWKS

ID tokens are validated against the IdP's JWKS:

- discovery: `GET {issuer}/.well-known/openid-configuration`,
- JWKS fetched from `jwks_uri` in the discovery document,
- cached on disk with the issuer hostname, keyed by `kid`.

### Pinning strategy

Three modes, operator-selectable in `[auth.idp]`:

1. **`pin_mode = "tofu"` (default).** Trust-on-first-use: the first JWKS
   fetch is pinned by SHA-256 of the JWK set. Subsequent rotations are
   accepted only if a new key appears while at least one pinned key is
   still present — this implements the standard "overlap" rotation pattern
   that all major IdPs use.
2. **`pin_mode = "static"`.** Operator-supplied JWKS fingerprint(s). No
   auto-rotation; changes require a config push. Appropriate for
   high-assurance deployments.
3. **`pin_mode = "ca"`.** The IdP's TLS certificate is pinned to a specific
   CA bundle. JWKS trust piggybacks on TLS. This is the weakest mode and is
   documented as such.

No mode accepts system trust store alone. Public CA compromise is not an
acceptable failure mode for an enterprise identity bridge.

## 5. Token Lifecycle

- **IdP ID token:** typically 5–60 minutes. Used only to mint a pCloud
  session, then kept for re-exchange if the pCloud session is invalidated.
- **IdP refresh token:** hours to days, per IdP policy. Stored
  `SecretString`-wrapped.
- **pCloud session token:** the existing short-lived derivative. The daemon
  handles it via the existing `auth_backend` and `auth_vault`.

Refresh flow:

1. Daemon notices the pCloud session is about to expire.
2. Daemon calls `IdpBroker::refresh(&token)`.
3. Broker POSTs `grant_type=refresh_token` to the IdP.
4. On success, new ID token is exchanged for a new pCloud session.
5. On `IdpError::RefreshRejected`, the daemon triggers interactive
   re-auth — no silent fallback to cached credentials.

## 6. Failure Modes

- **IdP unreachable during initial login.** Hard failure. Login cannot
  proceed offline.
- **IdP unreachable during refresh.** Soft failure: the cached pCloud
  session continues to be used until its own expiry, up to a configurable
  `cache_ttl` (default 24h). After that, interactive re-auth is forced.
  This matches enterprise expectations: a flaky IdP must not lock the
  entire workforce out of their files mid-workday, but it must not permit
  access beyond a short grace period either.
- **Refresh rejected.** Treated as session revocation. The broker surfaces
  `IdpError::RefreshRejected`; the daemon wipes the cached session and
  prompts `pcloudc` to re-run `login`.
- **Signature validation failure.** Hard error. The cached JWKS is
  invalidated, forcing a rediscovery on next login.
- **State/nonce mismatch on callback.** Hard error; the challenge is
  dropped. No retry with reused PKCE material.

## 7. Security

- The broker **never** sees the user's IdP password. The password is
  entered in the browser-hosted IdP UI.
- `id_token` and `refresh_token` are wrapped in
  [`pcloud_secret::SecretString`]: zeroized on `Drop`, redacted in `Debug`.
- PKCE `code_verifier` is held in `SecretString` for its lifetime and
  consumed by `complete_authorization`, which drops it before returning.
- No secret is logged, printed, or included in audit events. Audit events
  log *issuer, subject (`sub`), and token expiry only*.
- The broker forbids password grant (ROPC). There is no surface in the
  `IdpBroker` trait to pass a password. This is an intentional
  API-level enforcement.
- Transport is TLS-only in production builds. The existing production
  transport policy applies: no plaintext downgrade, no endpoint override
  without validation.

## 8. Operator Config

Example `config.toml`:

```toml
[auth.idp]
enabled     = true
issuer      = "https://corp.okta.com"
client_id   = "0oa1example"
flow        = "oidc-authorization-code"  # or "device-code", "saml-bridge", "ldap"
scopes      = ["openid", "email", "profile", "groups"]
pin_mode    = "tofu"                     # or "static", "ca"
cache_ttl   = "24h"
loopback_ports = [47000, 47001, 47002]   # first available is used
```

Validation rules:

- `issuer` must be `https://`.
- `scopes` must contain `openid`.
- `client_id` must be non-empty.
- `flow = "ldap"` requires `ldap_url = "ldaps://..."`; `ldap://` is rejected.

## 9. CLI UX

```
pcloudc login --idp=oidc --issuer=https://corp.okta.com
pcloudc auth idp-list
pcloudc auth idp-refresh
pcloudc auth idp-logout
```

- `login --idp=oidc` opens the system browser and blocks until the loopback
  callback fires or the user hits Ctrl+C.
- `auth idp-list` prints configured IdPs (issuer, client_id, flow,
  `cache_ttl`). No tokens are printed.
- `auth idp-refresh` forces a refresh round-trip. Exits non-zero if the
  refresh is rejected.
- `auth idp-logout` wipes the cached pCloud session and the IdP refresh
  token. Does not call the IdP's end-session endpoint unless
  `--revoke-at-idp` is passed, because not every IdP supports it.

## 10. Open Items — honest gaps

- pCloud's **trusted-issuer exchange endpoint is not documented.** The
  landed broker completes the IdP half of the handshake (OIDC discovery,
  PKCE auth code, ID-token verification, refresh). The pCloud-side
  `ID token → pCloud auth token` exchange is delivered through the
  pluggable `PcloudTokenExchanger` trait
  (`crates/pcloud-idp/src/exchange.rs`):
  - `NullPcloudTokenExchanger` is the default and returns
    `IdpError::NotConfigured("pCloud trusted-issuer exchange endpoint not configured; set [oidc.trusted_issuer].exchange_url")`.
    No fabricated success, no panic.
  - `HttpPcloudTokenExchanger` (cargo feature `oidc-http-exchange`, on
    by default) POSTs the ID token to a configurable URL and parses a
    pCloud-shaped session response. Site operators that run their own
    JWT-to-pCloud bridge service can wire this in; no pCloud-hosted
    exchange endpoint is officially documented at the time of writing.
  - HTTPS is enforced at construction time. Non-loopback `http://`
    URLs are rejected with `IdpError::NotConfigured`. Loopback
    plaintext is only permitted in `cfg(test)` / the
    `insecure-plaintext-exchange` feature, which is intentionally not
    enabled in any release build.

  This keeps the broker testable end-to-end against a stub exchange
  service today, without making a false claim that "log in to pCloud
  with Okta works in production" — it does not, until either pCloud
  publishes a trusted-issuer exchange endpoint or the operator stands
  up their own bridge and points `exchange_url` at it.
- SCIM user provisioning is out of scope for this crate.
- WebAuthn step-up inside the broker is out of scope; it is delegated to
  the IdP.

### 10.1 Operator configuration

```toml
[oidc.trusted_issuer]
exchange_url = "https://bridge.corp.example/pcloud/exchange"
```

If the key is absent, `NullPcloudTokenExchanger` is used and any login
attempt surfaces the typed `IdpError::NotConfigured` above.

## 11. Crate Layout

- `pcloud-idp` (this crate): trait scaffold, no I/O.
- `pcloud-idp-oidc` (future): OIDC Authorization Code + PKCE + Device Code.
- `pcloud-idp-ldap` (future, optional): LDAP seeding only.

All implementors depend on `pcloud-idp` and on `pcloud-secret`. They do
**not** depend on `pcloud-daemon`; the daemon wires them via its plugin
registry.

## 12. Interface / trait shape

Authoritative trait definition:

- `IdpBroker` trait — `crates/pcloud-idp/src/lib.rs:258`
- `IdpConfig` — `crates/pcloud-idp/src/lib.rs:164`
- `AuthChallenge` (carries PKCE verifier as `SecretString`) —
  `crates/pcloud-idp/src/lib.rs:183`
- `IdpToken` (id/access/refresh tokens as `SecretString`) —
  `crates/pcloud-idp/src/lib.rs:200`
- `IdpError` — `crates/pcloud-idp/src/lib.rs:214`
- `OidcAuthorizationCodeBroker` — `crates/pcloud-idp/src/oidc.rs:47`
  - `::new(issuer)` at `:66`
  - `::with_redirect_uri(..)` at `:72`
- JWKS cache (1 h TTL constant) — `crates/pcloud-idp/src/jwks.rs:26`
  (`pub(crate) const JWKS_TTL: Duration = Duration::from_secs(3600);`)
- PKCE S256 helpers — `crates/pcloud-idp/src/pkce.rs`

```rust
// Simplified; see crates/pcloud-idp/src/lib.rs:258 for the authoritative
// declaration.
#[async_trait]
pub trait IdpBroker: Send + Sync {
    async fn begin_authorization(&self, cfg: &IdpConfig) -> Result<AuthChallenge, IdpError>;
    async fn complete_authorization(
        &self,
        cfg: &IdpConfig,
        challenge: AuthChallenge,
        code: SecretString,
    ) -> Result<IdpToken, IdpError>;
    async fn refresh(&self, cfg: &IdpConfig, token: &IdpToken) -> Result<IdpToken, IdpError>;
}
```

## 13. Configuration reference

Every key in `[idp]` of `pcloud-rs.toml`:

| Key               | Type           | Default     | Purpose                                                    | Example |
|-------------------|----------------|-------------|------------------------------------------------------------|---------|
| `provider`        | enum string    | `"null"`    | `"null"` \| `"oidc"`. `null` disables the broker.          | `"oidc"` |
| `issuer`          | string (URL)   | —           | OIDC issuer. Used for discovery and `iss` claim pin.       | `"https://id.corp.example"` |
| `client_id`       | string         | —           | Public client ID registered at the IdP.                    | `"pcloud-rs-desktop"` |
| `redirect_uri`    | string (URL)   | `http://127.0.0.1/callback` (see `DEFAULT_REDIRECT_URI` @ `crates/pcloud-idp/src/oidc.rs:40`) | Loopback callback. Must be loopback in a public client.    | `"http://127.0.0.1:8765/callback"` |
| `scopes`          | `[string]`     | `["openid","profile","email"]` | Requested OIDC scopes.                        | `["openid","groups"]` |
| `jwks_ttl_seconds`| integer        | `3600`      | Override the JWKS cache TTL. Lower values increase IdP QPS.| `1800` |
| `audience`        | string         | `client_id` | Explicit `aud` claim value for verification.               | `"pcloud-rs-desktop"` |

Credentials (client secrets, etc.) are **never** permitted in the config
file — this is a public client. A non-empty `client_secret` key is a
load-time error.

## 14. Onboarding recipe

### Beginner — deploy in 5 steps

1. Register `pcloud-rs-desktop` as a **public** client in your IdP with
   `http://127.0.0.1/callback` as the single allowed redirect URI.
2. Record the issuer URL (whatever prefix exposes
   `/.well-known/openid-configuration`).
3. Add `[idp]` to `/etc/pcloud-rs/pcloud-rs.toml`:
   ```toml
   [idp]
   provider  = "oidc"
   issuer    = "https://id.corp.example"
   client_id = "pcloud-rs-desktop"
   ```
4. `sudo systemctl restart pcloudcd` and check the audit log for
   `idp.provider=oidc loaded` and the discovery document hash.
5. Run `pcloudc login --idp` on a test workstation; confirm the
   browser hands control back to the daemon and that `pcloudc status`
   prints the ID-token subject.

### Expert — Terraform integration

```hcl
resource "okta_app_oauth" "pcloud-rs" {
  label                     = "pcloud-rs-desktop"
  type                      = "native"
  grant_types               = ["authorization_code", "refresh_token"]
  pkce_required             = true
  token_endpoint_auth_method = "none"   # public client, PKCE only
  redirect_uris             = ["http://127.0.0.1/callback"]
  response_types            = ["code"]
}

resource "ansible_template" "pcloud-rs_toml" {
  vars = {
    issuer    = okta_auth_server.default.issuer
    client_id = okta_app_oauth.pcloud-rs.client_id
  }
}
```

Enforce `token_endpoint_auth_method = "none"`: any IdP that returns a
client secret will be rejected by `IdpConfig` validation at load time.

## 15. Verification — prove it's working

1. **Discovery pin** — `journalctl -u pcloudcd | grep 'idp.discovery'`:
   must show the post-normalised issuer matching the configured value.
2. **`alg=none` defence** — run the negative test:
   ```
   cargo test -p pcloud-idp rejects_alg_none
   ```
   covers `crates/pcloud-idp/src/jwks.rs` rejection path.
3. **JWKS TTL** — flip system clock +3601 s on a test box; the next
   verification must trigger a re-fetch (audit log `idp.jwks.refresh`).
4. **Secret redaction** — `pcloudc diag dump` must never emit the raw
   `id_token`; look for `SecretString(***redacted***)` in JSON output.
5. **End-to-end login** — `pcloudc login --idp && pcloudc whoami`.

## 16. Failure modes + remediation

| Symptom / `IdpError`                          | Root cause                                                | Remediation |
|-----------------------------------------------|-----------------------------------------------------------|-------------|
| `DiscoveryFailed`                             | Issuer unreachable, TLS interception, wrong URL           | Verify `openssl s_client -connect` to `issuer` succeeds; check corporate proxy's CA chain. |
| `UnsupportedAlgorithm`                        | IdP signs with HS256 or rotates to an unlisted alg        | Force IdP to RS256; we do **not** loosen the allow-list. |
| `NonceMismatch` / `StateMismatch`             | Replay attempt, browser cache poisoning, concurrent login | Clear session, retry. Repeated hits imply attack — escalate. |
| `TokenExchangeFailed`                         | Bad client config / clock skew                            | Check NTP (`timedatectl`); verify `client_id` exact match. |
| `NotConfigured("...exchange_url")`            | `[oidc.trusted_issuer].exchange_url` absent / `NullPcloudTokenExchanger` active | Set `exchange_url` to your bridge service, or accept that SSO-to-pCloud is disabled. See §10. |
| Loopback listener bind failure                | `127.0.0.1:*` blocked by a local firewall                 | Allowlist loopback; do **not** expose the listener on a routable IP. |

## 17. Extension points

To add a new broker implementor (e.g. SAML bridge, device-code flow):

1. Create a new crate (`pcloud-idp-<name>`).
2. Depend on `pcloud-idp`, `pcloud-secret`, `async-trait`, `reqwest`
   with `rustls-tls` (no native-tls).
3. Implement `IdpBroker` (`crates/pcloud-idp/src/lib.rs`). Return
   `IdpError::NotConfigured("...")` until a concrete trusted-issuer
   exchanger is wired — do **not** fabricate a success.
4. Wire the implementor into the daemon plugin registry
   (`crates/pcloud-daemon/src/runtime.rs`) behind a `provider` string
   value. Default must remain `NullBroker`.
5. Provide offline tests using an in-process stub discovery + JWKS +
   token endpoint. No network in CI.

Security constraints for any new implementor:

- Must wrap **every** token in `SecretString`.
- Must reject `alg=none` and symmetric algorithms.
- Must use `rustls` with `https_only(true)` — see
  `crates/pcloud-idp/src/oidc.rs:79`.

## 18. Cross-refs

- CLI: `docs/book/src/cli/login.md` (`pcloudc login --idp`)
- Config schema: `docs/book/src/config/idp.md`
- Runbook — IdP outage: `docs/runbooks/idp-outage.md`
- Security model: `docs/book/src/security/model.md`
- Secret wrappers: `crates/pcloud-secret/src/secret_string.rs`
- Parity row: `C_FEATURE_PARITY_MATRIX.csv` (feature `idp.*`,
  marked `Rejected` for legacy C — OIDC is net-new in the Rust path)
