#![allow(clippy::pedantic)]
//! Integration test: bootstrap decides transport wrapping by environment.
//!
//! Validates the contract landed by the resilience-default-enable work:
//!
//! - `Environment::Production` bootstrap produces a factory whose
//!   `wrap_binary` call returns `Some(ResilientTransport<_>)`, i.e. the
//!   outbound transport is wrapped by default.
//! - `Environment::Development` and `Environment::Test` bootstrap produce
//!   a factory whose `wrap_binary` returns `None` (bare transport),
//!   preserving existing test determinism.
//!
//! The test does not exercise any feature backend (FUSE, crypto, shares,
//! backups, sync, public-links, notifications, audit, plugins) — it only
//! inspects the transport factory decision exposed on `RuntimeShell`.
//!
//! Deterministic timing: the production factory uses `SystemClock` and
//! `ThreadSleepWaiter`, but this test never calls `.execute()` on the
//! resulting wrapper, so no real sleep can occur. Integration tests that
//! do need to exercise rate-limit / retry timing must construct a
//! `ResilientTransport` directly with a `ManualClock` (see the
//! `pcloud-proto` unit tests for the canonical pattern).

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_config::{ConfigProfile, Environment};
use pcloud_daemon::bootstrap_with_config;
use pcloud_daemon::transport_factory::WrapDecision;
use pcloud_proto::transport::{BinaryApiTransport, TransportConfig};

fn unique_root(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nonce = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "pcloud-daemon-transport-factory-{tag}-{}-{nonce}",
        std::process::id()
    ))
}

fn dummy_binary_transport() -> BinaryApiTransport {
    BinaryApiTransport::new({
        let mut cfg = TransportConfig::dev_plaintext("127.0.0.1", 65535u16, "localhost");
        cfg.connect_timeout = std::time::Duration::from_millis(10);
        cfg.read_timeout = std::time::Duration::from_millis(10);
        cfg.total_request_timeout = std::time::Duration::from_secs(30);
        cfg
    })
}

#[test]
fn production_bootstrap_wraps_transport_by_default() {
    let config = ConfigProfile::secure_defaults(unique_root("prod"), Environment::Production);
    let runtime = bootstrap_with_config(config).expect("production bootstrap should succeed");

    let factory = &runtime.transport_factory;
    assert_eq!(
        factory.decision(),
        WrapDecision::Wrap,
        "production bootstrap must wrap outbound transports"
    );
    assert_eq!(factory.environment(), Environment::Production);

    let wrapped = factory
        .wrap_binary(dummy_binary_transport())
        .expect("secure-default resilience policy is valid");
    assert!(
        wrapped.is_some(),
        "production factory must produce a wrapped binary transport"
    );
}

#[test]
fn development_bootstrap_does_not_wrap_transport() {
    let config = ConfigProfile::secure_defaults(unique_root("dev"), Environment::Development);
    let runtime = bootstrap_with_config(config).expect("development bootstrap should succeed");

    let factory = &runtime.transport_factory;
    assert_eq!(
        factory.decision(),
        WrapDecision::Bare,
        "development bootstrap must keep transports bare for test determinism"
    );
    assert_eq!(factory.environment(), Environment::Development);

    let wrapped = factory
        .wrap_binary(dummy_binary_transport())
        .expect("secure-default resilience policy is valid");
    assert!(
        wrapped.is_none(),
        "development factory must not wrap outbound transports"
    );
}

#[test]
fn test_env_bootstrap_does_not_wrap_transport() {
    let config = ConfigProfile::secure_defaults(unique_root("test"), Environment::Test);
    let runtime = bootstrap_with_config(config).expect("test bootstrap should succeed");

    let factory = &runtime.transport_factory;
    assert_eq!(factory.decision(), WrapDecision::Bare);
    assert_eq!(factory.environment(), Environment::Test);

    let wrapped = factory
        .wrap_binary(dummy_binary_transport())
        .expect("secure-default resilience policy is valid");
    assert!(wrapped.is_none());
}
