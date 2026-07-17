//! Public folder API coverage for date parsing and server-hint variants.

use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Mutex},
};

use pcloud_proto::{
    EncodedRequest, FolderApi,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    response::Value,
};

#[derive(Debug, Clone)]
struct ScriptedTransport {
    responses: Arc<Mutex<VecDeque<Value>>>,
    hints: Arc<Mutex<Vec<String>>>,
}

impl ScriptedTransport {
    fn new(responses: impl IntoIterator<Item = Value>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().collect())),
            hints: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl ProtocolTransport for ScriptedTransport {
    type Error = io::Error;

    fn execute(&self, _request: &EncodedRequest) -> Result<Value, Self::Error> {
        self.responses
            .lock()
            .expect("responses should lock")
            .pop_front()
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "missing response"))
    }
}

impl ApiServerHintConsumer for ScriptedTransport {
    fn apply_api_server_hint(&self, api_server: &str) {
        self.hints
            .lock()
            .expect("hints should lock")
            .push(api_server.to_owned());
    }
}

fn file_entry(name: &str, modified: &str) -> Value {
    Value::Hash(vec![
        ("name".to_owned(), Value::String(name.to_owned())),
        ("isfolder".to_owned(), Value::Bool(false)),
        ("fileid".to_owned(), Value::Number(1)),
        ("modified".to_owned(), Value::String(modified.to_owned())),
    ])
}

fn listing_response(contents: Vec<Value>, api_server: Value) -> Value {
    Value::Hash(vec![
        ("result".to_owned(), Value::Number(0)),
        (
            "metadata".to_owned(),
            Value::Hash(vec![
                ("folderid".to_owned(), Value::Number(1)),
                ("name".to_owned(), Value::String("root".to_owned())),
                ("contents".to_owned(), Value::Array(contents)),
            ]),
        ),
        ("apiserver".to_owned(), api_server),
    ])
}

#[test]
fn folder_dates_accept_valid_rfc2822_and_reject_malformed_server_values() {
    let valid = [
        "Thu, 01 Jan 1970 00:00:00 +0000",
        "Thu, 01 Jan 1970 01:00:00 +0100",
        "Wed, 31 Dec 1969 23:00:00 -0100",
        "Thu, 01 February 2024 00:00:00 +0000",
        "Fri, 01 Mar 2024 00:00:00 +0000",
        "Mon, 01 Apr 2024 00:00:00 +0000",
        "Wed, 01 May 2024 00:00:00 +0000",
        "Sat, 01 Jun 2024 00:00:00 +0000",
        "Mon, 01 Jul 2024 00:00:00 +0000",
        "Thu, 01 Aug 2024 00:00:00 +0000",
        "Sun, 01 Sep 2024 00:00:00 +0000",
        "Tue, 01 Oct 2024 00:00:00 +0000",
        "Fri, 01 Nov 2024 00:00:00 +0000",
        "Sun, 01 Dec 2024 00:00:00 +0000",
        "1700000000",
    ];
    let invalid = [
        "",
        "Wed,",
        "Wed, xx Jan 2024 00:00:00 +0000",
        "Wed, 01",
        "Wed, 01 X 2024 00:00:00 +0000",
        "Wed, 01 Qqq 2024 00:00:00 +0000",
        "Wed, 01 Jan nope 00:00:00 +0000",
        "Wed, 01 Jan 2024 bad +0000",
        "Wed, 01 Jan 2024 00:00 +0000",
        "Wed, 01 Jan 2024 00:00:00:00 +0000",
        "Wed, 01 Jan 2024 24:00:00 +0000",
        "Wed, 01 Jan 2024 00:60:00 +0000",
        "Wed, 01 Jan 2024 00:00:60 +0000",
        "Wed, 00 Jan 2024 00:00:00 +0000",
        "Wed, 30 Feb 2024 00:00:00 +0000",
        "Wed, 29 Feb 2023 00:00:00 +0000",
        "Wed, 01 Jan 2024 00:00:00 UTC",
        "Wed, 01 Jan 2024 00:00:00 *0000",
        "Wed, 01 Jan 2024 00:00:00 +0é0",
        "Wed, 01 Jan 2024 00:00:00 +2400",
        "Wed, 01 Jan 2024 00:00:00 +0060",
        "Wed, 31 Dec 1969 23:59:59 +0000",
    ];
    let mut contents = Vec::new();
    contents.extend(
        valid
            .iter()
            .enumerate()
            .map(|(index, date)| file_entry(&format!("valid-{index}"), date)),
    );
    contents.extend(
        invalid
            .iter()
            .enumerate()
            .map(|(index, date)| file_entry(&format!("invalid-{index}"), date)),
    );

    let transport = ScriptedTransport::new([listing_response(
        contents,
        Value::Hash(vec![(
            "binapi".to_owned(),
            Value::Array(vec![Value::String("bineapi-array.example".to_owned())]),
        )]),
    )]);
    let hints = Arc::clone(&transport.hints);
    let listing = FolderApi::new(transport)
        .list_folder_contents_by_path("token", "/")
        .expect("well-formed listing should parse");

    assert!(
        listing.entries[..valid.len()]
            .iter()
            .all(|entry| entry.modified.is_some())
    );
    assert!(
        listing.entries[valid.len()..]
            .iter()
            .all(|entry| entry.modified.is_none())
    );
    assert_eq!(listing.entries[0].modified, Some(0));
    assert_eq!(listing.entries[1].modified, Some(0));
    assert_eq!(listing.entries[2].modified, Some(0));
    assert_eq!(listing.entries[14].modified, Some(1_700_000_000));
    assert_eq!(
        hints.lock().expect("hints should lock").as_slice(),
        ["bineapi-array.example"]
    );
}

#[test]
fn folder_api_accepts_nested_scalar_api_server_hint() {
    let transport = ScriptedTransport::new([listing_response(
        Vec::new(),
        Value::Hash(vec![(
            "binapi".to_owned(),
            Value::String("bineapi-scalar.example".to_owned()),
        )]),
    )]);
    let hints = Arc::clone(&transport.hints);
    FolderApi::new(transport)
        .list_folder_contents_by_path("token", "/")
        .expect("nested scalar hint should parse");
    assert_eq!(
        hints.lock().expect("hints should lock").as_slice(),
        ["bineapi-scalar.example"]
    );
}
