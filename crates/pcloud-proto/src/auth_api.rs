//! Auth protocol client: password and token login, TFA code /
//! recovery-code submission, SMS and device-notification resend,
//! authenticated `userinfo`, refresh, and logged-device enumeration.
//! Responses are strongly typed; secrets transit via
//! `pcloud-secret::SecretString`.
//!
//! ## Role in the request pipeline
//!
//! This module is the entry point for every session: it turns a
//! user's credentials into an auth token that downstream API
//! modules thread through their requests. It also defines the
//! [`ProtocolTransport`] and [`ApiServerHintConsumer`] traits that
//! every transport in this crate implements, keeping the generic
//! `*Api` clients decoupled from a specific transport type.
//!
//! ## Security considerations
//!
//! - Passwords enter this module via
//!   `pcloud_secret::SecretString`; they are unwrapped into a
//!   `BinaryParam` exactly once, at the moment of frame
//!   construction, and never cloned or logged.
//! - Login digests (legacy `logindigest` flow) are computed with
//!   SHA-1 because the server mandates it for that code path; the
//!   modern flow uses TLS-protected direct password exchange and
//!   should be preferred.
//! - TFA codes are short-lived secrets; this module does not
//!   retain them past the request.
//! - `ApiServerHint` updates are applied via
//!   [`ApiServerHintConsumer::apply_api_server_hint`] *only after*
//!   the login succeeds, so a failed-auth server cannot force a
//!   transport redirect.
//!
//! Consumed by `pcloud-backends::auth_backend`. Portable; no platform
//! gating.

use sha1::{Digest, Sha1};
use thiserror::Error;

use pcloud_secret::{ExposeSecret, secret_string::SecretString};

use crate::{
    EncodedRequest, FrameParseError, ProtocolMethod,
    methods::auth::{
        AuthRequestContext, GetDigestRequest, LoginDigestRequest, TwoFactorLoginRequest,
        TwoFactorSendNotificationRequest, TwoFactorSendSmsRequest, UserInfoRequest,
    },
    response::Value,
};

/// `ProtocolTransport` trait — protocol transport.
pub trait ProtocolTransport {
    /// Associated type `Error` — error.
    type Error: std::error::Error + Send + Sync + 'static;

    /// `execute` — execute.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error>;
}

/// `ApiServerHintConsumer` trait — api server hint consumer.
pub trait ApiServerHintConsumer {
    /// `apply_api_server_hint` — apply api server hint.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    fn apply_api_server_hint(&self, api_server: &str);
}

/// `AuthApi` — auth api.
#[derive(Debug)]
pub struct AuthApi<T> {
    transport: T,
    context: AuthRequestContext,
}

/// `AuthApiError` — auth api error.
#[derive(Debug, Error)]
pub enum AuthApiError<E: std::error::Error + Send + Sync + 'static> {
    /// `Encode` variant (encode).
    #[error("request encoding failed: {0}")]
    Encode(#[from] FrameParseError),
    /// `Transport` variant (transport).
    #[error("transport failed: {0}")]
    Transport(E),
    /// `Malformed` variant (malformed).
    #[error("response was malformed: {0}")]
    Malformed(&'static str),
}

/// `DigestChallenge` — digest challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestChallenge {
    /// The `digest` field (digest).
    pub digest: String,
}

/// `ApiServerHint` — api server hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiServerHint {
    /// The `binapi` field (binapi).
    pub binapi: String,
}

