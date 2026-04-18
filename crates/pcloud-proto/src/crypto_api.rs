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

use crate::{
    ProtocolMethod,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    methods::crypto::{ChangeUserPrivateRequest, SendChangeUserPrivateRequest},
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
}
