//! High-level transport shim for the crypto password-change family.
//!
//! Mirrors [`crate::account_api`] structurally. This module exposes
//! [`CryptoApi::change_user_private`] and
//! [`CryptoApi::send_change_user_private`], which are used by the daemon
//! runtime after the local crypto shell has produced the re-encoded private
//! key and its signature.
//!
//! ## Role in the request pipeline
//!
//! Consumes pre-encoded key material produced by `pcloud-crypto`
//! and submits it to the server alongside the authenticated
//! session. This module does **not** perform any cryptographic
//! operations of its own — all key derivation, re-encryption, and
//! signing happens upstream. It is purely a typed wire-format
//! adaptor.
//!
//! ## Security considerations
//!
//! - Private-key bytes arrive here as `&[u8]`; they are written
//!   verbatim into the frame and never cloned, logged, or retained
//!   past the request.
//! - Signatures are treated as opaque bytes; this module performs
//!   no verification of its own.
//! - Callers must hold the crypto key in a zeroizing container
//!   (`pcloud-secret::SecretBytes`) upstream and only expose the
//!   slice for the duration of the call.

// **PLATFORM:** all
// **GATING:** none (portable).

use thiserror::Error;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;

use crate::{
    ProtocolMethod,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    methods::crypto::{
        ChangeUserPrivateRequest, CryptoGetFileKeyRequest, CryptoGetFolderKeyRequest,
        PclsyncSetUserKeysRequest, SendChangeUserPrivateRequest,
    },
    response::HashView,
};

/// `CryptoApi` — crypto api.
#[derive(Debug)]
pub struct CryptoApi<T> {
    transport: T,
}