/// `PasswordLoginOutcome` — password login outcome.
///
/// CLAUDEREV iter-1 SEC-H fix: `auth_token` and `challenge_token` are
/// `SecretString` so they zeroize on drop, redact in `Debug`, and never
/// transit as raw `String`. The enum drops `Clone`, `PartialEq`, `Eq`
/// because `SecretString` is intentionally not `Clone` (use
/// `SecretString::clone_secret` for explicit duplication) and equality
/// on a credential is a leak vector.
#[derive(Debug)]
pub enum PasswordLoginOutcome {
    /// `Authenticated` variant (authenticated).
    Authenticated {
        /// The `auth_token` field (auth token). `SecretString` per
        /// CLAUDEREV iter-1 SEC-H fix.
        auth_token: SecretString,
        /// The `user_id` field (user id).
        user_id: Option<u64>,
        /// The `api_server` field (api server).
        api_server: Option<ApiServerHint>,
    },
    /// `TwoFactorRequired` variant (two factor required).
    TwoFactorRequired {
        /// The `challenge_token` field (challenge token). `SecretString`
        /// per CLAUDEREV iter-1 SEC-H fix.
        challenge_token: SecretString,
        /// The `trust_device` field (trust device).
        trust_device: bool,
        /// The `api_server` field (api server).
        api_server: Option<ApiServerHint>,
    },
    /// `Failed` variant (failed).
    Failed {
        /// The `result` field (result).
        result: u64,
        /// The `message` field (message).
        message: Option<String>,
        /// The `api_server` field (api server).
        api_server: Option<ApiServerHint>,
    },
}

/// `UserInfo` — user info.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    /// The `user_id` field (user id).
    pub user_id: Option<u64>,
    /// The `email` field (email).
    pub email: Option<String>,
    /// The `api_server` field (api server).
    pub api_server: Option<ApiServerHint>,
    /// Total account quota in bytes.
    pub quota: Option<u64>,
    /// Bytes currently used against the account quota.
    pub used_quota: Option<u64>,
    /// Whether the account is premium.
    pub premium: Option<bool>,
    /// Unix timestamp when premium expires (0 or absent if non-premium).
    pub premium_expires: Option<u64>,
    /// Whether the account email is verified.
    pub email_verified: Option<bool>,
    /// Numeric plan identifier returned by the backend.
    pub plan: Option<u64>,
}

/// `TwoFactorSmsDelivery` — two factor sms delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoFactorSmsDelivery {
    /// The `country_code` field (country code).
    pub country_code: Option<String>,
    /// The `phone_number` field (phone number).
    pub phone_number: Option<String>,
}

/// `LoggedDevice` — logged device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedDevice {
    /// The `name` field (name).
    pub name: Option<String>,
    /// The `device_type` field (device type).
    pub device_type: Option<u64>,
}

/// `TwoFactorNotificationDelivery` — two factor notification delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoFactorNotificationDelivery {
    /// The `devices` field (devices).
    pub devices: Vec<LoggedDevice>,
}

impl<T> AuthApi<T> {
    /// `new` — new.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            context: AuthRequestContext::default(),
        }
    }

    /// `with_context` — with context.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn with_context(transport: T, context: AuthRequestContext) -> Self {
        Self { transport, context }
    }
}

/// Classified failure from [`AuthApi::refresh_auth_token`].
///
/// Callers (the orchestrator facade in `pcloud-auth`) use this to decide
/// whether to revoke the session (auth expired) or retry with backoff
/// (temporary transport/server failure). Never reveals the current token.
#[derive(Debug, Error)]
pub enum AuthRefreshError<E: std::error::Error + Send + Sync + 'static> {
    /// The current auth token is no longer valid server-side.
    /// The caller MUST NOT retry; a fresh interactive login is required.
    #[error("auth token expired or revoked (result {0})")]
    AuthExpired(u64),
    /// Transient condition: transport/encode/server/malformed response.
    /// The caller MAY retry with backoff.
    #[error("temporary refresh failure: {0}")]
    TemporaryFailure(#[from] AuthApiError<E>),
    /// Server accepted the call but returned no replacement auth field.
    /// Treated as a temporary protocol anomaly.
    #[error("refresh response missing auth field")]
    MissingAuthField,
}

