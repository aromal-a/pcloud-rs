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
//! - **Protocol version pin.** Only TLS 1.3 and TLS 1.2 are enabled.
//!   Older versions (TLS 1.0 / 1.1 / SSL 3) are categorically rejected.
//! - **Root trust.** Mozilla's `webpki-roots` bundle is used; no system
//!   trust store, no ad-hoc CA injection.
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

    use std::sync::Arc;
}
