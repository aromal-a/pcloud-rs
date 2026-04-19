#![allow(clippy::pedantic)]
//! Integration tests covering the optional SHA-256 integrity
//! verification path added to [`fetch_download_verified`]. The test
//! exercises a local `TcpListener` that serves a canned HTTP/1.1
//! response so the checksum path is verified end-to-end without
//! requiring TLS or live pCloud access.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

use pcloud_proto::{
    HttpDownloadConfig, HttpDownloadError, ResumableOutcome, SignedDownload,
    fetch_download_resumable, fetch_download_verified, fetch_download_verified_streaming,
};
use sha2::{Digest, Sha256};

/// Sink wrapper that counts `write_all` invocations so tests can
/// assert that the streaming download path writes the body in multiple
/// chunks rather than a single buffered flush.
struct CountingSink {
    inner: Vec<u8>,
    writes: usize,
}

impl CountingSink {
    fn new() -> Self {
        Self {
            inner: Vec::new(),
            writes: 0,
        }
    }
}

impl Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.writes += 1;
        self.inner.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn spawn_server(body: Vec<u8>) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = vec![0u8; 1024];
        let _ = stream.read(&mut request).expect("read");
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes()).expect("write headers");
        stream.write_all(&body).expect("write body");
    });
    address
}

fn test_config() -> HttpDownloadConfig {
    HttpDownloadConfig {
        use_tls: false,
        connect_timeout: Duration::from_secs(2),
        read_timeout: Duration::from_secs(2),
        write_timeout: Duration::from_secs(2),
        total_request_timeout: Duration::from_secs(30),
        max_header_bytes: 4096,
        max_body_bytes: 4096,
        bandwidth_pacer: None,
    }
}

fn signed(address: std::net::SocketAddr) -> SignedDownload {
    SignedDownload {
        host: address.ip().to_string(),
        port: Some(address.port()),
        path: "/get/abc/report.txt".to_owned(),
        dwltag: None,
        range: None,
    }
}

#[test]
fn fetch_download_verified_accepts_matching_sha256() {
    let payload = b"hello integrity world".to_vec();
    let expected: [u8; 32] = Sha256::digest(&payload).into();
    let address = spawn_server(payload.clone());

    let bytes = fetch_download_verified(&signed(address), &test_config(), Some(expected))
        .expect("download should succeed");
    assert_eq!(bytes, payload);
}