/// pCloud server result codes that indicate the auth token is no longer
/// valid. A refresh attempt against any of these must surface
/// [`AuthRefreshError::AuthExpired`] so the caller can revoke the session
/// cleanly without triggering retry storms.
///
/// - 1000: log in required (server treats the request as unauthenticated).
/// - 2000: invalid username or password (includes token credential failure).
/// - 2094: invalid `auth` (token revoked or expired).
/// - 2297: two-factor re-authentication required.
const AUTH_EXPIRED_RESULTS: &[u64] = &[1000, 2000, 2094, 2297];

#[inline]
fn is_auth_expired_result(result: u64) -> bool {
    AUTH_EXPIRED_RESULTS.contains(&result)
}

impl<T> AuthApi<T>
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

    /// Exchange the current auth token for a fresh one via
    /// `userinfo?getauth=1&auth=<current>`.
    ///
    /// pCloud does not issue OAuth-style refresh tokens; the auth token
    /// itself is the long-lived credential, and calling `userinfo` with
    /// `getauth=1` causes the server to mint a new token. The previous
    /// token remains valid until the server expires or revokes it.
    ///
    /// The current token is passed by reference (`&SecretString`) and
    /// only briefly exposed via `expose_secret()` to build the wire
    /// request. The returned `SecretString` is zeroized on drop.
    ///
    /// Failure classification:
    /// - [`AuthRefreshError::AuthExpired`] on server result codes in
    ///   `AUTH_EXPIRED_RESULTS` — the session must be revoked.
    /// - [`AuthRefreshError::TemporaryFailure`] on transport/protocol
    ///   errors or any other non-zero server result — safe to retry.
    /// - [`AuthRefreshError::MissingAuthField`] if the server returns
    ///   `result=0` but omits `auth` (protocol anomaly).
    pub fn refresh_auth_token(
        &self,
        current: &SecretString,
    ) -> Result<SecretString, AuthRefreshError<T::Error>> {
        // Force getauth=true on this request, independent of whatever
        // the ambient AuthRequestContext default happens to be.
        let mut context = self.context.clone();
        context.get_auth = true;

        let request = UserInfoRequest {
            auth_token: crate::redacted::RedactedProtoString::from(
                current.expose_secret().to_owned(),
            ),
            context,
        };
        let encoded = request.encode().map_err(AuthApiError::Encode)?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(AuthApiError::Transport)?;

        let hash = response
            .as_hash()
            .ok_or(AuthApiError::Malformed("refresh response was not a hash"))?;

        let result = hash.get_number("result").unwrap_or(0);
        if result != 0 {
            if is_auth_expired_result(result) {
                return Err(AuthRefreshError::AuthExpired(result));
            }
            return Err(AuthRefreshError::TemporaryFailure(AuthApiError::Malformed(
                "refresh returned non-zero result",
            )));
        }

        // Propagate API-server hints on success, consistent with other calls.
        let hint = extract_api_server_hint(hash);
        apply_api_server_hint(&self.transport, hint.as_ref());

        let new_token = hash
            .get_string("auth")
            .ok_or(AuthRefreshError::MissingAuthField)?;
        if new_token.is_empty() {
            return Err(AuthRefreshError::MissingAuthField);
        }
        Ok(SecretString::new(new_token.to_owned()))
    }

    /// `login_password` — login password.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn login_password(
        &self,
        username: String,
        password: impl AsRef<str>,
    ) -> Result<PasswordLoginOutcome, AuthApiError<T::Error>> {
        self.login_password_with_code(username, password, Option::<&str>::None)
    }

    /// `login_password_with_code` — login password with code.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn login_password_with_code(
        &self,
        username: String,
        password: impl AsRef<str>,
        code: Option<impl AsRef<str>>,
    ) -> Result<PasswordLoginOutcome, AuthApiError<T::Error>> {
        let challenge = self.get_digest()?;
        let request = LoginDigestRequest {
            username: username.clone(),
            digest_token: crate::redacted::RedactedProtoString::from(challenge.digest.clone()),
            password_digest: crate::redacted::RedactedProtoString::from(compute_password_digest(
                &username,
                password.as_ref(),
                &challenge.digest,
            )),
            code: code
                .map(|code| crate::redacted::RedactedProtoString::from(code.as_ref().to_owned())),
            context: self.context.clone(),
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(AuthApiError::Transport)?;
        let outcome = parse_login_outcome(response)?;
        apply_api_server_hint(&self.transport, login_outcome_api_server_hint(&outcome));
        Ok(outcome)
    }

    /// `get_digest` — get digest.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn get_digest(&self) -> Result<DigestChallenge, AuthApiError<T::Error>> {
        let encoded = GetDigestRequest.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(AuthApiError::Transport)?;
        let hash = response
            .as_hash()
            .ok_or(AuthApiError::Malformed("getdigest response was not a hash"))?;

        if hash.get_number("result").unwrap_or(0) != 0 {
            return Err(AuthApiError::Malformed("getdigest result was non-zero"));
        }

        let digest = hash.get_string("digest").ok_or(AuthApiError::Malformed(
            "getdigest response was missing digest",
        ))?;

        Ok(DigestChallenge {
            digest: digest.to_owned(),
        })
    }

    /// `submit_two_factor_code` — submit two factor code.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn submit_two_factor_code(
        &self,
        token: impl Into<String>,
        code: impl AsRef<str>,
        trust_device: bool,
        recovery_code: bool,
    ) -> Result<PasswordLoginOutcome, AuthApiError<T::Error>> {
        let request = TwoFactorLoginRequest {
            token: crate::redacted::RedactedProtoString::from(token.into()),
            code: crate::redacted::RedactedProtoString::from(code.as_ref().to_owned()),
            trust_device,
            recovery_code,
            context: self.context.clone(),
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(AuthApiError::Transport)?;
        let outcome = parse_login_outcome(response)?;
        apply_api_server_hint(&self.transport, login_outcome_api_server_hint(&outcome));
        Ok(outcome)
    }

    /// `userinfo` — userinfo.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn userinfo(
        &self,
        auth_token: impl AsRef<str>,
    ) -> Result<UserInfo, AuthApiError<T::Error>> {
        let request = UserInfoRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.as_ref().to_owned()),
            context: self.context.clone(),
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(AuthApiError::Transport)?;

        let hash = response
            .as_hash()
            .ok_or(AuthApiError::Malformed("userinfo response was not a hash"))?;

        let userinfo = UserInfo {
            user_id: hash.get_number("userid"),
            email: hash.get_string("email").map(ToOwned::to_owned),
            api_server: extract_api_server_hint(hash),
            quota: hash.get_number("quota"),
            used_quota: hash.get_number("usedquota"),
            premium: hash.get_bool("premium"),
            premium_expires: hash.get_number("premiumexpires"),
            email_verified: hash.get_bool("emailverified"),
            plan: hash.get_number("plan"),
        };
        apply_api_server_hint(&self.transport, userinfo.api_server.as_ref());
        Ok(userinfo)
    }

    /// `send_two_factor_sms` — send two factor sms.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn send_two_factor_sms(
        &self,
        token: impl Into<String>,
    ) -> Result<TwoFactorSmsDelivery, AuthApiError<T::Error>> {
        let request = TwoFactorSendSmsRequest {
            token: crate::redacted::RedactedProtoString::from(token.into()),
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(AuthApiError::Transport)?;
        parse_two_factor_sms_delivery(response)
    }

    /// `send_two_factor_notification` — send two factor notification.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn send_two_factor_notification(
        &self,
        token: impl Into<String>,
    ) -> Result<TwoFactorNotificationDelivery, AuthApiError<T::Error>> {
        let request = TwoFactorSendNotificationRequest {
            token: crate::redacted::RedactedProtoString::from(token.into()),
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(AuthApiError::Transport)?;
        parse_two_factor_notification_delivery(response)
    }
}

