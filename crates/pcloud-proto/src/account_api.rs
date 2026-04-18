//! Account protocol client: typed builders and parsers for
//! `verifyemail`, `lostpassword`, `changepassword`, `register`,
//! `getpromo`, `getapiservers`, `setlanguage`, and `setapiserver`.
//! Consumed by `pcloud-backends::account_backend`.
//!
//! ## Role in the request pipeline
//!
//! Wraps the pCloud account-management command family and projects
//! responses into typed domain structs. Credential-bearing requests
//! (`register`, `changepassword`) accept the password via `&str`
//! but upstream callers wrap the material in `pcloud-secret` and
//! unwrap it only at the call site to minimise the time cleartext
//! lives in `String`.
//!
//! ## Security considerations
//!
//! - Passwords are never logged and never stored on the client
//!   side in any long-lived struct.
//! - `getapiservers` responses are used to update transport host /
//!   port; the transport layer validates each returned hint
//!   before applying it, so a malicious server cannot redirect the
//!   client to a non-pCloud host that would then present a valid
//!   certificate for its own name.
//! - `setlanguage` is safe by construction — the language code is
//!   a short BCP-47 tag validated against a known list at the
//!   backend layer.
//!
//! Portable; no platform gating.

use thiserror::Error;

use crate::{
    ProtocolMethod,
    auth_api::{ApiServerHint, ApiServerHintConsumer, ProtocolTransport},
    methods::account::{
        ChangePasswordRequest, GetLocationApiRequest, GetPromoRequest, LostPasswordRequest,
        RegisterRequest, SetLanguageRequest, VerifyEmailRequest,
    },
    response::{HashView, Value},
};

/// `AccountApi` — account api.
#[derive(Debug)]
pub struct AccountApi<T> {
    transport: T,
}

/// `AccountApiError` — account api error.
#[derive(Debug, Error)]
pub enum AccountApiError<E: std::error::Error + Send + Sync + 'static> {
    /// `Encode` variant (encode).
    #[error(transparent)]
    Encode(#[from] crate::FrameParseError),
    /// `Transport` variant (transport).
    #[error("transport failed: {0}")]
    Transport(E),
    /// `Result` variant (result).
    #[error("account method returned non-zero result code {result} ({message:?})")]
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

/// `PromoInfo` — promo info.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromoInfo {
    /// The `url` field (url).
    pub url: String,
    /// The `width` field (width).
    pub width: u64,
    /// The `height` field (height).
    pub height: u64,
    /// The `api_server` field (api server).
    pub api_server: Option<ApiServerHint>,
}

/// `ApiServerInfo` — api server info.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiServerInfo {
    /// The `label` field (label).
    pub label: String,
    /// The `api` field (api).
    pub api: String,
    /// The `binapi` field (binapi).
    pub binapi: String,
    /// The `location_id` field (location id).
    pub location_id: u64,
}

/// `PasswordChangeResult` — password change result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordChangeResult {
    /// The `auth_token` field (auth token).
    pub auth_token: String,
    /// The `api_server` field (api server).
    pub api_server: Option<ApiServerHint>,
}

impl<T> AccountApi<T> {
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

impl<T> AccountApi<T>
where
    T: ProtocolTransport + ApiServerHintConsumer,
{
    /// `apply_api_server_hint` — apply api server hint.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn apply_api_server_hint(&self, api_server: &str) {
        self.transport.apply_api_server_hint(api_server);
    }

