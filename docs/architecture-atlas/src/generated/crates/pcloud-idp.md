# `pcloud-idp`

**Maturity:** Experimental / bounded

**Version:** `0.1.0`

**Directory:** `crates/pcloud-idp`

**Manifest:** [`crates/pcloud-idp/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/Cargo.toml)

Enterprise identity-provider (IdP) broker traits for federated login (OIDC, SAML, LDAP). Includes an OIDC Authorization Code + PKCE broker.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_idp` | lib | [`crates/pcloud-idp/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs) |

## Direct dependencies

`base64`, `getrandom`, `jsonwebtoken`, `pcloud-observability`, `pcloud-secret`, `reqwest`, `serde`, `serde_json`, `sha2`, `thiserror`, `url`

## Cargo features

| Feature | Enables |
|---|---|
| `default` | `oidc-http-exchange` |
| `insecure-plaintext-exchange` | empty marker |
| `oidc-http-exchange` | empty marker |

## File inventory (6)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-idp/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/Cargo.toml) | Cargo manifest | HTTP-backed pCloud trusted-issuer exchanger. On by default so the |
| [`crates/pcloud-idp/src/exchange.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs) | Rust module | pCloud trusted-issuer token exchange. |
| [`crates/pcloud-idp/src/jwks.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs) | Rust module | OIDC discovery and JWKS fetch/cache with RS256-only ID token verification. |
| [`crates/pcloud-idp/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs) | library root | Enterprise Identity Provider (IdP) broker trait scaffold. |
| [`crates/pcloud-idp/src/oidc.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs) | Rust module | OIDC Authorization Code + PKCE broker. |
| [`crates/pcloud-idp/src/pkce.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/pkce.rs) | Rust module | RFC 7636 PKCE helpers (S256 only). |

## Rust declaration index (102 total; 41 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `PcloudSession` | `pub` | struct | [`crates/pcloud-idp/src/exchange.rs:47`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L47) | Short-lived pCloud session derived from a trusted-issuer exchange. The daemon persists this through its exist… |
| `PcloudTokenExchanger` | `pub` | trait | [`crates/pcloud-idp/src/exchange.rs:75`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L75) | Pluggable pCloud trusted-issuer exchange. Implementors convert a verified IdP ID token into a pCloud session.… |
| `exchange` | `private` | fn | [`crates/pcloud-idp/src/exchange.rs:86`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L86) | Exchange an IdP ID token for a pCloud session. # Errors - \[`IdpError::NotConfigured`\] if no exchange endpoint… |
| `NullPcloudTokenExchanger` | `pub` | struct | [`crates/pcloud-idp/src/exchange.rs:95`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L95) | Default exchanger that always returns \[`IdpError::NotConfigured`\]. Wired by default so the daemon plugin regi… |
| `NULL_EXCHANGER_MESSAGE` | `pub` | const | [`crates/pcloud-idp/src/exchange.rs:99`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L99) | Operator guidance returned when the default \[`NullPcloudTokenExchanger`\] is invoked. |
| `exchange` | `private` | fn | [`crates/pcloud-idp/src/exchange.rs:102`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L102) | Read the source/rustdoc for the exact contract. |
| `http_exchanger` | `private` | mod | [`crates/pcloud-idp/src/exchange.rs:115`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L115) | Read the source/rustdoc for the exact contract. |
| `HttpPcloudTokenExchanger` | `pub` | struct | [`crates/pcloud-idp/src/exchange.rs:138`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L138) | HTTP-backed pCloud trusted-issuer exchanger. POSTs a JSON body `{ "id_token": "&lt;jwt&gt;" }` to the configured en… |
| `fmt` | `private` | fn | [`crates/pcloud-idp/src/exchange.rs:144`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L144) | Read the source/rustdoc for the exact contract. |
| `new` | `pub` | fn | [`crates/pcloud-idp/src/exchange.rs:155`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L155) | Construct an exchanger targeting `exchange_url`. The URL must be `https://` in release builds; `http://` is r… |
| `with_client` | `pub` | fn | [`crates/pcloud-idp/src/exchange.rs:173`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L173) | Construct with a caller-provided HTTP client. Used in tests to inject a client that permits `http://127.0.0.1… |
| `reject_plaintext_in_prod` | `private` | fn | [`crates/pcloud-idp/src/exchange.rs:184`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L184) | Read the source/rustdoc for the exact contract. |
| `ExchangeRequest` | `private` | struct | [`crates/pcloud-idp/src/exchange.rs:200`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L200) | Read the source/rustdoc for the exact contract. |
| `post_exchange` | `private` | fn | [`crates/pcloud-idp/src/exchange.rs:208`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L208) | Read the source/rustdoc for the exact contract. |
| `ExchangeResponse` | `private` | struct | [`crates/pcloud-idp/src/exchange.rs:237`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L237) | Read the source/rustdoc for the exact contract. |
| `exchange` | `private` | fn | [`crates/pcloud-idp/src/exchange.rs:244`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L244) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-idp/src/exchange.rs:258`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L258) | Read the source/rustdoc for the exact contract. |
| `null_exchanger_returns_not_configured` | `private` | fn | [`crates/pcloud-idp/src/exchange.rs:262`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L262) | Read the source/rustdoc for the exact contract. |
| `null_exchanger_is_object_safe` | `private` | fn | [`crates/pcloud-idp/src/exchange.rs:275`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L275) | Read the source/rustdoc for the exact contract. |
| `http` | `private` | mod | [`crates/pcloud-idp/src/exchange.rs:280`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L280) | Read the source/rustdoc for the exact contract. |
| `spawn_stub` | `private` | fn | [`crates/pcloud-idp/src/exchange.rs:288`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L288) | Read the source/rustdoc for the exact contract. |
| `permissive_client` | `private` | fn | [`crates/pcloud-idp/src/exchange.rs:305`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L305) | Read the source/rustdoc for the exact contract. |
| `http_exchanger_rejects_plaintext_url_in_prod_path` | `private` | fn | [`crates/pcloud-idp/src/exchange.rs:315`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L315) | Read the source/rustdoc for the exact contract. |
| `http_exchanger_returns_pcloud_session_on_success` | `private` | fn | [`crates/pcloud-idp/src/exchange.rs:323`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L323) | Read the source/rustdoc for the exact contract. |
| `http_exchanger_maps_401_to_refresh_rejected` | `private` | fn | [`crates/pcloud-idp/src/exchange.rs:339`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L339) | Read the source/rustdoc for the exact contract. |
| `http_exchanger_maps_500_to_token_exchange_without_body` | `private` | fn | [`crates/pcloud-idp/src/exchange.rs:351`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/exchange.rs#L351) | Read the source/rustdoc for the exact contract. |
| `JWKS_TTL` | `pub(crate)` | const | [`crates/pcloud-idp/src/jwks.rs:27`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L27) | Default JWKS cache TTL (1 hour). |
| `DiscoveryDocument` | `pub(crate)` | struct | [`crates/pcloud-idp/src/jwks.rs:31`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L31) | Subset of the OIDC discovery document the broker needs. |
| `Jwk` | `pub(crate)` | struct | [`crates/pcloud-idp/src/jwks.rs:43`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L43) | A single key in a JWKS document. Only RSA keys are usable by this broker; other `kty` values are retained so… |
| `JwkSet` | `pub(crate)` | struct | [`crates/pcloud-idp/src/jwks.rs:59`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L59) | Top-level JWKS document shape. |
| `IdTokenClaims` | `pub(crate)` | struct | [`crates/pcloud-idp/src/jwks.rs:68`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L68) | Read the source/rustdoc for the exact contract. |
| `Audience` | `pub(crate)` | struct | [`crates/pcloud-idp/src/jwks.rs:82`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L82) | Audience deserializer that accepts either a single string or a list. |
| `deserialize` | `private` | fn | [`crates/pcloud-idp/src/jwks.rs:85`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L85) | Read the source/rustdoc for the exact contract. |
| `Raw` | `private` | enum | [`crates/pcloud-idp/src/jwks.rs:91`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L91) | Read the source/rustdoc for the exact contract. |
| `JwksCache` | `pub(crate)` | struct | [`crates/pcloud-idp/src/jwks.rs:115`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L115) | JWKS cache with a TTL and a bounded forced-refresh policy. The cache stores the discovery document alongside… |
| `CacheState` | `private` | struct | [`crates/pcloud-idp/src/jwks.rs:122`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L122) | Read the source/rustdoc for the exact contract. |
| `new` | `pub(crate)` | fn | [`crates/pcloud-idp/src/jwks.rs:129`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L129) | Read the source/rustdoc for the exact contract. |
| `refresh` | `pub(crate)` | fn | [`crates/pcloud-idp/src/jwks.rs:143`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L143) | Force-refresh the discovery document and JWKS. |
| `discovery` | `pub(crate)` | fn | [`crates/pcloud-idp/src/jwks.rs:176`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L176) | Return the cached discovery document, refreshing if absent or stale. |
| `lookup_key` | `private` | fn | [`crates/pcloud-idp/src/jwks.rs:191`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L191) | Look up a JWK by `kid`, forcing a single refresh on miss. |
| `verify_id_token` | `pub(crate)` | fn | [`crates/pcloud-idp/src/jwks.rs:211`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L211) | Verify an ID token: header.alg ∈ {RS256}, signature via the JWKS, `iss`/`aud`/`exp`/`nbf` enforced. Returns t… |
| `pick_key` | `private` | fn | [`crates/pcloud-idp/src/jwks.rs:249`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L249) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-idp/src/jwks.rs:257`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L257) | Read the source/rustdoc for the exact contract. |
| `id_token_signature_rejected_if_alg_none` | `private` | fn | [`crates/pcloud-idp/src/jwks.rs:265`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L265) | Forge a JWT with `alg=none` and a valid-looking payload, confirm the verifier refuses it before touching any… |
| `audience_deser_accepts_string_and_array` | `private` | fn | [`crates/pcloud-idp/src/jwks.rs:303`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L303) | Read the source/rustdoc for the exact contract. |
| `W` | `private` | struct | [`crates/pcloud-idp/src/jwks.rs:305`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/jwks.rs#L305) | Read the source/rustdoc for the exact contract. |
| `exchange` | `pub` | mod | [`crates/pcloud-idp/src/lib.rs:154`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L154) | Read the source/rustdoc for the exact contract. |
| `jwks` | `pub(crate)` | mod | [`crates/pcloud-idp/src/lib.rs:155`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L155) | Read the source/rustdoc for the exact contract. |
| `oidc` | `pub` | mod | [`crates/pcloud-idp/src/lib.rs:156`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L156) | Read the source/rustdoc for the exact contract. |
| `pkce` | `pub(crate)` | mod | [`crates/pcloud-idp/src/lib.rs:157`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L157) | Read the source/rustdoc for the exact contract. |
| `IdpFlow` | `pub` | enum | [`crates/pcloud-idp/src/lib.rs:171`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L171) | Supported federation flows. The primary flow is \[`IdpFlow::OidcAuthorizationCode`\] with PKCE. The others exis… |
| `IdpConfig` | `pub` | struct | [`crates/pcloud-idp/src/lib.rs:192`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L192) | Operator-facing IdP configuration. This struct mirrors the `\[auth.idp\]` section of `config.toml`. It does not… |
| `AuthChallenge` | `pub` | struct | [`crates/pcloud-idp/src/lib.rs:211`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L211) | Server-bound authorization challenge returned by \[`IdpBroker::begin_authorization`\]. The challenge owns the e… |
| `IdpToken` | `pub` | struct | [`crates/pcloud-idp/src/lib.rs:228`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L228) | Federated identity token material returned by the IdP. The `id_token` is the JWT that the pCloud "trusted-iss… |
| `IdpError` | `pub` | enum | [`crates/pcloud-idp/src/lib.rs:242`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L242) | Errors surfaced by an \[`IdpBroker`\] implementation. Variants are intentionally coarse for the scaffold; concr… |
| `IdpBroker` | `pub` | trait | [`crates/pcloud-idp/src/lib.rs:294`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L294) | Broker trait implemented per federation flow. Implementors live in dedicated crates (`pcloud-idp-oidc`, `pclo… |
| `begin_authorization` | `private` | fn | [`crates/pcloud-idp/src/lib.rs:312`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L312) | Construct the authorization challenge the user's browser will follow. Implementations MUST: - fetch and pin t… |
| `complete_authorization` | `private` | fn | [`crates/pcloud-idp/src/lib.rs:331`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L331) | Complete the authorization flow by exchanging `code` for an \[`IdpToken`\]. `challenge` is consumed to prevent… |
| `refresh` | `private` | fn | [`crates/pcloud-idp/src/lib.rs:354`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L354) | Refresh an \[`IdpToken`\] using its refresh token. Returns \[`IdpError::RefreshRejected`\] when the IdP rejects t… |
| `UnimplementedBroker` | `pub` | struct | [`crates/pcloud-idp/src/lib.rs:365`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L365) | No-op broker used by the scaffold and by tests that need a trait object. Every method returns \[`IdpError::Not… |
| `UNIMPLEMENTED_BROKER_MESSAGE` | `pub` | const | [`crates/pcloud-idp/src/lib.rs:370`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L370) | Human-readable operator guidance returned by \[`UnimplementedBroker`\] when a concrete \[`IdpBroker`\] has not be… |
| `begin_authorization` | `private` | fn | [`crates/pcloud-idp/src/lib.rs:374`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L374) | Read the source/rustdoc for the exact contract. |
| `complete_authorization` | `private` | fn | [`crates/pcloud-idp/src/lib.rs:378`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L378) | Read the source/rustdoc for the exact contract. |
| `refresh` | `private` | fn | [`crates/pcloud-idp/src/lib.rs:386`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L386) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-idp/src/lib.rs:392`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L392) | Read the source/rustdoc for the exact contract. |
| `config_roundtrip` | `private` | fn | [`crates/pcloud-idp/src/lib.rs:396`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L396) | Read the source/rustdoc for the exact contract. |
| `broker_is_object_safe` | `private` | fn | [`crates/pcloud-idp/src/lib.rs:408`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L408) | Read the source/rustdoc for the exact contract. |
| `unimplemented_broker_returns_not_configured_instead_of_panicking` | `private` | fn | [`crates/pcloud-idp/src/lib.rs:416`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/lib.rs#L416) | Read the source/rustdoc for the exact contract. |
| `DEFAULT_REDIRECT_URI` | `pub` | const | [`crates/pcloud-idp/src/oidc.rs:40`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L40) | Default redirect URI for desktop clients that bind a loopback listener on a random port. Operators override t… |
| `CHALLENGE_TTL` | `private` | const | [`crates/pcloud-idp/src/oidc.rs:44`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L44) | Authorization-challenge lifetime. Matches the 10-minute ceiling most IdPs enforce on `state` reuse. |
| `OidcAuthorizationCodeBroker` | `pub` | struct | [`crates/pcloud-idp/src/oidc.rs:47`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L47) | Concrete OIDC broker for the Authorization Code + PKCE flow. |
| `fmt` | `private` | fn | [`crates/pcloud-idp/src/oidc.rs:54`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L54) | Read the source/rustdoc for the exact contract. |
| `new` | `pub` | fn | [`crates/pcloud-idp/src/oidc.rs:66`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L66) | Construct a broker pinned to `issuer` using the default redirect URI. The HTTP client is configured with rust… |
| `with_redirect_uri` | `pub` | fn | [`crates/pcloud-idp/src/oidc.rs:72`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L72) | Construct a broker with an explicit redirect URI (e.g. a loopback URL containing the bound port chosen at run… |
| `build_authorization_url` | `pub(crate)` | fn | [`crates/pcloud-idp/src/oidc.rs:94`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L94) | Build a challenge from a pre-generated verifier/state/nonce — used in tests to assert URL formatting without… |
| `build_auth_url` | `private` | fn | [`crates/pcloud-idp/src/oidc.rs:113`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L113) | Read the source/rustdoc for the exact contract. |
| `TokenResponse` | `private` | struct | [`crates/pcloud-idp/src/oidc.rs:147`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L147) | Raw OAuth2 token-endpoint response shape. `access_token` is accepted but the broker does not surface it: pClo… |
| `TokenErrorResponse` | `private` | struct | [`crates/pcloud-idp/src/oidc.rs:157`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L157) | Token-endpoint error surface per RFC 6749 §5.2. |
| `post_token` | `private` | fn | [`crates/pcloud-idp/src/oidc.rs:164`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L164) | Read the source/rustdoc for the exact contract. |
| `begin_authorization` | `private` | fn | [`crates/pcloud-idp/src/oidc.rs:196`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L196) | Read the source/rustdoc for the exact contract. |
| `complete_authorization` | `private` | fn | [`crates/pcloud-idp/src/oidc.rs:217`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L217) | Read the source/rustdoc for the exact contract. |
| `refresh` | `private` | fn | [`crates/pcloud-idp/src/oidc.rs:253`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L253) | Read the source/rustdoc for the exact contract. |
| `extract_client_id` | `private` | fn | [`crates/pcloud-idp/src/oidc.rs:286`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L286) | Read the source/rustdoc for the exact contract. |
| `parse_aud_unverified` | `private` | fn | [`crates/pcloud-idp/src/oidc.rs:299`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L299) | Parse the `aud` claim from an ID token *without* verifying the signature. The value is only used to drive the… |
| `Aud` | `private` | enum | [`crates/pcloud-idp/src/oidc.rs:311`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L311) | Read the source/rustdoc for the exact contract. |
| `P` | `private` | struct | [`crates/pcloud-idp/src/oidc.rs:316`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L316) | Read the source/rustdoc for the exact contract. |
| `compute_expiry` | `private` | fn | [`crates/pcloud-idp/src/oidc.rs:329`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L329) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-idp/src/oidc.rs:339`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L339) | Read the source/rustdoc for the exact contract. |
| `cfg` | `private` | fn | [`crates/pcloud-idp/src/oidc.rs:344`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L344) | Read the source/rustdoc for the exact contract. |
| `authorization_url_includes_state_and_challenge` | `private` | fn | [`crates/pcloud-idp/src/oidc.rs:354`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L354) | Read the source/rustdoc for the exact contract. |
| `token_roundtrip_is_zeroized_on_drop` | `private` | fn | [`crates/pcloud-idp/src/oidc.rs:393`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L393) | Read the source/rustdoc for the exact contract. |
| `extract_client_id_works` | `private` | fn | [`crates/pcloud-idp/src/oidc.rs:417`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L417) | Read the source/rustdoc for the exact contract. |
| `openid_scope_is_injected_when_missing` | `private` | fn | [`crates/pcloud-idp/src/oidc.rs:426`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/oidc.rs#L426) | Read the source/rustdoc for the exact contract. |
| `VERIFIER_BYTES` | `pub(crate)` | const | [`crates/pcloud-idp/src/pkce.rs:17`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/pkce.rs#L17) | Length of the raw random material used for the PKCE code verifier. 32 bytes encodes to 43 unpadded base64url… |
| `STATE_BYTES` | `pub(crate)` | const | [`crates/pcloud-idp/src/pkce.rs:21`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/pkce.rs#L21) | Length of the random CSRF `state` parameter. 16 bytes → 22 unpadded base64url characters, which comfortably e… |
| `NONCE_BYTES` | `pub(crate)` | const | [`crates/pcloud-idp/src/pkce.rs:25`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/pkce.rs#L25) | Length of the OIDC `nonce`. 16 bytes → 128 bits of entropy to bind the request to the returned ID token's `no… |
| `random_bytes` | `pub(crate)` | fn | [`crates/pcloud-idp/src/pkce.rs:28`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/pkce.rs#L28) | Fill `buf` with cryptographically strong random bytes via \[`getrandom`\]. |
| `random_token` | `pub(crate)` | fn | [`crates/pcloud-idp/src/pkce.rs:34`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/pkce.rs#L34) | Generate a fresh URL-safe token of `n` random bytes, base64url-encoded without padding. Used for `code_verifi… |
| `s256_challenge` | `pub` | fn | [`crates/pcloud-idp/src/pkce.rs:47`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/pkce.rs#L47) | Compute the S256 PKCE challenge: `base64url(sha256(verifier))`. ```ignore let c = pcloud_idp::pkce::s256_chal… |
| `tests` | `private` | mod | [`crates/pcloud-idp/src/pkce.rs:55`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/pkce.rs#L55) | Read the source/rustdoc for the exact contract. |
| `pkce_challenge_is_s256_of_verifier` | `private` | fn | [`crates/pcloud-idp/src/pkce.rs:62`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/pkce.rs#L62) | RFC 7636 Appendix B test vector: verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk" challenge = "E9Melh… |
| `random_token_is_unpadded_urlsafe` | `private` | fn | [`crates/pcloud-idp/src/pkce.rs:69`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-idp/src/pkce.rs#L69) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.