#[test]
fn fetch_download_verified_rejects_bit_flipped_body() {
    let original = b"hello integrity world".to_vec();
    let expected: [u8; 32] = Sha256::digest(&original).into();

    // Flip a single bit in the served body to simulate on-wire
    // corruption or a malicious tamper. The SHA-256 expected above
    // is still the digest of the pristine payload.
    let mut tampered = original.clone();
    tampered[0] ^= 0x01;

    let address = spawn_server(tampered);
    let err = fetch_download_verified(&signed(address), &test_config(), Some(expected))
        .expect_err("mismatched sha256 must fail");

    match err {
        HttpDownloadError::IntegrityMismatch {
            ref expected,
            ref actual,
        } => {
            assert_ne!(expected, actual);
            assert_eq!(expected.len(), 64);
            assert_eq!(actual.len(), 64);
            assert!(err.is_retryable());
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn fetch_download_verified_without_expected_sha_skips_check() {
    let payload = b"no verification requested".to_vec();
    let address = spawn_server(payload.clone());
    let bytes = fetch_download_verified(&signed(address), &test_config(), None)
        .expect("download should succeed");
    assert_eq!(bytes, payload);
}

fn large_config(max_body: usize) -> HttpDownloadConfig {
    HttpDownloadConfig {
        use_tls: false,
        connect_timeout: Duration::from_secs(5),
        read_timeout: Duration::from_secs(5),
        write_timeout: Duration::from_secs(5),
        total_request_timeout: Duration::from_secs(60),
        max_header_bytes: 4096,
        max_body_bytes: max_body,
        bandwidth_pacer: None,
    }
}

fn deterministic_payload(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        out.push((i as u8).wrapping_mul(31).wrapping_add(7));
    }
    out
}

#[test]
fn streaming_download_writes_to_writer() {
    let payload = deterministic_payload(1024 * 1024);
    let expected: [u8; 32] = Sha256::digest(&payload).into();
    let address = spawn_server(payload.clone());

    let mut sink: Vec<u8> = Vec::new();
    let written = fetch_download_verified_streaming(
        &signed(address),
        &large_config(2 * 1024 * 1024),
        Some(expected),
        &mut sink,
    )
    .expect("streaming download should succeed");

    assert_eq!(written, payload.len() as u64);
    assert_eq!(sink.len(), payload.len());
    assert_eq!(sink, payload);
}

#[test]
fn streaming_download_verifies_sha_at_eof() {
    let pristine = deterministic_payload(128 * 1024);
    let expected: [u8; 32] = Sha256::digest(&pristine).into();

    // Flip the last byte so the SHA computed at EOF diverges from the
    // expected digest of the pristine payload.
    let mut tampered = pristine.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0x01;

    let address = spawn_server(tampered);
    let mut sink: Vec<u8> = Vec::new();
    let err = fetch_download_verified_streaming(
        &signed(address),
        &large_config(1024 * 1024),
        Some(expected),
        &mut sink,
    )
    .expect_err("tampered tail must fail verification");

    assert!(matches!(err, HttpDownloadError::IntegrityMismatch { .. }));
    assert!(err.is_retryable());
}

/// Minimal range-aware HTTP/1.1 server. Reads the request headers,
/// parses an optional `Range: bytes=N-` header and serves the
/// appropriate slice of `body`. `announce_accept_ranges` controls
/// whether the response includes an `Accept-Ranges: bytes` header.
fn spawn_range_server(body: Vec<u8>, announce_accept_ranges: bool) -> std::net::SocketAddr {
    spawn_range_server_n(body, announce_accept_ranges, 1)
}

fn spawn_range_server_n(
    body: Vec<u8>,
    announce_accept_ranges: bool,
    connections: usize,
) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr");
    thread::spawn(move || {
        for _ in 0..connections {
            let (mut stream, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(_) => return,
            };
            // Read until end of headers.
            let mut buf = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                let n = stream.read(&mut byte).expect("read request");
                if n == 0 {
                    return;
                }
                buf.push(byte[0]);
                if buf.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let req = String::from_utf8_lossy(&buf).into_owned();
            let mut range_start: Option<usize> = None;
            for line in req.split("\r\n") {
                if let Some(rest) = line
                    .strip_prefix("Range: bytes=")
                    .or_else(|| line.strip_prefix("range: bytes="))
                {
                    let s = rest.split('-').next().unwrap_or("0");
                    range_start = s.trim().parse::<usize>().ok();
                }
            }

            if let Some(start) = range_start {
                let slice = &body[start..];
                let header = if announce_accept_ranges {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                        slice.len(),
                        start,
                        body.len() - 1,
                        body.len()
                    )
                } else {
                    // Server ignores range (no accept-ranges). Reply with 200 full body.
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                };
                stream.write_all(header.as_bytes()).expect("hdr");
                if announce_accept_ranges {
                    stream.write_all(slice).expect("body");
                } else {
                    stream.write_all(&body).expect("body");
                }
            } else {
                // Full body request.
                let mut header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                if announce_accept_ranges {
                    header.push_str("Accept-Ranges: bytes\r\n");
                }
                header.push_str("\r\n");
                stream.write_all(header.as_bytes()).expect("hdr");
                stream.write_all(&body).expect("body");
            }
        }
    });
    address
}

