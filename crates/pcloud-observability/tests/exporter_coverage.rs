#![cfg(feature = "prometheus-exporter")]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use pcloud_observability::exporter::{ExporterConfig, ExporterSnapshot, spawn};

fn request(addr: std::net::SocketAddr, bytes: &[u8]) -> String {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.write_all(bytes).unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn exporter_public_lifecycle_and_malformed_request_paths_are_executable() {
    let mut config = ExporterConfig::from_env(false);
    config.port = 0;
    let handle = spawn(config, Arc::new(AtomicBool::new(false)), || {
        ExporterSnapshot::new("# HELP coverage fixture\n".to_owned(), false)
    })
    .unwrap();
    let addr = handle.local_addr();

    assert!(request(addr, b"\r\n\r\n").starts_with("HTTP/1.1 400"));
    assert!(
        request(addr, b"POST /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .starts_with("HTTP/1.1 405")
    );
    assert!(
        request(addr, b"GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .contains("# HELP coverage fixture")
    );
    assert!(
        request(addr, b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .starts_with("HTTP/1.1 503")
    );
    assert!(
        request(addr, b"GET /slo HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .contains("slo_not_configured")
    );
    assert!(
        request(addr, b"GET /missing HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .starts_with("HTTP/1.1 404")
    );
    handle.shutdown();
}