    /// `get_promo` — get promo.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn get_promo(
        &self,
        auth_token: impl Into<String>,
        os_id: u64,
    ) -> Result<Option<PromoInfo>, AccountApiError<T::Error>> {
        let request = GetPromoRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            os_id,
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(AccountApiError::Transport)?;
        let hash = response.as_hash().ok_or(AccountApiError::Malformed(
            "getpromourl response was not a hash",
        ))?;
        expect_ok_result(hash)?;

        if !hash.get_bool("haspromo").unwrap_or(false) {
            return Ok(None);
        }

        let promo = PromoInfo {
            url: hash
                .get_string("url")
                .ok_or(AccountApiError::Malformed("getpromourl missing url"))?
                .to_owned(),
            width: hash
                .get_number("width")
                .ok_or(AccountApiError::Malformed("getpromourl missing width"))?,
            height: hash
                .get_number("height")
                .ok_or(AccountApiError::Malformed("getpromourl missing height"))?,
            api_server: extract_api_server_hint(hash),
        };
        if let Some(hint) = promo.api_server.as_ref() {
            self.transport.apply_api_server_hint(&hint.binapi);
        }
        Ok(Some(promo))
    }

    /// `set_language` — set language.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn set_language(
        &self,
        auth_token: impl Into<String>,
        language: impl Into<String>,
    ) -> Result<(), AccountApiError<T::Error>> {
        let request = SetLanguageRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            language: language.into(),
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(AccountApiError::Transport)?;
        let hash = response.as_hash().ok_or(AccountApiError::Malformed(
            "setlanguage response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        if let Some(hint) = extract_api_server_hint(hash).as_ref() {
            self.transport.apply_api_server_hint(&hint.binapi);
        }
        Ok(())
    }

    /// `get_api_servers` — get api servers.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn get_api_servers(&self) -> Result<Vec<ApiServerInfo>, AccountApiError<T::Error>> {
        let request = GetLocationApiRequest;
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(AccountApiError::Transport)?;
        let hash = response.as_hash().ok_or(AccountApiError::Malformed(
            "getlocationapi response was not a hash",
        ))?;
        expect_ok_result(hash)?;

        let locations = hash
            .get_array("locations")
            .ok_or(AccountApiError::Malformed(
                "getlocationapi missing locations",
            ))?;

        locations
            .iter()
            .map(|value| {
                let location = value
                    .as_hash()
                    .ok_or(AccountApiError::Malformed("location entry was not a hash"))?;
                Ok(ApiServerInfo {
                    label: location
                        .get_string("label")
                        .ok_or(AccountApiError::Malformed("location missing label"))?
                        .to_owned(),
                    api: location
                        .get_string("api")
                        .ok_or(AccountApiError::Malformed("location missing api"))?
                        .to_owned(),
                    binapi: location
                        .get_string("binapi")
                        .ok_or(AccountApiError::Malformed("location missing binapi"))?
                        .to_owned(),
                    location_id: location
                        .get_number("id")
                        .ok_or(AccountApiError::Malformed("location missing id"))?,
                })
            })
            .collect()
    }

    /// `verify_email` — verify email.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn verify_email(
        &self,
        auth_token: impl Into<String>,
    ) -> Result<(), AccountApiError<T::Error>> {
        let request = VerifyEmailRequest {
            auth_token: Some(crate::redacted::RedactedProtoString::from(
                auth_token.into(),
            )),
            verify_token: None,
        };
        execute_unit(
            &self.transport,
            &request,
            "sendverificationemail response was not a hash",
        )
    }

    /// `verify_email_restricted` — verify email restricted.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn verify_email_restricted(
        &self,
        verify_token: impl Into<String>,
    ) -> Result<(), AccountApiError<T::Error>> {
        let request = VerifyEmailRequest {
            auth_token: None,
            verify_token: Some(crate::redacted::RedactedProtoString::from(
                verify_token.into(),
            )),
        };
        execute_unit(
            &self.transport,
            &request,
            "sendverificationemail response was not a hash",
        )
    }

    /// `lost_password` — lost password.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn lost_password(&self, email: impl Into<String>) -> Result<(), AccountApiError<T::Error>> {
        let request = LostPasswordRequest {
            email: email.into(),
        };
        execute_unit(
            &self.transport,
            &request,
            "lostpassword response was not a hash",
        )
    }

    /// `change_password` — change password.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn change_password(
        &self,
        auth_token: impl Into<String>,
        current_password: impl Into<String>,
        new_password: impl Into<String>,
        device: impl Into<String>,
    ) -> Result<PasswordChangeResult, AccountApiError<T::Error>> {
        let request = ChangePasswordRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            current_password: crate::redacted::RedactedProtoString::from(current_password.into()),
            new_password: crate::redacted::RedactedProtoString::from(new_password.into()),
            device: device.into(),
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(AccountApiError::Transport)?;
        let hash = response.as_hash().ok_or(AccountApiError::Malformed(
            "changepassword response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        let result = PasswordChangeResult {
            auth_token: hash
                .get_string("auth")
                .ok_or(AccountApiError::Malformed("changepassword missing auth"))?
                .to_owned(),
            api_server: extract_api_server_hint(hash),
        };
        if let Some(hint) = result.api_server.as_ref() {
            self.transport.apply_api_server_hint(&hint.binapi);
        }
        Ok(result)
    }

    /// `register` — register.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn register(
        &self,
        email: impl Into<String>,
        password: impl Into<String>,
        terms_accepted: bool,
        os_id: u64,
    ) -> Result<(), AccountApiError<T::Error>> {
        let request = RegisterRequest {
            email: email.into(),
            password: crate::redacted::RedactedProtoString::from(password.into()),
            terms_accepted,
            os_id,
        };
        execute_unit(
            &self.transport,
            &request,
            "register response was not a hash",
        )
    }
}

fn execute_unit<T, M>(
    transport: &T,
    request: &M,
    malformed_message: &'static str,
) -> Result<(), AccountApiError<T::Error>>
where
    T: ProtocolTransport + ApiServerHintConsumer,
    M: ProtocolMethod,
{
    let encoded = request.encode()?;
    let response = transport
        .execute(&encoded)
        .map_err(AccountApiError::Transport)?;
    let hash = response
        .as_hash()
        .ok_or(AccountApiError::Malformed(malformed_message))?;
    expect_ok_result(hash)?;
    if let Some(hint) = extract_api_server_hint(hash).as_ref() {
        transport.apply_api_server_hint(&hint.binapi);
    }
    Ok(())
}

