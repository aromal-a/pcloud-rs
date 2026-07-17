use std::collections::HashMap;
use std::io::ErrorKind as IoKind;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use pcloud_resilience::{
    BackoffSchedule, MethodRetryPolicy, RetryClass, RetryPolicy,
    transport::{
        ErrorKind, ResilientTransport, ResilientTransportConfig, TlsError, TransportError,
        TransportErrorClass, TransportOutcome, TransportOutcomeLabel, TransportResponse,
        classify_error, classify_transport_error, is_retryable_io_kind, observe_transport_error,
        observe_transport_latency, parse_retry_after_from_headers, parse_retry_after_header,
    },
};

fn transport(max_attempts: u32, max_total: u32) -> ResilientTransport {
    let retry = RetryPolicy::new(
        max_attempts,
        BackoffSchedule::Fixed {
            delay: Duration::from_millis(1),
        },
    );
    ResilientTransport::new(
        ResilientTransportConfig::new(MethodRetryPolicy::secure_default(retry))
            .with_max_total_attempts(max_total),
    )
    .with_host("coverage.test")
}

async fn no_sleep(_: Duration) {}

#[test]
fn labels_retry_after_and_typed_classification_cover_complete_taxonomy() {
    for (label, expected) in [
        (TransportOutcomeLabel::Success, "success"),
        (TransportOutcomeLabel::Retry, "retry"),
        (TransportOutcomeLabel::GiveUp, "give_up"),
    ] {
        assert_eq!(label.as_str(), expected);
        observe_transport_latency("coverage.test", label, 0.001);
    }
    for (class, expected) in [
        (TransportErrorClass::Connect, "connect"),
        (TransportErrorClass::Tls, "tls"),
        (TransportErrorClass::Io, "io"),
        (TransportErrorClass::Response, "response"),
        (TransportErrorClass::BudgetExhausted, "budget_exhausted"),
        (TransportErrorClass::CircuitOpen, "circuit_open"),
    ] {
        assert_eq!(class.as_str(), expected);
        observe_transport_error("coverage.test", class);
    }

    assert_eq!(
        parse_retry_after_header(" 1.5 "),
        Some(Duration::from_millis(1_500))
    );
    assert_eq!(
        parse_retry_after_header("999"),
        Some(Duration::from_secs(300))
    );
    for invalid in [
        "-1",
        "NaN",
        "inf",
        "",
        "bad",
        "Wed 21 Oct 2015 07:28:00 GMT",
    ] {
        assert_eq!(parse_retry_after_header(invalid), None, "{invalid}");
    }
    for month in [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ] {
        let date = format!("Thu, 01 {month} 2099 00:00:00 GMT");
        assert!(parse_retry_after_header(&date).is_some(), "{date}");
    }
    for date in [
        "Thu, 00 Jan 2099 00:00:00 GMT",
        "Thu, 32 Jan 2099 00:00:00 GMT",
        "Thu, 30 Feb 2099 00:00:00 GMT",
        "Thu, 01 Xxx 2099 00:00:00 GMT",
        "Thu, 01 Jan 2099 24:00:00 GMT",
        "Thu, 01 Jan 2099 00:60:00 GMT",
        "Thu, 01 Jan 2099 00:00:60 GMT",
    ] {
        assert_eq!(parse_retry_after_header(date), None, "{date}");
    }
    assert_eq!(
        parse_retry_after_from_headers("HTTP/1.1 429\r\nRetry-After: 2\r\n"),
        Some(Duration::from_secs(2))
    );
    assert_eq!(parse_retry_after_from_headers("HTTP/1.1 200\r\n"), None);

    for kind in [
        IoKind::TimedOut,
        IoKind::ConnectionReset,
        IoKind::BrokenPipe,
        IoKind::ConnectionAborted,
        IoKind::Interrupted,
        IoKind::WouldBlock,
    ] {
        assert!(is_retryable_io_kind(kind));
        assert_eq!(
            classify_transport_error(&TransportError::Io(kind)),
            ErrorKind::Transient
        );
    }
    for kind in [
        IoKind::PermissionDenied,
        IoKind::NotFound,
        IoKind::AlreadyExists,
        IoKind::InvalidInput,
        IoKind::InvalidData,
        IoKind::UnexpectedEof,
        IoKind::Other,
    ] {
        assert!(!is_retryable_io_kind(kind));
        assert_eq!(
            classify_transport_error(&TransportError::Io(kind)),
            ErrorKind::Terminal
        );
    }

    let typed = vec![
        TransportError::Tls(TlsError::InvalidCertificate),
        TransportError::Tls(TlsError::AlertReceived),
        TransportError::Tls(TlsError::NoVersionOrCipher),
        TransportError::Tls(TlsError::InvalidServerName),
        TransportError::Tls(TlsError::Other),
        TransportError::Connect,
        TransportError::Timeout,
        TransportError::Body,
        TransportError::InvalidAddress,
        TransportError::Decode,
        TransportError::ResponseTooLarge,
        TransportError::SocketConfig,
        TransportError::Unknown,
    ];
    for error in typed {
        let expected = classify_transport_error(&error);
        let encoded = TransportResponse::typed_error(error).error.unwrap();
        assert_eq!(classify_error(&encoded), expected, "{encoded}");
    }
    assert_eq!(
        classify_error("pcloud-resilience:typed:not-real:"),
        ErrorKind::Terminal
    );
    assert_eq!(classify_error("legacy free text"), ErrorKind::Terminal);
}