fn parse_login_outcome<E>(response: Value) -> Result<PasswordLoginOutcome, AuthApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let hash = response
        .as_hash()
        .ok_or(AuthApiError::Malformed("login response was not a hash"))?;
    let result = hash.get_number("result").unwrap_or(0);

    if result == 0 {
        let auth_token = hash.get_string("auth").ok_or(AuthApiError::Malformed(
            "missing auth token on successful login",
        ))?;
        return Ok(PasswordLoginOutcome::Authenticated {
            auth_token: SecretString::new(auth_token.to_owned()),
            user_id: hash.get_number("userid"),
            api_server: extract_api_server_hint(hash),
        });
    }

    if let Some(challenge_token) = hash
        .get_string("token")
        .or_else(|| hash.get_string("tfa_token"))
    {
        return Ok(PasswordLoginOutcome::TwoFactorRequired {
            challenge_token: SecretString::new(challenge_token.to_owned()),
            trust_device: hash.get_bool("trustdevice").unwrap_or(false),
            api_server: extract_api_server_hint(hash),
        });
    }

    if result == 2297 {
        return Ok(PasswordLoginOutcome::TwoFactorRequired {
            challenge_token: SecretString::new(String::new()),
            trust_device: hash.get_bool("trustdevice").unwrap_or(false),
            api_server: extract_api_server_hint(hash),
        });
    }

    Ok(PasswordLoginOutcome::Failed {
        result,
        message: hash.get_string("error").map(ToOwned::to_owned),
        api_server: extract_api_server_hint(hash),
    })
}

