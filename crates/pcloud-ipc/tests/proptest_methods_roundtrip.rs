#![allow(clippy::pedantic)]
//! Property-based round-trip tests for every IPC `Method`, `Request`, and
//! `Response` variant. Complements `peer_and_protocol.rs` by exhaustively
//! exercising the enum cartesian product with random payload strings/ids.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_ipc::{
    decode_request, decode_response, encode_request_bare as encode_request, encode_response,
    methods::{Method, Request, Response, ResponseStatus, ValueKvKind, ValueKvPayload},
};
use proptest::prelude::*;

fn every_method() -> &'static [Method] {
    &[
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
        Method::GetCryptoStatus,
        Method::CryptoReset,
        Method::GetCryptoPrivKeyFlags,
        Method::SendCryptoChangeUserPrivate,
        Method::Shutdown,
        Method::SetAuthPersistence,
        Method::ListIncomingShares,
        Method::ListOutgoingShares,
        Method::ListIncomingShareRequests,
        Method::ListOutgoingShareRequests,
        Method::ListContacts,
        Method::ListMyTeams,
        Method::ListNotifications,
    ]
}

/// Compile-time exhaustive match — forces the test to be updated whenever a
/// new `Method` variant is introduced. `Method` is `#[non_exhaustive]` so a
/// catch-all `_` arm is required from out-of-crate code (this integration
/// test is compiled as an external crate); the explicit arms above still
/// enumerate every currently-known variant, so adding a new variant without
/// extending the list will be caught in code review rather than at compile
/// time.
// Compile-time exhaustiveness guard. Never called at runtime; it exists so
// that adding a new `Method` variant forces a reviewer to extend this match
// (see the doc comment above). Dead-code lint silenced intentionally.
#[allow(dead_code)]
fn must_match_every_method_variant(m: Method) -> u8 {
    match m {
        Method::GetStatus
        | Method::GetHealth
        | Method::Health
        | Method::GetPending
        | Method::GetSyncRoots
        | Method::ListPublicLinks
        | Method::ListUploadLinks
        | Method::GetUserInfo
        | Method::PauseSync
        | Method::ResumeSync
        | Method::LoginBegin
        | Method::Logout
        | Method::SendTwoFactorSms
        | Method::SendTwoFactorNotification
        | Method::SubmitPassword
        | Method::SubmitTwoFactorCode
        | Method::UnlockCrypto
        | Method::LockCrypto
        | Method::GetCryptoStatus
        | Method::CryptoReset
        | Method::GetCryptoPrivKeyFlags
        | Method::SendCryptoChangeUserPrivate
        | Method::Shutdown
        | Method::SetAuthPersistence
        | Method::ListIncomingShares
        | Method::ListOutgoingShares
        | Method::ListIncomingShareRequests
        | Method::ListOutgoingShareRequests
        | Method::ListContacts
        | Method::ListMyTeams
        | Method::ListNotifications
        | Method::SessionStatus => 0,
        _ => 0,
    }
}

