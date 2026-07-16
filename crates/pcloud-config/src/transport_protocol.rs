//! T2.6 — transport-protocol selector + fallback decision matrix.
//!
//! # AI-scope deliverable
//!
//! `TransportProtocol` lets the operator choose between TLS-over-TCP
//! (the existing default) and QUIC. `FallbackPolicy` describes what
//! the daemon should do when the preferred protocol's handshake
//! fails. Both are pure compute; the wire-level QUIC implementation
//! (which would pull `quinn`) is the follow-up — the live
//! verification needs a QUIC-enabled pCloud endpoint and a TLS cert
//! chain validated against pCloud's certs.
//!
//! Keeping this in `pcloud-config` (not `pcloud-proto`) means
//! profiles can express the preference + fallback policy without
//! the proto layer needing to grow a dep on `quinn` until the live
//! transport actually lands.
//!
//! # Decision matrix
//!
//! When a request needs to dispatch:
//!
//! | preferred | fallback                | handshake result   | actual transport |
//! |-----------|-------------------------|---------------------|------------------|
//! | TLS       | any                     | TLS ok              | TLS              |
//! | TLS       | any                     | TLS fails           | error (no fallback away from TLS) |
//! | QUIC      | `Strict`                | QUIC ok             | QUIC             |
//! | QUIC      | `Strict`                | QUIC fails          | error            |
//! | QUIC      | `FallBackToTls`         | QUIC ok             | QUIC             |
//! | QUIC      | `FallBackToTls`         | QUIC fails          | TLS              |
//!
//! The matrix is encoded by [`resolve_after_handshake`] so the
//! daemon-side dispatcher just calls one function and the operator's
//! preference + policy are honoured deterministically.

// **PLATFORM:** all
// **GATING:** none.

use serde::{Deserialize, Serialize};

/// Transport protocol the operator prefers for the API endpoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportProtocol {
    /// TLS-over-TCP (the existing default). Always valid in
    /// production.
    #[default]
    Tls,
    /// QUIC / HTTP/3. Preferred when it works; the live integration
    /// is gated on a QUIC-enabled pCloud endpoint + the `quinn`
    /// dependency.
    Quic,
}

/// What to do when the preferred transport fails its handshake.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackPolicy {
    /// No fallback: a handshake failure is propagated as an error.
    /// This is the right pick when the operator must know that
    /// QUIC is broken (e.g. monitoring an explicit "use HTTP/3"
    /// rollout).
    Strict,
    /// Fall back to TLS-over-TCP when the preferred transport
    /// fails. Default for the recommended posture: prefer QUIC,
    /// silently fall back so a flaky middlebox does not break
    /// transfers.
    #[default]
    FallBackToTls,
}

/// Outcome reported by the dispatcher's actual handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeOutcome {
    /// The preferred transport's handshake succeeded.
    PreferredOk,
    /// The preferred transport's handshake failed. The dispatcher
    /// must consult the fallback policy to decide what to do.
    PreferredFailed,
}

/// Decision returned by [`resolve_after_handshake`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportDecision {
    /// Use the preferred transport — it handshook successfully.
    UsePreferred,
    /// Fall back to TLS-over-TCP. Only emitted when
    /// `preferred == Quic` AND `policy == FallBackToTls` AND the
    /// preferred handshake failed.
    FallBackToTls,
    /// Hard error — the preferred transport failed and no fallback
    /// is allowed.
    Error,
}

/// Combine the operator's preference, policy, and the live
/// handshake outcome into a single decision the dispatcher
/// dispatches on.
#[must_use]
pub fn resolve_after_handshake(
    preferred: TransportProtocol,
    policy: FallbackPolicy,
    outcome: HandshakeOutcome,
) -> TransportDecision {
    if matches!(outcome, HandshakeOutcome::PreferredOk) {
        return TransportDecision::UsePreferred;
    }
    // Preferred failed.
    match (preferred, policy) {
        (TransportProtocol::Tls, _) => TransportDecision::Error,
        (TransportProtocol::Quic, FallbackPolicy::Strict) => TransportDecision::Error,
        (TransportProtocol::Quic, FallbackPolicy::FallBackToTls) => {
            TransportDecision::FallBackToTls
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_recommended_posture() {
        assert_eq!(TransportProtocol::default(), TransportProtocol::Tls);
        assert_eq!(FallbackPolicy::default(), FallbackPolicy::FallBackToTls);
    }

    #[test]
    fn handshake_ok_always_uses_preferred() {
        for preferred in [TransportProtocol::Tls, TransportProtocol::Quic] {
            for policy in [FallbackPolicy::Strict, FallbackPolicy::FallBackToTls] {
                assert_eq!(
                    resolve_after_handshake(preferred, policy, HandshakeOutcome::PreferredOk),
                    TransportDecision::UsePreferred,
                );
            }
        }
    }

    #[test]
    fn tls_failure_never_falls_back() {
        // TLS is the floor; there is nothing below it to fall back
        // to. Both policy values produce Error.
        for policy in [FallbackPolicy::Strict, FallbackPolicy::FallBackToTls] {
            assert_eq!(
                resolve_after_handshake(
                    TransportProtocol::Tls,
                    policy,
                    HandshakeOutcome::PreferredFailed
                ),
                TransportDecision::Error,
            );
        }
    }

    #[test]
    fn quic_strict_failure_is_error() {
        assert_eq!(
            resolve_after_handshake(
                TransportProtocol::Quic,
                FallbackPolicy::Strict,
                HandshakeOutcome::PreferredFailed
            ),
            TransportDecision::Error,
        );
    }

    #[test]
    fn quic_fallback_failure_falls_back_to_tls() {
        assert_eq!(
            resolve_after_handshake(
                TransportProtocol::Quic,
                FallbackPolicy::FallBackToTls,
                HandshakeOutcome::PreferredFailed
            ),
            TransportDecision::FallBackToTls,
        );
    }

    #[test]
    fn serde_roundtrip_protocol() {
        let json = serde_json::to_string(&TransportProtocol::Quic).unwrap();
        assert_eq!(json, "\"quic\"");
        let back: TransportProtocol = serde_json::from_str(&json).unwrap();
        assert_eq!(back, TransportProtocol::Quic);
    }

    #[test]
    fn serde_roundtrip_policy() {
        let json = serde_json::to_string(&FallbackPolicy::FallBackToTls).unwrap();
        assert_eq!(json, "\"fall_back_to_tls\"");
        let back: FallbackPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, FallbackPolicy::FallBackToTls);
    }
}
