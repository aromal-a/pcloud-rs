#![allow(clippy::pedantic)]
//! Integration tests for pcloud-ipc peer authorization and protocol framing.
//!
//! Covers:
//! - IpcServer rejects mismatched peer uids (local IPC owner-only rule).
//! - IpcServer accepts the owning uid.
//! - Request/response protocol frames round-trip for every Method variant.
//! - Malformed frames are rejected with the proper ProtocolError variant.
//! - Oversized payloads are rejected.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_ipc::{
    IpcServer, Method, PeerIdentity, Request, Response, ResponseStatus, decode_request,
    decode_response, encode_request_bare as encode_request, encode_response,
    protocol::{IPC_PROTOCOL_VERSION, MAX_IPC_PAYLOAD_LEN, ProtocolError},
};
use proptest::prelude::*;

#[test]
fn server_rejects_non_owner_peer() {
    let server = IpcServer::new(1000);
    let peer = PeerIdentity { uid: 1001, pid: 42 };
    assert!(!server.authorize_peer(&peer));
}

#[test]
fn server_rejects_root_peer_when_owner_is_user() {
    let server = IpcServer::new(1000);
    let root_peer = PeerIdentity { uid: 0, pid: 1 };
    assert!(!server.authorize_peer(&root_peer));
}

#[test]
fn server_accepts_owner_peer() {
    let server = IpcServer::new(1000);
    let peer = PeerIdentity {
        uid: 1000,
        pid: 999,
    };
    assert!(server.authorize_peer(&peer));
}

#[test]
fn decode_request_rejects_version_mismatch() {
    // Build a frame with a bogus version field (0xFF 0xFF).
    let payload = serde_json::to_vec(&Request::Plain {
        method: Method::GetStatus,
    })
    .unwrap();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0xFFFFu16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&payload);

    let err = decode_request(&bytes).expect_err("version mismatch should fail");
    assert!(matches!(
        err,
        ProtocolError::VersionMismatch { expected, actual }
            if expected == IPC_PROTOCOL_VERSION && actual == 0xFFFF
    ));
}

#[test]
fn decode_request_rejects_truncated_header() {
    let err = decode_request(&[0u8, 0, 0, 0]).expect_err("short header should fail");
    assert!(matches!(err, ProtocolError::TruncatedHeader));
}

#[test]
fn decode_request_rejects_len_mismatch() {
    // Claim payload length 100 but only include 4 bytes.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&100u32.to_le_bytes());
    bytes.extend_from_slice(&IPC_PROTOCOL_VERSION.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&[0, 0, 0, 0]);
    let err = decode_request(&bytes).expect_err("length mismatch should fail");
    assert!(matches!(err, ProtocolError::PayloadTooLarge));
}

#[test]
fn decode_response_rejects_malformed_json() {
    // Valid header, junk JSON payload.
    let bogus = b"\x7B\x7B\x7B\x7B"; // four opening braces
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(bogus.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&IPC_PROTOCOL_VERSION.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(bogus);
    let err = decode_response(&bytes).expect_err("malformed json should fail");
    assert!(matches!(err, ProtocolError::Codec(_)));
}

#[test]
fn encode_rejects_oversized_payload() {
    // Build a Request with an enormous embedded string that crosses the payload cap.
    let big = "A".repeat(MAX_IPC_PAYLOAD_LEN + 10);
    let request = Request::PasswordSubmission {
        username: "user".to_owned(),
        value: big,
    };
    let err = encode_request(&request).expect_err("oversized payload must be rejected");
    assert!(matches!(err, ProtocolError::PayloadTooLarge));
}

#[test]
fn server_decode_request_reads_authorized_payload() {
    let server = IpcServer::new(1000);
    let bytes = encode_request(&Request::Plain {
        method: Method::GetHealth,
    })
    .expect("encode should succeed");
    let decoded = server
        .decode_request(&bytes)
        .expect("decode should succeed");
    assert!(matches!(
        decoded,
        Request::Plain {
            method: Method::GetHealth
        }
    ));
    let envelope = server
        .decode_envelope(&bytes)
        .expect("envelope decode should succeed");
    assert!(envelope.traceparent().is_none());
}

#[test]
fn encode_status_frame_round_trips() {
    let server = IpcServer::new(0);
    let bytes = server
        .encode_status(ResponseStatus::Unauthorized, "nope")
        .expect("encode should succeed");
    let frame = decode_response(&bytes).expect("response should decode");
    assert_eq!(frame.payload.status, ResponseStatus::Unauthorized);
    assert_eq!(frame.payload.message, "nope");
}

fn all_methods() -> Vec<Method> {
    vec![
        Method::GetStatus,
        Method::GetHealth,
        Method::GetPending,
        Method::GetSyncRoots,
        Method::ListPublicLinks,
        Method::ListUploadLinks,
        Method::GetUserInfo,
        Method::PauseSync,
        Method::ResumeSync,
        Method::LoginBegin,
        Method::Logout,
        Method::SendTwoFactorSms,
        Method::SendTwoFactorNotification,
        Method::SubmitPassword,
        Method::SubmitTwoFactorCode,
        Method::UnlockCrypto,
        Method::LockCrypto,
        Method::Shutdown,
        Method::SetAuthPersistence,
    ]
}

#[test]
fn every_method_round_trips_plain_request() {
    for method in all_methods() {
        let bytes = encode_request(&Request::Plain { method }).expect("encode should succeed");
        let frame = decode_request(&bytes).expect("decode should succeed");
        match frame.payload.request {
            Request::Plain { method: decoded } => assert_eq!(decoded, method),
            other => panic!("unexpected variant {:?}", other),
        }
    }
}

proptest! {
    #[test]
    fn prop_random_bytes_never_produce_spurious_authorization(
        uid in any::<u32>(),
        owner_uid in any::<u32>()
    ) {
        let server = IpcServer::new(owner_uid);
        let peer = PeerIdentity { uid, pid: 1 };
        prop_assert_eq!(server.authorize_peer(&peer), uid == owner_uid);
    }

    #[test]
    fn prop_random_frames_do_not_panic(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        let _ = decode_request(&bytes);
        let _ = decode_response(&bytes);
    }

    #[test]
    fn prop_response_round_trip(
        status in prop_oneof![
            Just(ResponseStatus::Ok),
            Just(ResponseStatus::InvalidRequest),
            Just(ResponseStatus::Unauthorized),
            Just(ResponseStatus::Conflict),
            Just(ResponseStatus::Unavailable),
            Just(ResponseStatus::InternalError),
        ],
        message in ".{0,128}"
    ) {
        let response = Response { status: status.clone(), message: message.clone() };
        let bytes = encode_response(&response).expect("encode should succeed");
        let frame = decode_response(&bytes).expect("decode should succeed");
        prop_assert_eq!(frame.payload.status, status);
        prop_assert_eq!(frame.payload.message, message);
    }
}
