#![allow(clippy::pedantic)]
//! Wire-shape and back-compat tests for [`RequestEnvelope`].
//!
//! These guard the contract that:
//! * a modern envelope-wrapped request round-trips with its
//!   `traceparent`,
//! * a bare-`Request` payload from a pre-envelope client still decodes
//!   cleanly into a `RequestEnvelope` with `traceparent: None`,
//! * the `traceparent` field is omitted from the JSON when `None`,
//! * the envelope itself does NOT validate the traceparent string —
//!   anything that's a valid string must round-trip verbatim.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_ipc::{
    Method, Request, RequestEnvelope, decode_request, encode_request, encode_request_bare,
};

#[test]
fn envelope_round_trips_with_traceparent() {
    let traceparent = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_owned();
    let envelope = RequestEnvelope::new(Request::Plain {
        method: Method::GetHealth,
    })
    .with_traceparent(traceparent.clone());

    let bytes = encode_request(&envelope).expect("envelope should encode");
    let frame = decode_request(&bytes).expect("envelope should decode");
    assert!(matches!(
        frame.payload.request,
        Request::Plain {
            method: Method::GetHealth
        }
    ));
    assert_eq!(frame.payload.traceparent(), Some(traceparent.as_str()));
}

#[test]
fn envelope_backward_compat_decodes_bare_request_without_traceparent() {
    // Simulate an old client that emits a bare `Request` JSON body with
    // no envelope wrapping. The framed bytes from `encode_request_bare`
    // intentionally use the same wire format the legacy client would
    // have produced.
    let bytes = encode_request_bare(&Request::Plain {
        method: Method::GetStatus,
    })
    .expect("bare encode should succeed");
    let frame = decode_request(&bytes).expect("bare-request bytes must still decode");
    assert!(matches!(
        frame.payload.request,
        Request::Plain {
            method: Method::GetStatus
        }
    ));
    assert!(frame.payload.traceparent().is_none());

    // And the JSON-only fallback path inside RequestEnvelope::try_from_wire
    // must accept a hand-crafted bare `Request` payload too.
    let bare_json = serde_json::to_vec(&Request::Plain {
        method: Method::GetStatus,
    })
    .expect("bare request serializes");
    let envelope =
        RequestEnvelope::try_from_wire(&bare_json).expect("bare-request payload should fall back");
    assert!(matches!(
        envelope.request,
        Request::Plain {
            method: Method::GetStatus
        }
    ));
    assert!(envelope.traceparent().is_none());
}

#[test]
fn envelope_traceparent_omitted_when_none() {
    let envelope = RequestEnvelope::new(Request::Plain {
        method: Method::GetHealth,
    });
    let json = serde_json::to_string(&envelope).expect("envelope serializes");
    assert!(
        !json.contains("traceparent"),
        "traceparent must be omitted from the wire when None, got: {json}"
    );
    assert!(
        json.contains("\"request\""),
        "request field must be present"
    );
}

#[test]
fn envelope_reject_malformed_traceparent() {
    // The envelope deliberately does NOT validate the traceparent
    // string — downstream observability code parses W3C format. Anything
    // that is a valid Rust `String` must round-trip verbatim.
    for bogus in [
        "",
        "not-a-traceparent",
        "ff-ff-ff",
        "🚀-emoji-content",
        "with whitespace and tabs\there",
    ] {
        let envelope = RequestEnvelope::new(Request::Plain {
            method: Method::GetHealth,
        })
        .with_traceparent(bogus.to_owned());
        let bytes = encode_request(&envelope).expect("any string traceparent must encode");
        let frame = decode_request(&bytes).expect("any string traceparent must decode");
        assert_eq!(frame.payload.traceparent(), Some(bogus));
    }
}

#[test]
fn envelope_from_request_yields_no_traceparent() {
    let envelope: RequestEnvelope = Request::Plain {
        method: Method::GetHealth,
    }
    .into();
    assert!(envelope.traceparent().is_none());
}
