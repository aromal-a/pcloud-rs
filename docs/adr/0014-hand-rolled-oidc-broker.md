# ADR 0014: Hand-Rolled OIDC Authorization-Code Broker

- Status: Accepted
- Date: 2026-04-16

## Context

Wave H delivered an OIDC identity broker (`pcloud-idp`) so enterprise
deployments can front pCloud auth with a trusted IdP. The broker needs
to perform PKCE S256 authorization-code flow, verify ID tokens with
JWKS, and exchange the resulting token for a pCloud session via the
(still-developing) trusted-issuer endpoint.

The obvious dependency choice is the `openidconnect` crate. It is
mature and widely used. However, for a pre-alpha security surface we
care about:

- **Unambiguous algorithm policy.** We must be able to state "RS256
  only, `alg=none` rejected before signature verification" and not
  rely on a transitive dependency doing the right thing;
- **Controlled secret handling.** Tokens must live in `SecretString`
  wrappers and never hit `Debug`/`Display`, which is tricky to enforce
  through a third-party type;
- **Minimal dependency footprint.** The enterprise crates already pull
  in rustls, reqwest, and serde; adding `openidconnect` brings
  transitive surface we have not audited;
- **Pre-alpha control surface.** We want every line of the OIDC
  validation path to be reviewable and patchable in this repository
  during the parity-gate phase.

## Decision

`pcloud-idp` ships `OidcAuthorizationCodeBroker`, a hand-rolled
implementor of the `IdpBroker` trait. The implementation is
deliberately minimal:

1. **PKCE S256 only.** `plain` is rejected at verifier-generation time;
   the code verifier is generated with `getrandom`, 64 bytes, base64url
   encoded, wrapped in `SecretString`.
2. **RS256-only JWKS verification** with a 1-hour in-memory TTL cache
   on the JWKS document. Algorithm confusion (`HS*` keys used with
   `RS*` JWKS) and `alg=none` are rejected **before** any signature
   verification attempt, not after.
3. **Issuer / audience / expiry validation** is performed in a fixed
   order, documented in the source, and unit-tested with negative
   cases for each failure mode.
4. **Every token surface is `SecretString`**: ID token, access token,
   refresh token, code verifier, client secret. `Debug` is redacted
   and `Display` is not implemented.
5. **pCloud trusted-issuer token exchange is explicitly stubbed** and
   returns `IdpError::TrustedIssuerExchangeUnavailable` until pCloud
   publishes the endpoint. Live pCloud OIDC interop is **not**
   claimed — documented in `docs/enterprise/oidc-broker.md`.
6. **`IdpBroker` is object-safe** so enterprise deployments can plug in
   a replacement (e.g. a different algorithm policy, a managed broker)
   without patching `pcloud-idp`.

## Consequences

Good:

- Every line of the algorithm policy is in-repo and reviewable.
- Secret-wrapping is enforced by the type system end-to-end; a
  reviewer can follow every `SecretString` boundary without reading
  third-party code.
- The JWKS cache behaviour is ours; rotation semantics are exactly
  the behaviour we documented, not the behaviour a crate happens to
  ship this release.
- The surface stays minimal — roughly 1.2 k lines including tests —
  which is within pre-alpha review budget.

Bad:

- We own the maintenance of the OIDC validation path. When the OIDC
  spec tightens (e.g. new algorithm guidance), we patch rather than
  `cargo update`.
- We do not get the ecosystem testing the `openidconnect` crate
  receives. Mitigated by negative-case unit tests for every failure
  branch, property tests over JWT shape, and the explicit
  "trusted-issuer exchange unavailable" caveat.
- When we revisit this decision post-alpha, migrating to a crate
  dependency requires a new ADR that supersedes this one.

## Alternatives Considered

- **`openidconnect` crate**: rejected for pre-alpha — we want every
  line of the signature-verification path in-repo. Re-evaluate after
  `bd-1du.10` closes.
- **Delegate to a native broker (e.g. `kcd`, `mod_auth_openidc`)**:
  rejected — moves the trust boundary out of the daemon and creates a
  new deployment dependency for enterprise admins.
- **Accept HS256 for a simpler client-secret flow**: rejected —
  algorithm confusion is a repeated real-world OIDC CVE class; we
  standardise on RS256 and document it.
