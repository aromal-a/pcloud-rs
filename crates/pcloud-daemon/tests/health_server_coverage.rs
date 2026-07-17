use std::{
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    time::Duration,
};

use pcloud_daemon::health_server::{HealthServerConfig, spawn};

fn unused_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("reserve health address")
        .local_addr()
        .expect("health address")
        .port()
}

fn request(addr: SocketAddr, bytes: &[u8]) -> String {
    let mut stream = TcpStream::connect(addr).expect("connect health server");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    stream.write_all(bytes).expect("write request");
    stream.shutdown(Shutdown::Write).expect("finish request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read response");
    response
}

#[test]
fn live_health_server_handles_http_method_path_and_malformed_inputs() {
    assert!(spawn(HealthServerConfig::default()).unwrap().is_none());
    assert!(
        spawn(HealthServerConfig {
            http_port: 80,
            read_timeout_ms: 10,
        })
        .is_err()
    );

    let port = unused_port();
    let handle = spawn(HealthServerConfig {
        http_port: port,
        read_timeout_ms: 50,
    })
    .expect("spawn health server")
    .expect("enabled health server");
    assert_eq!(handle.port, port);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));

    let live = request(addr, b"GET /livez HTTP/1.0\r\n\r\n");
    assert!(live.starts_with("HTTP/1.0 200 OK"), "{live}");
    assert!(live.ends_with("ok\n"));

    let ready = request(addr, b"GET /readyz HTTP/1.1\r\nHost: localhost\r\n\r\n");
    assert!(ready.starts_with("HTTP/1.0 200 OK"), "{ready}");

    let missing = request(addr, b"GET /missing HTTP/1.0\r\n\r\n");
    assert!(missing.starts_with("HTTP/1.0 404 Not Found"), "{missing}");

    let method = request(addr, b"POST /livez HTTP/1.0\r\n\r\n");
    assert!(
        method.starts_with("HTTP/1.0 405 Method Not Allowed"),
        "{method}"
    );

    let invalid_utf8 = request(addr, &[0xff, 0xfe, b'\n']);
    assert!(invalid_utf8.is_empty());
    let empty = request(addr, b"");
    assert!(empty.is_empty());
}

#[test]
fn duplicate_bind_reports_an_operational_error() {
    let port = unused_port();
    let _first = spawn(HealthServerConfig {
        http_port: port,
        read_timeout_ms: 50,
    })
    .expect("first bind")
    .expect("enabled");
    let error = match spawn(HealthServerConfig {
        http_port: port,
        read_timeout_ms: 50,
    }) {
        Ok(_) => panic!("duplicate bind unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(error.contains("bind"));
}