fn parse_two_factor_sms_delivery<E>(
    response: Value,
) -> Result<TwoFactorSmsDelivery, AuthApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let hash = response
        .as_hash()
        .ok_or(AuthApiError::Malformed("tfa sms response was not a hash"))?;
    let result = hash.get_number("result").unwrap_or(0);
    if result != 0 {
        return Err(AuthApiError::Malformed("tfa sms result was non-zero"));
    }

    let phone = hash.get_hash("phonedata");
    Ok(TwoFactorSmsDelivery {
        country_code: phone
            .and_then(|phone| phone.get_string("countrycode"))
            .map(ToOwned::to_owned),
        phone_number: phone
            .and_then(|phone| phone.get_string("msisdn"))
            .map(ToOwned::to_owned),
    })
}

fn parse_two_factor_notification_delivery<E>(
    response: Value,
) -> Result<TwoFactorNotificationDelivery, AuthApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let hash = response.as_hash().ok_or(AuthApiError::Malformed(
        "tfa notification response was not a hash",
    ))?;
    let result = hash.get_number("result").unwrap_or(0);
    if result != 0 {
        return Err(AuthApiError::Malformed(
            "tfa notification result was non-zero",
        ));
    }

    let devices = hash
        .get_array("devices")
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_hash)
                .map(|device| LoggedDevice {
                    name: device.get_string("name").map(ToOwned::to_owned),
                    device_type: device.get_number("type"),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(TwoFactorNotificationDelivery { devices })
}

fn login_outcome_api_server_hint(outcome: &PasswordLoginOutcome) -> Option<&ApiServerHint> {
    match outcome {
        PasswordLoginOutcome::Authenticated { api_server, .. }
        | PasswordLoginOutcome::TwoFactorRequired { api_server, .. }
        | PasswordLoginOutcome::Failed { api_server, .. } => api_server.as_ref(),
    }
}

fn apply_api_server_hint<T>(transport: &T, api_server: Option<&ApiServerHint>)
where
    T: ApiServerHintConsumer,
{
    if let Some(hint) = api_server {
        transport.apply_api_server_hint(&hint.binapi);
    }
}

fn extract_api_server_hint(hash: crate::response::HashView<'_>) -> Option<ApiServerHint> {
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

fn compute_password_digest(username: &str, password: &str, server_digest: &str) -> String {
    let lower_user = username.to_ascii_lowercase();
    let user_sha = Sha1::digest(lower_user.as_bytes());
    let user_hex = hex_lower(&user_sha);

    let mut hasher = Sha1::new();
    hasher.update(password.as_bytes());
    hasher.update(user_hex.as_bytes());
    hasher.update(server_digest.as_bytes());
    hex_lower(&hasher.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Mutex;

    use crate::response::Value;

    use super::{ApiServerHintConsumer, AuthApi, PasswordLoginOutcome, ProtocolTransport};

    #[derive(Debug)]
    struct HintTrackingTransport {
        responses: Mutex<Vec<Value>>,
        applied_hints: Mutex<Vec<String>>,
    }

    impl HintTrackingTransport {
        fn with_responses(responses: Vec<Value>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().rev().collect()),
                applied_hints: Mutex::new(Vec::new()),
            }
        }
    }

    impl ProtocolTransport for HintTrackingTransport {
        type Error = io::Error;

        fn execute(&self, _request: &crate::EncodedRequest) -> Result<Value, Self::Error> {
            self.responses
                .lock()
                .expect("responses lock should not be poisoned")
                .pop()
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "missing response"))
        }
    }

    impl ApiServerHintConsumer for HintTrackingTransport {
        fn apply_api_server_hint(&self, api_server: &str) {
            self.applied_hints
                .lock()
                .expect("hints lock should not be poisoned")
                .push(api_server.to_owned());
        }
    }

    #[test]
    fn login_parses_nested_api_server_hint_and_applies_it() {
        let transport = HintTrackingTransport::with_responses(vec![
            Value::Hash(vec![
                ("result".to_owned(), Value::Number(0)),
                (
                    "digest".to_owned(),
                    Value::String("development-digest".to_owned()),
                ),
            ]),
            Value::Hash(vec![
                ("result".to_owned(), Value::Number(0)),
                ("auth".to_owned(), Value::String("auth-token".to_owned())),
                (
                    "apiserver".to_owned(),
                    Value::Hash(vec![(
                        "binapi".to_owned(),
                        Value::Array(vec![Value::String("bineapi-eu.pcloud.com".to_owned())]),
                    )]),
                ),
            ]),
        ]);
        let api = AuthApi::new(transport);

        let outcome = api
            .login_password("alice@example.com".to_owned(), "correct-horse")
            .expect("login should succeed");

        match outcome {
            PasswordLoginOutcome::Authenticated { api_server, .. } => {
                assert_eq!(
                    api_server.map(|hint| hint.binapi),
                    Some("bineapi-eu.pcloud.com".to_owned())
                );
            }
            other => panic!("expected authenticated outcome, got {other:?}"),
        }

        assert_eq!(
            api.transport
                .applied_hints
                .lock()
                .expect("hints lock should not be poisoned")
                .as_slice(),
            ["bineapi-eu.pcloud.com"]
        );
    }

    #[test]
    fn userinfo_parses_direct_binapi_hint_and_applies_it() {
        let transport = HintTrackingTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            ("userid".to_owned(), Value::Number(42)),
            (
                "email".to_owned(),
                Value::String("alice@example.com".to_owned()),
            ),
            (
                "binapi".to_owned(),
                Value::String("bineapi-us.pcloud.com".to_owned()),
            ),
        ])]);
        let api = AuthApi::new(transport);

        let userinfo = api.userinfo("auth-token").expect("userinfo should succeed");

        assert_eq!(userinfo.user_id, Some(42));
        assert_eq!(userinfo.email.as_deref(), Some("alice@example.com"));
        assert_eq!(
            userinfo.api_server.map(|hint| hint.binapi),
            Some("bineapi-us.pcloud.com".to_owned())
        );
        assert_eq!(
            api.transport
                .applied_hints
                .lock()
                .expect("hints lock should not be poisoned")
                .as_slice(),
            ["bineapi-us.pcloud.com"]
        );
    }

    #[test]
    fn userinfo_parses_quota_and_plan_fields() {
        let transport = HintTrackingTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            ("userid".to_owned(), Value::Number(3_775_493)),
            (
                "email".to_owned(),
                Value::String("u@example.com".to_owned()),
            ),
            ("quota".to_owned(), Value::Number(10_737_418_240)),
            ("usedquota".to_owned(), Value::Number(4_294_967_296)),
            ("premium".to_owned(), Value::Bool(true)),
            ("premiumexpires".to_owned(), Value::Number(1_800_000_000)),
            ("emailverified".to_owned(), Value::Bool(true)),
            ("plan".to_owned(), Value::Number(7)),
        ])]);
        let api = AuthApi::new(transport);

        let userinfo = api.userinfo("auth-token").expect("userinfo should succeed");

        assert_eq!(userinfo.quota, Some(10_737_418_240));
        assert_eq!(userinfo.used_quota, Some(4_294_967_296));
        assert_eq!(userinfo.premium, Some(true));
        assert_eq!(userinfo.premium_expires, Some(1_800_000_000));
        assert_eq!(userinfo.email_verified, Some(true));
        assert_eq!(userinfo.plan, Some(7));
    }

    #[test]
    fn two_factor_login_applies_api_server_hint() {
        let transport = HintTrackingTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            ("auth".to_owned(), Value::String("auth-token".to_owned())),
            (
                "binapi".to_owned(),
                Value::String("bineapi-tfa.pcloud.com".to_owned()),
            ),
        ])]);
        let api = AuthApi::new(transport);

        let outcome = api
            .submit_two_factor_code("challenge-token", "654321", false, false)
            .expect("2fa login should succeed");

        match outcome {
            PasswordLoginOutcome::Authenticated { api_server, .. } => {
                assert_eq!(
                    api_server.map(|hint| hint.binapi),
                    Some("bineapi-tfa.pcloud.com".to_owned())
                );
            }
            other => panic!("expected authenticated outcome, got {other:?}"),
        }

        assert_eq!(
            api.transport
                .applied_hints
                .lock()
                .expect("hints lock should not be poisoned")
                .as_slice(),
            ["bineapi-tfa.pcloud.com"]
        );
    }

    #[test]
    fn send_two_factor_sms_parses_phone_data() {
        let transport = HintTrackingTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            (
                "phonedata".to_owned(),
                Value::Hash(vec![
                    ("countrycode".to_owned(), Value::String("+49".to_owned())),
                    ("msisdn".to_owned(), Value::String("123456789".to_owned())),
                ]),
            ),
        ])]);
        let api = AuthApi::new(transport);

        let delivery = api
            .send_two_factor_sms("challenge-token")
            .expect("sms delivery should parse");

        assert_eq!(delivery.country_code.as_deref(), Some("+49"));
        assert_eq!(delivery.phone_number.as_deref(), Some("123456789"));
    }

    #[test]
    fn refresh_auth_token_happy_path_returns_new_token_and_applies_hint() {
        use pcloud_secret::{ExposeSecret, secret_string::SecretString};

        let transport = HintTrackingTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            ("userid".to_owned(), Value::Number(42)),
            (
                "auth".to_owned(),
                Value::String("new-token-abcdef".to_owned()),
            ),
            (
                "binapi".to_owned(),
                Value::String("bineapi-eu.pcloud.com".to_owned()),
            ),
        ])]);
        let api = AuthApi::new(transport);

        let current = SecretString::new("old-token-xyz");
        let new_token = api
            .refresh_auth_token(&current)
            .expect("refresh should succeed");

        assert_eq!(new_token.expose_secret(), "new-token-abcdef");
        // Old token is untouched (still valid server-side until expiry).
        assert_eq!(current.expose_secret(), "old-token-xyz");
        assert_eq!(
            api.transport
                .applied_hints
                .lock()
                .expect("hints lock should not be poisoned")
                .as_slice(),
            ["bineapi-eu.pcloud.com"]
        );
    }

    #[test]
    fn refresh_auth_token_classifies_expired_result() {
        use pcloud_secret::secret_string::SecretString;

        use crate::auth_api::AuthRefreshError;

        for &code in &[1000_u64, 2000, 2094, 2297] {
            let transport = HintTrackingTransport::with_responses(vec![Value::Hash(vec![
                ("result".to_owned(), Value::Number(code)),
                ("error".to_owned(), Value::String("expired".to_owned())),
            ])]);
            let api = AuthApi::new(transport);
            let current = SecretString::new("expired-token");
            let err = api
                .refresh_auth_token(&current)
                .expect_err("should classify as expired");
            match err {
                AuthRefreshError::AuthExpired(got) => assert_eq!(got, code),
                other => panic!("expected AuthExpired for {code}, got {other:?}"),
            }
        }
    }

    #[test]
    fn refresh_auth_token_classifies_temporary_failure_on_other_nonzero_result() {
        use pcloud_secret::secret_string::SecretString;

        use crate::auth_api::AuthRefreshError;

        let transport = HintTrackingTransport::with_responses(vec![Value::Hash(vec![(
            "result".to_owned(),
            Value::Number(5000),
        )])]);
        let api = AuthApi::new(transport);
        let current = SecretString::new("token");
        let err = api
            .refresh_auth_token(&current)
            .expect_err("non-zero non-auth result must surface as temporary");
        assert!(matches!(err, AuthRefreshError::TemporaryFailure(_)));
    }

    #[test]
    fn refresh_auth_token_reports_missing_auth_field() {
        use pcloud_secret::secret_string::SecretString;

        use crate::auth_api::AuthRefreshError;

        let transport = HintTrackingTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            ("userid".to_owned(), Value::Number(1)),
        ])]);
        let api = AuthApi::new(transport);
        let current = SecretString::new("token");
        let err = api
            .refresh_auth_token(&current)
            .expect_err("missing auth field should error");
        assert!(matches!(err, AuthRefreshError::MissingAuthField));
    }

    #[test]
    fn refresh_auth_token_transport_error_is_temporary() {
        use pcloud_secret::secret_string::SecretString;

        use crate::auth_api::AuthRefreshError;

        // Empty response queue triggers UnexpectedEof from the mock transport.
        let transport = HintTrackingTransport::with_responses(vec![]);
        let api = AuthApi::new(transport);
        let current = SecretString::new("token");
        let err = api
            .refresh_auth_token(&current)
            .expect_err("transport failure should surface");
        assert!(matches!(err, AuthRefreshError::TemporaryFailure(_)));
    }

    #[test]
    fn send_two_factor_notification_parses_devices() {
        let transport = HintTrackingTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            (
                "devices".to_owned(),
                Value::Array(vec![
                    Value::Hash(vec![
                        ("name".to_owned(), Value::String("Pixel".to_owned())),
                        ("type".to_owned(), Value::Number(1)),
                    ]),
                    Value::Hash(vec![("name".to_owned(), Value::String("iPad".to_owned()))]),
                ]),
            ),
        ])]);
        let api = AuthApi::new(transport);

        let delivery = api
            .send_two_factor_notification("challenge-token")
            .expect("notification delivery should parse");

        assert_eq!(delivery.devices.len(), 2);
        assert_eq!(delivery.devices[0].name.as_deref(), Some("Pixel"));
        assert_eq!(delivery.devices[0].device_type, Some(1));
        assert_eq!(delivery.devices[1].name.as_deref(), Some("iPad"));
        assert_eq!(delivery.devices[1].device_type, None);
    }
}