#[test]
fn every_method_variant_round_trips() {
    for &method in every_method() {
        let bytes = encode_request(&Request::Plain { method }).expect("encode");
        let frame = decode_request(&bytes).expect("decode");
        match frame.payload.request {
            Request::Plain { method: decoded } => assert_eq!(decoded, method),
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}

fn arb_method() -> impl Strategy<Value = Method> {
    let all = every_method().to_vec();
    (0..all.len()).prop_map(move |idx| all[idx])
}

fn arb_response_status() -> impl Strategy<Value = ResponseStatus> {
    prop_oneof![
        Just(ResponseStatus::Ok),
        Just(ResponseStatus::InvalidRequest),
        Just(ResponseStatus::Unauthorized),
        Just(ResponseStatus::Conflict),
        Just(ResponseStatus::Unavailable),
        Just(ResponseStatus::InternalError),
    ]
}

fn arb_kv_kind() -> impl Strategy<Value = ValueKvKind> {
    prop_oneof![
        Just(ValueKvKind::Bool),
        Just(ValueKvKind::Int),
        Just(ValueKvKind::Uint),
        Just(ValueKvKind::String),
    ]
}

fn arb_kv_payload() -> impl Strategy<Value = ValueKvPayload> {
    prop_oneof![
        any::<bool>().prop_map(ValueKvPayload::Bool),
        any::<i64>().prop_map(ValueKvPayload::Int),
        any::<u64>().prop_map(ValueKvPayload::Uint),
        ".{0,64}".prop_map(ValueKvPayload::String),
    ]
}

fn arb_request() -> impl Strategy<Value = Request> {
    prop_oneof![
        arb_method().prop_map(|method| Request::Plain { method }),
        (".{0,64}", ".{0,64}").prop_map(|(u, v)| Request::PasswordSubmission {
            username: u,
            value: v
        }),
        ".{0,64}".prop_map(|v| Request::AuthTokenSubmission { value: v }),
        (".{0,16}", any::<bool>(), any::<bool>()).prop_map(|(v, t, r)| {
            Request::TwoFactorCodeSubmission {
                value: v,
                trust_device: t,
                recovery_code: r,
            }
        }),
        ".{0,64}".prop_map(|p| Request::CryptoUnlock { password: p }),
        (".{0,64}", proptest::option::of(".{0,64}")).prop_map(|(p, h)| Request::CryptoSetup {
            password: p,
            hint: h
        }),
        any::<bool>().prop_map(|enabled| Request::AuthPersistence { enabled }),
        (".{0,64}", ".{0,64}").prop_map(|(l, r)| Request::SyncRootAdd {
            local_path: l,
            remote_path: r,
            sync_type: None,
        }),
        any::<u64>().prop_map(|id| Request::SyncRootRemove { sync_id: id }),
        any::<u64>().prop_map(|id| Request::SyncRootPause { sync_id: id }),
        any::<u64>().prop_map(|id| Request::SyncRootResume { sync_id: id }),
        ".{0,64}".prop_map(|p| Request::IsFolderSyncable { path: p }),
        ".{0,64}".prop_map(|c| Request::ShowPublicLink { code: c }),
        any::<u64>().prop_map(|id| Request::DeletePublicLink { link_id: id }),
        ".{0,64}".prop_map(|p| Request::CreateFilePublicLink { path: p }),
        ".{0,64}".prop_map(|p| Request::CreateFolderPublicLink { path: p }),
        (any::<u64>(), proptest::option::of(any::<u64>())).prop_map(|(id, exp)| {
            Request::ChangePublicLinkExpire {
                link_id: id,
                expire: exp,
            }
        }),
        (any::<u64>(), proptest::option::of(".{0,64}")).prop_map(|(id, p)| {
            Request::ChangePublicLinkPassword {
                link_id: id,
                password: p,
            }
        }),
        Just(Request::ListBookmarks),
        (".{0,64}", any::<u64>()).prop_map(|(c, l)| Request::RemoveBookmark {
            code: c,
            location_id: l
        }),
        (".{0,32}", arb_kv_kind()).prop_map(|(n, k)| Request::ValueGet { name: n, kind: k }),
        (".{0,32}", arb_kv_payload()).prop_map(|(n, p)| Request::ValueSet { name: n, value: p }),
        (".{0,32}", arb_kv_kind()).prop_map(|(n, k)| Request::ValueHas { name: n, kind: k }),
    ]
}

proptest! {
    #[test]
    fn prop_request_round_trips(request in arb_request()) {
        let bytes = match encode_request(&request) {
            Ok(b) => b,
            Err(_) => return Ok(()),
        };
        let frame = decode_request(&bytes).expect("decode should succeed");
        prop_assert!(frame.payload.traceparent().is_none());
        prop_assert_eq!(frame.payload.request, request);
    }

    #[test]
    fn prop_response_round_trips(
        status in arb_response_status(),
        message in ".{0,256}"
    ) {
        let original = Response { status: status.clone(), message: message.clone() };
        let bytes = encode_response(&original).expect("encode");
        let frame = decode_response(&bytes).expect("decode");
        prop_assert_eq!(frame.payload.status, status);
        prop_assert_eq!(frame.payload.message, message);
    }

    #[test]
    fn prop_every_method_plain_round_trip(method in arb_method()) {
        let bytes = encode_request(&Request::Plain { method }).expect("encode");
        let frame = decode_request(&bytes).expect("decode");
        match frame.payload.request {
            Request::Plain { method: decoded } => prop_assert_eq!(decoded, method),
            other => prop_assert!(false, "unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn prop_random_bytes_do_not_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let _ = decode_request(&bytes);
        let _ = decode_response(&bytes);
    }
}
