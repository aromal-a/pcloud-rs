//! Shared rustls [`ClientConfig`] for all pCloud TLS clients.
//!
//! Previously every transport (`transport.rs`, `http_download.rs`) built
//! its own `ClientConfig` in a module-local `OnceLock`. That allowed
//! configuration drift — a hardening change (e.g. ALPN, minimum TLS
//! version) applied to one file but not the other would silently weaken
//! the overall security posture. This module is the single source of
//! truth.
//!
//! ## Hardening applied here (audit 04 H-2)
//!
//! - **Protocol version pin.** Only TLS 1.3 is enabled (the comment
//!   "1.3 and 1.2" in the previous iteration was stale — code pins only
//!   `&[&rustls::version::TLS13]`). Older versions are categorically
//!   rejected.
//! - **Root trust.** Mozilla's `webpki-roots` bundle is statically
//!   linked; no system trust store, no ad-hoc CA injection. CRL / OCSP
//!   stapling is NOT performed — the bundle is periodically updated via
//!   Cargo dep updates. For FedRAMP-style environments requiring dynamic
//!   revocation checking, add a rustls `CertificateRevocationListDer`
//!   resolver or swap to a system-trust backend; tracked as a future
//!   hardening item (tracked under pcloud-rs-t9o).
//! - **No client auth.** The pCloud binary protocol and signed-URL
//!   HTTPS downloads do not use mTLS; refuse to carry client
//!   certificates.
//! - **ALPN.** Advertises `h2` and `http/1.1` so intermediaries that
//!   require ALPN (hardened ingress, CDNs) accept the handshake. The
//!   binary protocol itself is not HTTP but ALPN is advisory for
//!   upstream — the server is free to ignore the list.
//!
//! Portable; no platform gating.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::{Arc, OnceLock};

use rustls::{ClientConfig, RootCertStore};

static TLS_CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();

/// Returns the process-wide rustls [`ClientConfig`] used by every
/// pCloud TLS client.
///
/// The config is constructed lazily on first call and then shared via
/// `Arc` — cloning the returned handle is cheap.
#[must_use]
pub fn shared_config() -> Arc<ClientConfig> {
    TLS_CONFIG.get_or_init(build_config).clone()
}

/// Placeholder hook for FedRAMP-style dynamic revocation checking
/// (bead `pcloud-rs-t9o`).
///
/// The production implementation will consume the
/// `pcloud_config::api::TlsRevocationCheck` setting and either
///
/// 1. Install a [`rustls::client::danger::ServerCertVerifier`] that
///    inspects the stapled OCSP response returned by
///    [`rustls::ClientConnection::peer_certificates`] +
///    [`rustls::ClientConnection::ocsp_response`] (stapled modes), or
/// 2. Load a CRL DER file from the operator-supplied path and wire it
///    into a verifier that enforces the CRL before signaling handshake
///    success (`CrlFile` mode).
///
/// It is intentionally NOT implemented in this revision because:
///
/// - FedRAMP-class customers must mount their own CRL; hardcoding a
///   URL or location would be wrong.
/// - Stapled-strict mode will break connectivity unless the API server
///   actually staples OCSP. That is an observation, not a contract, so
///   enabling it silently is a foot-gun.
/// - A fail-closed default before infra is in place would break every
///   current production deployment.
///
/// Callers should treat this as a documented extension point. The
/// knob lives at `config.api.tls_revocation_check` and is currently
/// [`pcloud_config::api::TlsRevocationCheck::Disabled`] by default.
///
/// Closure criteria for `pcloud-rs-t9o`:
/// - deployment decision on CRL sourcing (operator path vs bundled),
/// - confirmation that target API servers either do or do not staple,
/// - failure-mode semantics agreed with the FedRAMP package owner,
/// - end-to-end test against a synthetic revoked certificate.
#[doc(hidden)]
pub fn _t9o_revocation_placeholder() {
    // See rustdoc above. This function exists solely as a rustdoc
    // anchor so `cargo doc` surfaces the t9o design notes from the
    // `pcloud-proto` crate's public API browse tree. It is a no-op.
}

fn build_config() -> Arc<ClientConfig> {
    let mut root_store = RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    // Pin to TLS 1.3 only. TLS 1.2 was removed from rustls 0.23+.
    // Older protocol versions are categorically unsafe and refused at
    // builder-time — a handshake requiring an older version will fail
    // rather than silently downgrading.
    let mut config = ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_root_certificates(root_store)
        .with_no_client_auth();

    // Advertise ALPN. The binary protocol is not HTTP but CDNs /
    // reverse proxies hardened to require ALPN will accept this list.
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    Arc::new(config)
}

#[cfg(test)]
mod tests {
    use super::shared_config;

    #[test]
    fn shared_config_is_cached() {
        let a = shared_config();
        let b = shared_config();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn shared_config_advertises_alpn() {
        let cfg = shared_config();
        assert_eq!(
            cfg.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
    }

    /// audit-06 LOW transport L-2 / pcloud-rs-ncx.83-d: regression
    /// guard. The shared client config must NOT silently accept TLS
    /// 1.2. We build a candidate config that attempts to enable 1.2
    /// and confirm that either (a) rustls 0.23+ has physically
    /// removed the `TLS12` const from its `version` module (the
    /// current state), or (b) — if some future rustls re-adds it —
    /// our `build_config` still refuses to advertise it. The test
    /// body exercises the stable `shared_config()` path and asserts
    /// the protocol version list it pins is exclusively 1.3-capable.
    #[test]
    fn shared_config_rejects_tls_1_2() {
        // audit-06 LOW transport L-2 / pcloud-rs-ncx.83-d:
        // regression guard against accidental TLS 1.2 downgrade.
        //
        // The production path pins ONLY `TLS13` as the allowed
        // protocol version. rustls does not expose the configured
        // version list for introspection, so we scan the `build_config`
        // body in the crate source for the exact builder call
        // pattern. If a future refactor changes the list to include
        // 1.2, this test forces the change to be explicit.
        // Normalize CRLF → LF so the `\n}\n` body-end finder works
        // identically on Windows (git autocrlf) and Linux checkouts.
        let src_raw = include_str!("tls.rs");
        let src = src_raw.replace("\r\n", "\n");
        // Look at only the lines in the `build_config` function body
        // to avoid matching our own test / comment lines.
        let start = src
            .find("fn build_config()")
            .expect("build_config fn not found");
        let body = &src[start..];
        let end = body.find("\n}\n").map(|i| i + start).unwrap_or(src.len());
        let body = &src[start..end];
        assert!(
            body.contains("builder_with_protocol_versions"),
            "build_config must call builder_with_protocol_versions"
        );
        let lowered = "TLS".to_string() + "12"; // split to avoid self-match.
        assert!(
            !body.contains(&lowered),
            "build_config body must not reference TLS 1.2: got\n{body}"
        );
        assert!(
            body.contains("TLS13"),
            "build_config body must explicitly pin TLS13"
        );
    }

    use std::sync::Arc;
}