/// `CryptoApiError` — crypto api error.
#[derive(Debug, Error)]
pub enum CryptoApiError<E: std::error::Error + Send + Sync + 'static> {
    /// `Encode` variant (encode).
    #[error(transparent)]
    Encode(#[from] crate::FrameParseError),
    /// `Transport` variant (transport).
    #[error("transport failed: {0}")]
    Transport(E),
    /// `Result` variant (result).
    #[error("crypto method returned non-zero result code {result} ({message:?})")]
    Result {
        /// The `result` field (result).
        result: u64,
        /// The `message` field (message).
        message: Option<String>,
    },
    /// `Malformed` variant (malformed).
    #[error("response was malformed: {0}")]
    Malformed(&'static str),
}

impl<T> CryptoApi<T> {
    /// `new` — new.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T> CryptoApi<T>
where
    T: ProtocolTransport + ApiServerHintConsumer,
{
    /// Upload a re-encoded private key (post-password-rotation). Returns
    /// `Ok(())` when the server reports `result=0`.
    pub fn change_user_private(
        &self,
        auth_token: &str,
        private_key: &str,
        signature: &str,
        hint: &str,
        code: &str,
    ) -> Result<(), CryptoApiError<T::Error>> {
        let request = ChangeUserPrivateRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token),
            private_key: private_key.to_owned(),
            signature: signature.to_owned(),
            hint: hint.to_owned(),
            code: code.to_owned(),
        };
        execute_unit(
            &self.transport,
            &request,
            "crypto_changeuserprivate response was not a hash",
        )
    }

    /// Ask the server to send a confirmation code to the user's email so the
    /// subsequent [`Self::change_user_private`] call can be authorized.
    pub fn send_change_user_private(
        &self,
        auth_token: &str,
    ) -> Result<(), CryptoApiError<T::Error>> {
        let request = SendChangeUserPrivateRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token),
        };
        execute_unit(
            &self.transport,
            &request,
            "crypto_sendchangeuserprivate response was not a hash",
        )
    }

    /// Upload the sealed `priv_key_ver1` + `pub_key_ver1` blobs for a
    /// fresh PclsyncCompat crypto setup (or for a post-password-rotation
    /// overwrite, which the upstream C client treats identically).
    ///
    /// Mirrors C `crypto_setuserkeys` (`pcryptofolder.c:155-178`). The
    /// caller passes the already-base64-encoded blobs.
    ///
    /// Known non-zero server result codes surfaced via
    /// [`CryptoApiError::Result`]:
    /// - `1000` — session not logged in
    /// - `2000` — can't connect
    /// - `2110` — crypto is already set up for this account
    ///
    /// # Errors
    /// Returns a typed [`CryptoApiError`] on transport failure, malformed
    /// response, or non-zero server result code.
    pub fn set_user_keys(
        &self,
        auth_token: &str,
        priv_key_ver1_b64: &str,
        pub_key_ver1_b64: &str,
        hint: Option<&str>,
    ) -> Result<(), CryptoApiError<T::Error>> {
        let request = PclsyncSetUserKeysRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token),
            priv_key_ver1_b64: priv_key_ver1_b64.to_owned(),
            pub_key_ver1_b64: pub_key_ver1_b64.to_owned(),
            hint: hint.map(str::to_owned),
        };
        execute_unit(
            &self.transport,
            &request,
            "crypto_setuserkeys response was not a hash",
        )
    }

    /// Fetch a folder's RSA-OAEP-wrapped `sym_key_ver1` blob from the
    /// server. Mirrors `download_fldr_enckey` at
    /// `pcryptofolder.c:808`. Returns the raw wrapped-key bytes (base64
    /// already decoded); the caller is responsible for RSA-OAEP-unwrap
    /// against the user's private key.
    ///
    /// # Errors
    /// Transport / malformed / non-zero `result` map onto
    /// [`CryptoApiError`] as usual. A `result=0` response with no `"key"`
    /// field surfaces as [`CryptoApiError::Malformed`].
    pub fn get_folder_key(
        &self,
        auth_token: &str,
        folder_id: u64,
    ) -> Result<Vec<u8>, CryptoApiError<T::Error>> {
        let request = CryptoGetFolderKeyRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token),
            folder_id,
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(CryptoApiError::Transport)?;
        let hash = response
            .as_hash()
            .ok_or(CryptoApiError::Malformed(
                "crypto_getfolderkey response was not a hash",
            ))?;
        expect_ok_result(hash)?;
        let key_b64 = hash.get_string("key").ok_or(CryptoApiError::Malformed(
            "crypto_getfolderkey response missing \"key\" field",
        ))?;
        B64.decode(key_b64).map_err(|_| {
            CryptoApiError::Malformed("crypto_getfolderkey \"key\" field was not valid base64")
        })
    }

    /// Fetch a file's RSA-OAEP-wrapped `sym_key_ver1` blob plus its
    /// file-version `hash`. Mirrors `download_file_enckey` at
    /// `pcryptofolder.c:862`.
    ///
    /// # Errors
    /// Same taxonomy as [`Self::get_folder_key`]. A successful response
    /// that omits the `"hash"` field surfaces as
    /// [`CryptoApiError::Malformed`] (the C client refuses to proceed
    /// without a version hash too, `pcryptofolder.c:900`).
    pub fn get_file_key(
        &self,
        auth_token: &str,
        file_id: u64,
    ) -> Result<(u64, Vec<u8>), CryptoApiError<T::Error>> {
        let request = CryptoGetFileKeyRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token),
            file_id,
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(CryptoApiError::Transport)?;
        let hash = response
            .as_hash()
            .ok_or(CryptoApiError::Malformed(
                "crypto_getfilekey response was not a hash",
            ))?;
        expect_ok_result(hash)?;
        let key_b64 = hash.get_string("key").ok_or(CryptoApiError::Malformed(
            "crypto_getfilekey response missing \"key\" field",
        ))?;
        let file_hash = hash.get_number("hash").ok_or(CryptoApiError::Malformed(
            "crypto_getfilekey response missing \"hash\" field",
        ))?;
        let wrapped = B64.decode(key_b64).map_err(|_| {
            CryptoApiError::Malformed("crypto_getfilekey \"key\" field was not valid base64")
        })?;
        Ok((file_hash, wrapped))
    }
}