#[test]
fn response_helpers_and_config_invariants_are_stable() {
    let ok = TransportResponse::ok(204);
    assert!(ok.is_success());
    assert!(!ok.is_server_error());
    assert!(!ok.is_rate_limited());
    let mut limited = TransportResponse::ok(429);
    limited.headers.insert("Retry-After".into(), "3".into());
    assert!(limited.is_rate_limited());
    assert_eq!(limited.retry_after(), Some(Duration::from_secs(3)));
    assert!(TransportResponse::ok(500).is_server_error());
    assert!(TransportResponse::transport_error("legacy").error.is_some());
    assert!(
        std::panic::catch_unwind(|| {
            let retry = RetryPolicy::new(
                1,
                BackoffSchedule::Fixed {
                    delay: Duration::ZERO,
                },
            );
            ResilientTransportConfig::new(MethodRetryPolicy::secure_default(retry))
                .with_max_total_attempts(0);
        })
        .is_err()
    );
}

#[tokio::test]
async fn executor_covers_success_terminal_policy_rate_limit_retry_and_budget_paths() {
    assert!(matches!(
        transport(3, 3)
            .execute(
                RetryClass::Idempotent,
                || async { TransportResponse::ok(200) },
                no_sleep,
            )
            .await,
        TransportOutcome::Response(response) if response.status == 200
    ));
    assert!(matches!(
        transport(3, 3)
            .execute(
                RetryClass::Idempotent,
                || async { TransportResponse::ok(404) },
                no_sleep,
            )
            .await,
        TransportOutcome::Response(response) if response.status == 404
    ));
    assert!(matches!(
        transport(3, 3)
            .execute(
                RetryClass::Idempotent,
                || async {
                    TransportResponse::typed_error(TransportError::Tls(
                        TlsError::InvalidCertificate,
                    ))
                },
                no_sleep,
            )
            .await,
        TransportOutcome::Failed(message) if message.contains("Terminal")
    ));

    let attempts = Arc::new(AtomicUsize::new(0));
    let shared = attempts.clone();
    assert!(matches!(
        transport(3, 3)
            .execute(
                RetryClass::Idempotent,
                move || {
                    let attempt = shared.fetch_add(1, Ordering::SeqCst);
                    async move { TransportResponse::ok(if attempt == 0 { 500 } else { 200 }) }
                },
                no_sleep,
            )
            .await,
        TransportOutcome::Response(response) if response.status == 200
    ));
    assert_eq!(attempts.load(Ordering::SeqCst), 2);

    assert!(matches!(
        transport(3, 3)
            .execute(
                RetryClass::Mutation,
                || async { TransportResponse::ok(500) },
                no_sleep,
            )
            .await,
        TransportOutcome::Failed(message) if message.contains("Method policy")
    ));

    let rate_attempts = Arc::new(AtomicUsize::new(0));
    let shared = rate_attempts.clone();
    assert!(matches!(
        transport(2, 2)
            .execute(
                RetryClass::Idempotent,
                move || {
                    let attempt = shared.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if attempt == 0 {
                            TransportResponse {
                                status: 429,
                                headers: HashMap::from([("retry-after".into(), "0".into())]),
                                error: None,
                            }
                        } else {
                            TransportResponse::ok(200)
                        }
                    }
                },
                no_sleep,
            )
            .await,
        TransportOutcome::Response(response) if response.status == 200
    ));

    assert!(matches!(
        transport(10, 2)
            .execute(
                RetryClass::Idempotent,
                || async { TransportResponse::ok(429) },
                no_sleep,
            )
            .await,
        TransportOutcome::Failed(message) if message.contains("budget")
    ));

    let transient_attempts = Arc::new(AtomicUsize::new(0));
    let shared = transient_attempts.clone();
    assert!(matches!(
        transport(3, 3)
            .execute(
                RetryClass::Idempotent,
                move || {
                    let attempt = shared.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if attempt == 0 {
                            TransportResponse::typed_error(TransportError::Connect)
                        } else {
                            TransportResponse::ok(200)
                        }
                    }
                },
                no_sleep,
            )
            .await,
        TransportOutcome::Response(response) if response.status == 200
    ));

    assert!(matches!(
        transport(3, 3)
            .execute(
                RetryClass::Unknown,
                || async { TransportResponse::ok(0) },
                no_sleep,
            )
            .await,
        TransportOutcome::Failed(message) if message.contains("Method policy")
    ));
}