fn unique_temp_path(tag: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    let uniq = format!(
        "pcloudproto-resume-{}-{}-{}.bin",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    p.push(uniq);
    p
}

#[test]
fn resumable_download_picks_up_after_interrupt() {
    let payload = deterministic_payload(200 * 1024);
    let expected: [u8; 32] = Sha256::digest(&payload).into();

    let dest = unique_temp_path("resume-happy");
    // Use .part suffix consistent with implementation.
    let mut part = dest.clone();
    let mut name = part.file_name().unwrap().to_os_string();
    name.push(".part");
    part.set_file_name(name);

    // Simulate a 50% interrupted download by pre-populating .part with
    // the first half of the payload.
    let half = payload.len() / 2;
    std::fs::write(&part, &payload[..half]).expect("seed part file");

    let address = spawn_range_server(payload.clone(), true);
    let outcome =
        fetch_download_resumable(&signed(address), &large_config(512 * 1024), expected, &dest)
            .expect("resume should succeed");

    match outcome {
        ResumableOutcome::Resumed {
            resumed_from,
            bytes_written,
        } => {
            assert_eq!(resumed_from, half as u64);
            assert_eq!(bytes_written, payload.len() as u64);
        }
        other => panic!("unexpected outcome: {other:?}"),
    }

    let contents = std::fs::read(&dest).expect("read dest");
    assert_eq!(contents, payload);
    assert!(!part.exists(), "part file should have been renamed");
    std::fs::remove_file(&dest).ok();
}

#[test]
fn resumable_download_falls_back_to_full_if_no_range_support() {
    let payload = deterministic_payload(64 * 1024);
    let expected: [u8; 32] = Sha256::digest(&payload).into();

    let dest = unique_temp_path("resume-fallback");
    let mut part = dest.clone();
    let mut name = part.file_name().unwrap().to_os_string();
    name.push(".part");
    part.set_file_name(name);
    // Seed a stale prefix.
    std::fs::write(&part, &payload[..32 * 1024]).expect("seed part");

    // Server does NOT advertise Accept-Ranges and serves full body on
    // any GET. Resume path should fall back to full redownload, which
    // costs TWO connections: the probe Range request + the full retry.
    let address = spawn_range_server_n(payload.clone(), false, 2);
    let outcome =
        fetch_download_resumable(&signed(address), &large_config(512 * 1024), expected, &dest)
            .expect("fallback should succeed");

    // The server may answer the first (range) request with 200 → the
    // implementation then issues a second full download. Either way the
    // caller should see a full-redownload-style outcome.
    match outcome {
        ResumableOutcome::FallbackFullRedownload { bytes_written }
        | ResumableOutcome::FullDownload { bytes_written } => {
            assert_eq!(bytes_written, payload.len() as u64);
        }
        other => panic!("unexpected outcome: {other:?}"),
    }

    let contents = std::fs::read(&dest).expect("read dest");
    assert_eq!(contents, payload);
    std::fs::remove_file(&dest).ok();
}

#[test]
fn resumable_download_deletes_part_on_sha_mismatch() {
    let payload = deterministic_payload(32 * 1024);
    let wrong_expected: [u8; 32] = Sha256::digest(b"totally different").into();

    let dest = unique_temp_path("resume-mismatch");
    let address = spawn_range_server(payload.clone(), true);
    let err = fetch_download_resumable(
        &signed(address),
        &large_config(512 * 1024),
        wrong_expected,
        &dest,
    )
    .expect_err("mismatched sha should fail");

    assert!(matches!(err, HttpDownloadError::IntegrityMismatch { .. }));
    // .part must have been removed.
    let mut part = dest.clone();
    let mut name = part.file_name().unwrap().to_os_string();
    name.push(".part");
    part.set_file_name(name);
    assert!(!part.exists(), ".part should be deleted on mismatch");
    assert!(!dest.exists(), "dest should not exist on mismatch");
}

#[test]
fn resumable_download_restarts_if_part_file_corrupted() {
    let payload = deterministic_payload(32 * 1024);
    let expected: [u8; 32] = Sha256::digest(&payload).into();

    let dest = unique_temp_path("resume-corrupt");
    let mut part = dest.clone();
    let mut name = part.file_name().unwrap().to_os_string();
    name.push(".part");
    part.set_file_name(name);

    // Seed .part with garbage of a plausible size — re-hash on resume
    // will produce a digest that mismatches the tail's contribution.
    let garbage = vec![0xFFu8; 16 * 1024];
    std::fs::write(&part, &garbage).expect("seed garbage");

    let address = spawn_range_server(payload.clone(), true);
    let err =
        fetch_download_resumable(&signed(address), &large_config(512 * 1024), expected, &dest)
            .expect_err("corrupt prefix must yield mismatch");

    assert!(matches!(err, HttpDownloadError::IntegrityMismatch { .. }));
    assert!(!part.exists(), ".part should be deleted after mismatch");
}

#[test]
fn streaming_download_never_allocates_full_body() {
    // 512 KiB body versus a 64 KiB streaming read buffer -> the sink
    // must receive at least two separate `write_all` calls, proving the
    // body was not buffered into a single allocation before being
    // handed to the sink.
    let payload = deterministic_payload(512 * 1024);
    let expected: [u8; 32] = Sha256::digest(&payload).into();
    let address = spawn_server(payload.clone());

    let mut sink = CountingSink::new();
    let written = fetch_download_verified_streaming(
        &signed(address),
        &large_config(1024 * 1024),
        Some(expected),
        &mut sink,
    )
    .expect("streaming download should succeed");

    assert_eq!(written, payload.len() as u64);
    assert_eq!(sink.inner, payload);
    assert!(
        sink.writes >= 2,
        "expected chunked writes, got {}",
        sink.writes
    );
}
