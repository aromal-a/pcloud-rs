use std::io::{Read, Write};
use std::net::{IpAddr, Shutdown, TcpStream};
use std::thread;
use std::time::Duration;

use pcloud_webdav::{
    BackendEntry, BackendError, IpcBackend, ListenerBinding, PutOutcome, ServerConfig, TcpServer,
};

#[derive(Default)]
struct Backend {
    writes: usize,
}

impl IpcBackend for Backend {
    fn list_folder(&self, _path: &str) -> Result<Vec<BackendEntry>, BackendError> {
        Ok(Vec::new())
    }

    fn stat(&self, path: &str) -> Result<BackendEntry, BackendError> {
        Ok(BackendEntry {
            name: path.rsplit('/').next().unwrap_or("").into(),
            is_collection: true,
            content_length: None,
            last_modified: None,
            content_type: None,
        })
    }

    fn get_file(&self, _path: &str) -> Result<Vec<u8>, BackendError> {
        Ok(Vec::new())
    }

    fn put_file(&mut self, _path: &str, bytes: &[u8]) -> Result<PutOutcome, BackendError> {
        self.writes += bytes.len();
        Ok(PutOutcome::Created)
    }

    fn delete(&mut self, _path: &str) -> Result<(), BackendError> {
        Ok(())
    }

    fn mkdir(&mut self, _path: &str) -> Result<(), BackendError> {
        Ok(())
    }
}

fn config() -> ServerConfig {
    ServerConfig {
        binding: ListenerBinding::LocalTcp {
            host: IpAddr::from([127, 0, 0, 1]),
            port: 0,
        },
        max_put_body_bytes: 4_096,
        allow_writes: true,
    }
}

fn exchange(parts: &[&[u8]], shutdown_write: bool) -> String {
    let server = TcpServer::bind(config()).unwrap();
    let addr = server.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let mut backend = Backend::default();
        server.serve_one(&mut backend).unwrap();
        backend.writes
    });
    let mut client = TcpStream::connect(addr).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    for (index, part) in parts.iter().enumerate() {
        client.write_all(part).unwrap();
        client.flush().unwrap();
        if index + 1 != parts.len() {
            thread::sleep(Duration::from_millis(10));
        }
    }
    if shutdown_write {
        client.shutdown(Shutdown::Write).unwrap();
    }
    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    let _ = worker.join().unwrap();
    response
}

#[test]
fn stopped_run_and_fragmented_body_follow_server_contract() {
    let server = TcpServer::bind(config()).unwrap();
    server
        .stop_handle()
        .store(true, std::sync::atomic::Ordering::Release);
    assert!(server.run(&mut Backend::default()).is_ok());

    let response = exchange(
        &[
            b"PUT /file HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\n\r\n",
            b"data",
        ],
        false,
    );
    assert!(response.starts_with("HTTP/1.1 201"), "{response}");
}

#[test]
fn malformed_wire_inputs_return_bounded_http_errors() {
    let incomplete_header = exchange(&[b"GET / HTTP/1.1\r\nHost: localhost"], true);
    assert!(
        incomplete_header.starts_with("HTTP/1.1 431"),
        "{incomplete_header}"
    );

    let huge = format!("GET / HTTP/1.1\r\nX-Fill: {}", "x".repeat(17_000));
    let huge_header = exchange(&[huge.as_bytes()], true);
    assert!(huge_header.starts_with("HTTP/1.1 431"), "{huge_header}");

    let bad_length = exchange(
        &[b"PUT /file HTTP/1.1\r\nContent-Length: nope\r\n\r\n"],
        false,
    );
    assert!(bad_length.starts_with("HTTP/1.1 400"), "{bad_length}");

    let short_body = exchange(&[b"PUT /file HTTP/1.1\r\nContent-Length: 4\r\n\r\nx"], true);
    assert!(short_body.starts_with("HTTP/1.1 400"), "{short_body}");

    let malformed_request = exchange(&[b"not-http\r\n\r\n"], false);
    assert!(
        malformed_request.starts_with("HTTP/1.1 400"),
        "{malformed_request}"
    );
}
