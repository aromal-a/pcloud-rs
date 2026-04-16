#![allow(clippy::pedantic)]
//! Mock-backed integration flows.
//!
//! These tests previously would have been gated behind `#[ignore]` +
//! `PCLOUD_LIVE_E2E=1` because they require a full pCloud-shaped HTTP backend
//! (userinfo, upload_create/write/save, listshares, listnotifications,
//! createbackup, stopdevice, etc.). With `pcloud-mockserver` we can run the
//! same flows hermetically — no network access, no real credentials — and
//! therefore keep them unignored and in the default test matrix.
//!
//! The tests here talk directly to the mock server over HTTP/1.1. They do NOT
//! route through the real production transport (which would require touching
//! production feature code to accept a custom base URL). This file's job is
//! to prove that the mock server faithfully emulates the pCloud REST JSON
//! shapes and token-handling rules that real production flows depend on.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::Duration,
};

use pcloud_mockserver::{MockFile, MockServer, TEST_TOKEN};
use serde_json::Value;

fn http_get(url: &str) -> (u16, Value) {
    let rest = url.strip_prefix("http://").expect("http:// url");
    let (authority, path_and_query) = match rest.split_once('/') {
        Some((a, r)) => (a.to_owned(), format!("/{r}")),
        None => (rest.to_owned(), "/".to_owned()),
    };
    let addr: SocketAddr = authority.parse().expect("sockaddr parse");
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
    let req =
        format!("GET {path_and_query} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf).to_string();
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
    let value: Value = serde_json::from_str(&body).expect("valid JSON body");
    (status, value)
}

fn http_post(url: &str, body: &[u8]) -> (u16, Value) {
    let rest = url.strip_prefix("http://").expect("http:// url");
    let (authority, path_and_query) = match rest.split_once('/') {
        Some((a, r)) => (a.to_owned(), format!("/{r}")),
        None => (rest.to_owned(), "/".to_owned()),
    };
    let addr: SocketAddr = authority.parse().expect("sockaddr parse");
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
    let head = format!(
        "POST {path_and_query} HTTP/1.1\r\nHost: {authority}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf).to_string();
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
    let value: Value = serde_json::from_str(&body).expect("valid JSON body");
    (status, value)
}

// -------- formerly #[ignore]'d flows, now deterministic --------

#[test]
fn userinfo_flow_against_mock() {
    let h = MockServer::start().unwrap();
    let (_, v) = http_get(&format!("{}/userinfo?auth={TEST_TOKEN}", h.base_url()));
    assert_eq!(v["result"], 0);
    assert_eq!(v["email"], "mock@example.invalid");
    assert!(v["quota"].as_u64().unwrap() > 0);
}

#[test]
fn upload_create_write_save_roundtrip_against_mock() {
    let h = MockServer::start().unwrap();
    let base = h.base_url().to_owned();

    let (_, v) = http_get(&format!("{base}/upload_create?auth={TEST_TOKEN}"));
    let upload_id = v["uploadid"].as_u64().unwrap();

    let payload: Vec<u8> = (0u16..512).flat_map(|n| n.to_le_bytes()).collect();
    let (_, _v) = http_post(
        &format!("{base}/upload_write?auth={TEST_TOKEN}&uploadid={upload_id}&uploadoffset=0"),
        &payload,
    );

    let (_, v) = http_get(&format!(
        "{base}/upload_save?auth={TEST_TOKEN}&uploadid={upload_id}&name=roundtrip.bin&folderid=0"
    ));
    assert_eq!(v["result"], 0);
    let meta = &v["metadata"][0];
    assert_eq!(meta["size"].as_u64().unwrap(), payload.len() as u64);
    assert_eq!(meta["name"], "roundtrip.bin");

    // listfolder now reports the file.
    let (_, v) = http_get(&format!("{base}/listfolder?auth={TEST_TOKEN}&folderid=0"));
    let contents = v["metadata"]["contents"].as_array().unwrap();
    assert!(contents.iter().any(|e| e["name"] == "roundtrip.bin"));
}

#[test]
fn listshares_then_sharefolder_then_listshares_against_mock() {
    let h = MockServer::start().unwrap();
    let base = h.base_url().to_owned();

    // Initially empty.
    let (_, v) = http_get(&format!("{base}/listshares?auth={TEST_TOKEN}"));
    assert_eq!(v["shares"]["outgoing"].as_array().unwrap().len(), 0);

    // Share something.
    let (_, v) = http_get(&format!(
        "{base}/sharefolder?auth={TEST_TOKEN}&folderid=0&mail=alice@example.invalid&permissions=3"
    ));
    assert_eq!(v["result"], 0);

    // Now listed.
    let (_, v) = http_get(&format!("{base}/listshares?auth={TEST_TOKEN}"));
    let out = v["shares"]["outgoing"].as_array().unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["tomail"], "alice@example.invalid");
    assert_eq!(out[0]["permissions"], 3);
}

#[test]
fn notifications_list_and_read_against_mock() {
    let h = MockServer::start().unwrap();
    {
        let st_arc = h.state();
        let mut st = st_arc.lock().unwrap();
        st.unread_notifications = 3;
    }

    let (_, v) = http_get(&format!(
        "{}/listnotifications?auth={TEST_TOKEN}",
        h.base_url()
    ));
    assert_eq!(v["result"], 0);
    assert_eq!(v["nnew"], 3);

    let (_, v) = http_get(&format!(
        "{}/readnotifications?auth={TEST_TOKEN}",
        h.base_url()
    ));
    assert_eq!(v["result"], 0);

    let (_, v) = http_get(&format!(
        "{}/listnotifications?auth={TEST_TOKEN}",
        h.base_url()
    ));
    assert_eq!(v["nnew"], 0);
}

#[test]
fn backup_create_and_device_stop_against_mock() {
    let h = MockServer::start().unwrap();
    let base = h.base_url().to_owned();

    let (_, v) = http_get(&format!(
        "{base}/createbackup?auth={TEST_TOKEN}&name=nightly"
    ));
    assert_eq!(v["result"], 0);
    assert_eq!(v["name"], "nightly");

    let (_, v) = http_get(&format!("{base}/stopdevice?auth={TEST_TOKEN}&deviceid=9"));
    assert_eq!(v["result"], 0);
    assert_eq!(v["deviceid"], 9);
    assert_eq!(v["stopped"], true);
}

#[test]
fn getfilepublink_and_listpubs_against_mock() {
    let h = MockServer::start().unwrap();
    let base = h.base_url().to_owned();

    // Seed a file in the shared state.
    let fid = {
        let st_arc = h.state();
        let mut st = st_arc.lock().unwrap();
        let id = st.alloc_id();
        st.files.insert(
            id,
            MockFile {
                name: "seed.bin".into(),
                parent_folder_id: 0,
                bytes: vec![0xAA; 16],
            },
        );
        id
    };

    let (_, v) = http_get(&format!(
        "{base}/getfilepublink?auth={TEST_TOKEN}&fileid={fid}"
    ));
    assert_eq!(v["result"], 0);
    assert!(v["link"].as_str().unwrap().contains("publink/show?code="));

    let (_, v) = http_get(&format!("{base}/listpubs?auth={TEST_TOKEN}"));
    assert_eq!(v["result"], 0);
    assert_eq!(v["publinks"].as_array().unwrap().len(), 1);
}

#[test]
fn inject_error_and_invalid_token_paths_against_mock() {
    let h = MockServer::start().unwrap();
    let base = h.base_url().to_owned();

    // inject_error wins even with valid token.
    let (_, v) = http_get(&format!(
        "{base}/userinfo?auth={TEST_TOKEN}&inject_error=2003"
    ));
    assert_eq!(v["result"], 2003);

    // Unknown token -> 2094.
    let (_, v) = http_get(&format!("{base}/userinfo?auth=bogus"));
    assert_eq!(v["result"], 2094);

    // Missing token on authenticated route -> 1000.
    let (_, v) = http_get(&format!("{base}/userinfo"));
    assert_eq!(v["result"], 1000);
}