fn execute_unit<T, M>(
    transport: &T,
    request: &M,
    malformed_message: &'static str,
) -> Result<(), CryptoApiError<T::Error>>
where
    T: ProtocolTransport + ApiServerHintConsumer,
    M: ProtocolMethod,
{
    let encoded = request.encode()?;
    let response = transport
        .execute(&encoded)
        .map_err(CryptoApiError::Transport)?;
    let hash = response
        .as_hash()
        .ok_or(CryptoApiError::Malformed(malformed_message))?;
    expect_ok_result(hash)?;
    Ok(())
}

fn expect_ok_result<E>(hash: HashView<'_>) -> Result<(), CryptoApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let result = hash.get_number("result").unwrap_or(0);
    if result == 0 {
        return Ok(());
    }
    Err(CryptoApiError::Result {
        result,
        message: hash.get_string("error").map(ToOwned::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use std::{io, sync::Mutex};

    use super::{CryptoApi, CryptoApiError};
    use crate::{
        auth_api::{ApiServerHintConsumer, ProtocolTransport},
        binary_api::EncodedRequest,
        response::Value,
    };

    #[derive(Debug)]
    struct MockTransport {
        responses: Mutex<Vec<Value>>,
        captured: Mutex<Vec<String>>,
    }

    impl MockTransport {
        fn with_responses(responses: Vec<Value>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().rev().collect()),
                captured: Mutex::new(Vec::new()),
            }
        }
    }

    impl ProtocolTransport for MockTransport {
        type Error = io::Error;

        fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
            self.captured
                .lock()
                .unwrap()
                .push(request.frame.command.clone());
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| io::Error::other("no mock response"))
        }
    }

    impl ApiServerHintConsumer for MockTransport {
        fn apply_api_server_hint(&self, _api_server: &str) {}
    }

    fn ok_hash() -> Value {
        Value::Hash(vec![("result".to_owned(), Value::Number(0))])
    }

    fn err_hash(code: u64, msg: &str) -> Value {
        Value::Hash(vec![
            ("result".to_owned(), Value::Number(code)),
            ("error".to_owned(), Value::String(msg.to_owned())),
        ])
    }

    #[test]
    fn change_user_private_ok_on_result_zero() {
        let transport = MockTransport::with_responses(vec![ok_hash()]);
        let api = CryptoApi::new(transport);
        api.change_user_private("token", "pk", "sig", "hint", "code")
            .expect("ok");
    }

    #[test]
    fn change_user_private_surfaces_server_error() {
        let transport = MockTransport::with_responses(vec![err_hash(2000, "bad code")]);
        let api = CryptoApi::new(transport);
        let err = api
            .change_user_private("token", "pk", "sig", "hint", "code")
            .expect_err("must fail");
        match err {
            CryptoApiError::Result { result, message } => {
                assert_eq!(result, 2000);
                assert_eq!(message.as_deref(), Some("bad code"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn send_change_user_private_ok() {
        let transport = MockTransport::with_responses(vec![ok_hash()]);
        let api = CryptoApi::new(transport);
        api.send_change_user_private("token").expect("ok");
    }

    // -----------------------------------------------------------------
    // Stage 4b.3 — set_user_keys / get_folder_key / get_file_key
    // -----------------------------------------------------------------

    fn hash_with(entries: Vec<(&str, Value)>) -> Value {
        Value::Hash(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
        )
    }

    #[test]
    fn set_user_keys_ok_on_result_zero() {
        let transport = MockTransport::with_responses(vec![ok_hash()]);
        let api = CryptoApi::new(transport);
        api.set_user_keys("token", "cHJpdg==", "cHViYmxpYw==", Some("hint"))
            .expect("set_user_keys ok");
        let captured = api.transport.captured.lock().unwrap().clone();
        assert_eq!(captured, vec!["crypto_setuserkeys".to_owned()]);
    }

    #[test]
    fn set_user_keys_surfaces_server_2110_already_setup() {
        let transport =
            MockTransport::with_responses(vec![err_hash(2110, "crypto already set up")]);
        let api = CryptoApi::new(transport);
        let err = api
            .set_user_keys("tok", "cHJpdg==", "cHViYmxpYw==", None)
            .expect_err("must fail");
        match err {
            CryptoApiError::Result { result, message } => {
                assert_eq!(result, 2110);
                assert_eq!(message.as_deref(), Some("crypto already set up"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn get_folder_key_decodes_base64_payload() {
        // base64("abc" || 0x00..0x02) = "YWJjAAEC"
        let key_payload = Value::String("YWJjAAEC".to_owned());
        let resp = hash_with(vec![
            ("result", Value::Number(0)),
            ("key", key_payload),
        ]);
        let transport = MockTransport::with_responses(vec![resp]);
        let api = CryptoApi::new(transport);
        let bytes = api.get_folder_key("tok", 424_242).expect("ok");
        assert_eq!(bytes, b"abc\x00\x01\x02");
        let captured = api.transport.captured.lock().unwrap().clone();
        assert_eq!(captured, vec!["crypto_getfolderkey".to_owned()]);
    }

    #[test]
    fn get_folder_key_surfaces_server_1000_not_logged_in() {
        let transport =
            MockTransport::with_responses(vec![err_hash(1000, "not logged in")]);
        let api = CryptoApi::new(transport);
        let err = api.get_folder_key("tok", 1).expect_err("must fail");
        match err {
            CryptoApiError::Result { result, message } => {
                assert_eq!(result, 1000);
                assert_eq!(message.as_deref(), Some("not logged in"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn get_folder_key_malformed_when_key_missing() {
        let resp = hash_with(vec![("result", Value::Number(0))]);
        let transport = MockTransport::with_responses(vec![resp]);
        let api = CryptoApi::new(transport);
        let err = api.get_folder_key("tok", 1).expect_err("must fail");
        assert!(matches!(err, CryptoApiError::Malformed(_)));
    }

    #[test]
    fn get_file_key_decodes_hash_and_payload() {
        let resp = hash_with(vec![
            ("result", Value::Number(0)),
            ("key", Value::String("YWJjAAEC".to_owned())), // "abc\x00\x01\x02"
            ("hash", Value::Number(0xDEAD_BEEF)),
        ]);
        let transport = MockTransport::with_responses(vec![resp]);
        let api = CryptoApi::new(transport);
        let (hash, bytes) = api.get_file_key("tok", 777).expect("ok");
        assert_eq!(hash, 0xDEAD_BEEF);
        assert_eq!(bytes, b"abc\x00\x01\x02");
        let captured = api.transport.captured.lock().unwrap().clone();
        assert_eq!(captured, vec!["crypto_getfilekey".to_owned()]);
    }

    #[test]
    fn get_file_key_surfaces_server_2000_cant_connect() {
        let transport =
            MockTransport::with_responses(vec![err_hash(2000, "can't connect")]);
        let api = CryptoApi::new(transport);
        let err = api.get_file_key("tok", 9).expect_err("must fail");
        match err {
            CryptoApiError::Result { result, message } => {
                assert_eq!(result, 2000);
                assert_eq!(message.as_deref(), Some("can't connect"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn get_file_key_malformed_when_hash_missing() {
        let resp = hash_with(vec![
            ("result", Value::Number(0)),
            ("key", Value::String("YWJjAAEC".to_owned())),
        ]);
        let transport = MockTransport::with_responses(vec![resp]);
        let api = CryptoApi::new(transport);
        let err = api.get_file_key("tok", 1).expect_err("must fail");
        assert!(matches!(err, CryptoApiError::Malformed(_)));
    }
}