fn expect_ok_result<E>(hash: HashView<'_>) -> Result<(), AccountApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let result = hash.get_number("result").unwrap_or(0);
    if result == 0 {
        return Ok(());
    }
    Err(AccountApiError::Result {
        result,
        message: hash.get_string("error").map(ToOwned::to_owned),
    })
}

fn extract_api_server_hint(hash: HashView<'_>) -> Option<ApiServerHint> {
    let direct = hash.get_string("binapi").map(ToOwned::to_owned);
    let nested = hash.get_hash("apiserver").and_then(|apiserver| {
        apiserver
            .get_array("binapi")
            .and_then(|entries| entries.first())
            .and_then(Value::as_string)
            .map(ToOwned::to_owned)
            .or_else(|| apiserver.get_string("binapi").map(ToOwned::to_owned))
    });
    direct.or(nested).map(|binapi| ApiServerHint { binapi })
}

#[cfg(test)]
mod tests {
    use std::{io, sync::Mutex};

    use crate::{
        auth_api::{ApiServerHintConsumer, ProtocolTransport},
        response::Value,
    };

    use super::AccountApi;

    #[derive(Debug)]
    struct MockTransport {
        responses: Mutex<Vec<Value>>,
        hints: Mutex<Vec<String>>,
    }

    impl MockTransport {
        fn with_responses(responses: Vec<Value>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().rev().collect()),
                hints: Mutex::new(Vec::new()),
            }
        }
    }

    impl ProtocolTransport for MockTransport {
        type Error = io::Error;

        fn execute(&self, _request: &crate::EncodedRequest) -> Result<Value, Self::Error> {
            self.responses
                .lock()
                .expect("responses lock should not be poisoned")
                .pop()
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "missing response"))
        }
    }

    impl ApiServerHintConsumer for MockTransport {
        fn apply_api_server_hint(&self, api_server: &str) {
            self.hints
                .lock()
                .expect("hints lock should not be poisoned")
                .push(api_server.to_owned());
        }
    }

    #[test]
    fn get_promo_parses_optional_promo() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("haspromo".to_owned(), Value::Bool(true)),
            (
                "url".to_owned(),
                Value::String("https://promo.example/banner".to_owned()),
            ),
            ("width".to_owned(), Value::Number(640)),
            ("height".to_owned(), Value::Number(480)),
            (
                "binapi".to_owned(),
                Value::String("bineapi-eu.pcloud.com".to_owned()),
            ),
        ])]);
        let api = AccountApi::new(transport);

        let promo = api
            .get_promo("auth", 3)
            .expect("promo should parse")
            .expect("promo should be present");

        assert_eq!(promo.width, 640);
        assert_eq!(promo.height, 480);
        assert_eq!(promo.url, "https://promo.example/banner");
        assert_eq!(
            api.transport
                .hints
                .lock()
                .expect("hints lock should not be poisoned")
                .as_slice(),
            ["bineapi-eu.pcloud.com"]
        );
    }

    #[test]
    fn get_api_servers_parses_locations() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![(
            "locations".to_owned(),
            Value::Array(vec![
                Value::Hash(vec![
                    ("label".to_owned(), Value::String("Europe".to_owned())),
                    ("api".to_owned(), Value::String("api.pcloud.com".to_owned())),
                    (
                        "binapi".to_owned(),
                        Value::String("bineapi-eu.pcloud.com".to_owned()),
                    ),
                    ("id".to_owned(), Value::Number(2)),
                ]),
                Value::Hash(vec![
                    ("label".to_owned(), Value::String("US".to_owned())),
                    (
                        "api".to_owned(),
                        Value::String("api-us.pcloud.com".to_owned()),
                    ),
                    (
                        "binapi".to_owned(),
                        Value::String("bineapi-us.pcloud.com".to_owned()),
                    ),
                    ("id".to_owned(), Value::Number(1)),
                ]),
            ]),
        )])]);
        let api = AccountApi::new(transport);

        let locations = api.get_api_servers().expect("locations should parse");

        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].label, "Europe");
        assert_eq!(locations[1].location_id, 1);
    }

    #[test]
    fn set_language_rejects_nonzero_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2000)),
            (
                "error".to_owned(),
                Value::String("invalid language".to_owned()),
            ),
        ])]);
        let api = AccountApi::new(transport);

        let err = api
            .set_language("auth", "zz")
            .expect_err("invalid language should fail");

        assert!(err.to_string().contains("2000"));
    }

    #[test]
    fn change_password_parses_new_auth_token() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![(
            "auth".to_owned(),
            Value::String("new-auth-token".to_owned()),
        )])]);
        let api = AccountApi::new(transport);

        let result = api
            .change_password("auth", "old", "new", "Desktop")
            .expect("change password should parse");

        assert_eq!(result.auth_token, "new-auth-token");
    }

    #[test]
    fn lost_password_rejects_nonzero_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2000)),
            (
                "error".to_owned(),
                Value::String("unknown email".to_owned()),
            ),
        ])]);
        let api = AccountApi::new(transport);

        let err = api
            .lost_password("missing@example.com")
            .expect_err("lost password should fail");

        assert!(err.to_string().contains("2000"));
    }
}
