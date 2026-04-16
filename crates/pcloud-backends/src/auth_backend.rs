//! Authentication backend: password/token login, TFA code and recovery
//! code submission, TFA SMS and device-notification resend, and live
//! `userinfo` refresh. Drives the auth vault persistence decisions and
//! is the single entry point for `pcloud-daemon::dispatch` auth frames.
//!
//! Secrets are held in `pcloud-secret::SecretString`/`SecretBytes`;
//! cleartext password persistence is intentionally not reintroduced.
//!
//! Portable; no platform gating.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::{io, sync::Arc};

use pcloud_auth::{AuthFlowError, ProtocolAuthFlow, RefreshTokenError, SessionManager};
use pcloud_config::{ConfigProfile, api::ApiMode};
use pcloud_proto::{
    AuthApi, BinaryApiTransport, BinaryParamValue, EncodedRequest, ParseLimits, ResponseParseError,
    TransportConfig, TransportError, TwoFactorNotificationDelivery, TwoFactorSmsDelivery,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    parse_response_frame,
    response::Value,
};
use pcloud_secret::secret_string::SecretString;
use thiserror::Error;

#[derive(Debug, Clone)]
/// `DevelopmentAuthTransport` struct.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub struct DevelopmentAuthTransport {
    expected_username: Arc<str>,
    expected_password: Arc<str>,
    expected_tfa_code: Arc<str>,
}

impl Default for DevelopmentAuthTransport {
    fn default() -> Self {
        Self {
            expected_username: Arc::from("alice@example.com"),
            expected_password: Arc::from("correct-horse"),
            expected_tfa_code: Arc::from("654321"),
        }
    }
}

impl ProtocolTransport for DevelopmentAuthTransport {
    type Error = io::Error;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        let frame = match request.frame.command.as_str() {
            "getdigest" => encode_hash_response(&[
                ("result", EncodedValue::Number(0)),
                ("digest", EncodedValue::String("development-digest")),
            ]),
            "login" => {
                let username = string_param(request, "username");
                let password = string_param(request, "password");
                let digest = string_param(request, "digest");
                let password_digest = string_param(request, "passworddigest");

                if username == Some(self.expected_username.as_ref())
                    && (password == Some(self.expected_password.as_ref())
                        || (digest == Some("development-digest") && password_digest.is_some()))
                {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2000)),
                        ("token", EncodedValue::String("challenge-token")),
                        ("trustdevice", EncodedValue::Bool(false)),
                    ])
                } else {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(4000)),
                        ("error", EncodedValue::String("invalid credentials")),
                    ])
                }
            }
            "tfa_login" | "tfa_loginwithrecoverycode" => {
                let token = string_param(request, "token");
                let code = string_param(request, "code");

                if token == Some("challenge-token") && code == Some(self.expected_tfa_code.as_ref())
                {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(0)),
                        ("auth", EncodedValue::String("auth-token-42")),
                        ("userid", EncodedValue::Number(42)),
                    ])
                } else {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(4001)),
                        ("error", EncodedValue::String("invalid two-factor code")),
                    ])
                }
            }
            "tfa_sendcodeviasms" => {
                let token = string_param(request, "token");
                if token == Some("challenge-token") {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(0)),
                        (
                            "phonedata",
                            EncodedValue::Hash(vec![
                                ("countrycode", EncodedValue::String("+49")),
                                ("msisdn", EncodedValue::String("123456789")),
                            ]),
                        ),
                    ])
                } else {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(4001)),
                        ("error", EncodedValue::String("missing two-factor token")),
                    ])
                }
            }
            "tfa_sendcodeviasysnotification" => {
                let token = string_param(request, "token");
                if token == Some("challenge-token") {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(0)),
                        (
                            "devices",
                            EncodedValue::Array(vec![
                                EncodedValue::Hash(vec![
                                    ("name", EncodedValue::String("Pixel")),
                                    ("type", EncodedValue::Number(1)),
                                ]),
                                EncodedValue::Hash(vec![("name", EncodedValue::String("iPad"))]),
                            ]),
                        ),
                    ])
                } else {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(4001)),
                        ("error", EncodedValue::String("missing two-factor token")),
                    ])
                }
            }
            "userinfo" => {
                let auth = string_param(request, "auth");
                let username = string_param(request, "username");
                let password = string_param(request, "password");
                let digest = string_param(request, "digest");
                let password_digest = string_param(request, "passworddigest");

                if matches!(auth, Some("auth-token-42" | "digest-auth-token")) {
                    // Include a fresh `auth` field so
                    // `AuthApi::refresh_auth_token` (which forces
                    // `getauth=1`) can extract a new token. Plain
                    // userinfo consumers ignore the extra key. The
                    // returned token is deterministic to keep tests
                    // auditable.
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(0)),
                        ("userid", EncodedValue::Number(42)),
                        (
                            "email",
                            EncodedValue::OwnedString(self.expected_username.to_string()),
                        ),
                        ("auth", EncodedValue::String("auth-token-42-refreshed")),
                    ])
                } else if username == Some(self.expected_username.as_ref())
                    && (password == Some(self.expected_password.as_ref())
                        || (digest == Some("development-digest") && password_digest.is_some()))
                {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2000)),
                        ("token", EncodedValue::String("challenge-token")),
                        ("trustdevice", EncodedValue::Bool(false)),
                    ])
                } else {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(4000)),
                        ("error", EncodedValue::String("invalid credentials")),
                    ])
                }
            }
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported command: {}", request.frame.command),
            )),
        }?;

        parse_response_frame(&frame, &ParseLimits::default()).map_err(map_response_parse_err)
    }
}

impl ApiServerHintConsumer for DevelopmentAuthTransport {
    fn apply_api_server_hint(&self, _api_server: &str) {}
}

#[derive(Debug, Error)]
/// `AuthBackendError` enum.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub enum AuthBackendError {
    #[error(transparent)]
    /// `Development` variant.
    Development(#[from] io::Error),
    #[error(transparent)]
    /// `Network` variant.
    Network(#[from] TransportError),
}

#[derive(Debug, Clone)]
enum AuthTransportMode {
    Development(DevelopmentAuthTransport),
    Network(BinaryApiTransport),
}

impl ProtocolTransport for AuthTransportMode {
    type Error = AuthBackendError;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        match self {
            Self::Development(transport) => {
                transport.execute(request).map_err(AuthBackendError::from)
            }
            Self::Network(transport) => transport.execute(request).map_err(AuthBackendError::from),
        }
    }
}

impl ApiServerHintConsumer for AuthTransportMode {
    fn apply_api_server_hint(&self, api_server: &str) {
        match self {
            Self::Development(transport) => transport.apply_api_server_hint(api_server),
            Self::Network(transport) => transport.apply_api_server_hint(api_server),
        }
    }
}

#[derive(Debug)]
/// Entry struct for the authentication backend.
///
/// # Architecture role
///
/// - Dispatches the `Login`, `TfaSubmit`, `TfaSubmitRecovery`,
///   `TfaResendSms`, `TfaResendNotification`, `Logout`, and `UserInfo`
///   IPC request frames routed by `pcloud-daemon::dispatch`.
/// - Issues the pCloud protocol methods `getdigest`, `login`, `tfa_login`,
///   `tfa_loginwithrecoverycode`, `tfa_sendsms`, `tfa_sendnotification`,
///   `logout`, `userinfo` via a pooled [`BinaryApiTransport`] (production)
///   or a [`DevelopmentAuthTransport`] (tests/offline). Wire encoding
///   uses the crate-level `EncodedValue` pattern.
/// - Emits audit events for login success, TFA challenges, vault writes,
///   and vault read failures through the shared `pcloud-audit` sink.
/// - Persists nothing by default. When `token_persistence = true` in the
///   active [`ConfigProfile`], writes the auth token (never the password)
///   to the owner-only (`0600`/`0700`) auth vault via
///   [`pcloud_auth::SessionManager`]. Cleartext password persistence is
///   intentionally not carried over from the C client.
/// - Error taxonomy: see [`AuthBackendError`].
pub struct AuthRuntime {
    flow: ProtocolAuthFlow<AuthTransportMode>,
}

impl Default for AuthRuntime {
    fn default() -> Self {
        Self {
            flow: ProtocolAuthFlow::new(AuthApi::new(AuthTransportMode::Development(
                DevelopmentAuthTransport::default(),
            ))),
        }
    }
}

impl AuthRuntime {
    #[must_use]
    /// Invoke `from_config` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn from_config(config: &ConfigProfile) -> Self {
        let transport = match config.api.mode {
            ApiMode::Development => {
                AuthTransportMode::Development(DevelopmentAuthTransport::default())
            }
            ApiMode::Plaintext | ApiMode::Tls => {
                AuthTransportMode::Network(BinaryApiTransport::new(TransportConfig {
                    host: config.api.host.clone(),
                    port: config.api.port,
                    server_name: config.api.server_name.clone(),
                    use_tls: matches!(config.api.mode, ApiMode::Tls),
                    connect_timeout: std::time::Duration::from_millis(
                        config.api.connect_timeout_ms,
                    ),
                    read_timeout: std::time::Duration::from_millis(config.api.read_timeout_ms),
                }))
            }
        };

        Self {
            flow: ProtocolAuthFlow::new(AuthApi::new(transport)),
        }
    }

    /// Invoke `login_with_password` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn login_with_password(
        &self,
        session: &mut SessionManager,
        username: String,
        password: SecretString,
    ) -> Result<pcloud_auth::AuthEvent, AuthFlowError<AuthBackendError>> {
        self.flow.login_with_password(session, username, password)
    }

    /// Invoke `login_with_token` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn login_with_token(
        &self,
        session: &mut SessionManager,
        auth_token: SecretString,
    ) -> Result<pcloud_auth::AuthEvent, AuthFlowError<AuthBackendError>> {
        self.flow.login_with_token(session, auth_token)
    }

    /// Invoke `submit_two_factor_code` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn submit_two_factor_code(
        &self,
        session: &mut SessionManager,
        code: SecretString,
        trust_device: bool,
        recovery_code: bool,
    ) -> Result<pcloud_auth::AuthEvent, AuthFlowError<AuthBackendError>> {
        self.flow
            .submit_two_factor_code(session, code, trust_device, recovery_code)
    }

    /// Invoke `submit_two_factor_code_with_password` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn submit_two_factor_code_with_password(
        &self,
        session: &mut SessionManager,
        username: String,
        password: SecretString,
        code: SecretString,
    ) -> Result<pcloud_auth::AuthEvent, AuthFlowError<AuthBackendError>> {
        self.flow
            .submit_two_factor_code_with_password(session, username, password, code)
    }

    /// Invoke `userinfo` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn userinfo(
        &self,
        auth_token: SecretString,
    ) -> Result<pcloud_proto::UserInfo, AuthFlowError<AuthBackendError>> {
        self.flow.userinfo(auth_token)
    }

    /// Invoke `send_two_factor_sms` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn send_two_factor_sms(
        &self,
        session: &SessionManager,
    ) -> Result<TwoFactorSmsDelivery, AuthFlowError<AuthBackendError>> {
        self.flow.send_two_factor_sms(session)
    }

    /// Invoke `send_two_factor_notification` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn send_two_factor_notification(
        &self,
        session: &SessionManager,
    ) -> Result<TwoFactorNotificationDelivery, AuthFlowError<AuthBackendError>> {
        self.flow.send_two_factor_notification(session)
    }

    /// Invoke `apply_api_server_hint` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn apply_api_server_hint(&self, api_server: &str) {
        self.flow.apply_api_server_hint(api_server);
    }

    /// Sub-task 3 (session supervisor): exchange the current auth token
    /// for a fresh one via `userinfo?getauth=1`. The orchestrator owns
    /// the session-state transitions; this method is a thin backend-
    /// aware forwarder so `pcloud_daemon::session_lifecycle::SessionSupervisor`
    /// can plug a concrete transport into the refresh loop without
    /// depending on [`ProtocolAuthFlow`] generics.
    ///
    /// Security: `current` is passed by reference and never logged. The
    /// returned `AuthEvent` payload does not carry the token.
    pub fn refresh_token(
        &self,
        session: &mut SessionManager,
        current: &SecretString,
    ) -> Result<pcloud_auth::AuthEvent, RefreshTokenError<AuthBackendError>> {
        self.flow.refresh_token(session, current)
    }
}

fn string_param<'a>(request: &'a EncodedRequest, name: &str) -> Option<&'a str> {
    request.params.iter().find_map(|param| {
        if param.name == name {
            match &param.value {
                BinaryParamValue::String(value) => Some(value.as_str()),
                _ => None,
            }
        } else {
            None
        }
    })
}

fn map_response_parse_err(err: ResponseParseError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}

enum EncodedValue<'a> {
    Bool(bool),
    Number(u64),
    String(&'a str),
    OwnedString(String),
    Hash(Vec<(&'a str, EncodedValue<'a>)>),
    Array(Vec<EncodedValue<'a>>),
}

fn encode_hash_response(entries: &[(&str, EncodedValue<'_>)]) -> Result<Vec<u8>, io::Error> {
    const RPARAM_HASH: u8 = 16;
    const RPARAM_END: u8 = 255;

    let mut payload = vec![RPARAM_HASH];
    for (key, value) in entries {
        encode_string(&mut payload, key)?;
        encode_value(&mut payload, value)?;
    }
    payload.push(RPARAM_END);

    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

fn encode_value(payload: &mut Vec<u8>, value: &EncodedValue<'_>) -> Result<(), io::Error> {
    const RPARAM_NUM8: u8 = 15;
    const RPARAM_HASH: u8 = 16;
    const RPARAM_ARRAY: u8 = 17;
    const RPARAM_BFALSE: u8 = 18;
    const RPARAM_BTRUE: u8 = 19;
    const RPARAM_SMALL_NUM_BASE: u8 = 200;
    const RPARAM_END: u8 = 255;

    match value {
        EncodedValue::Bool(false) => payload.push(RPARAM_BFALSE),
        EncodedValue::Bool(true) => payload.push(RPARAM_BTRUE),
        EncodedValue::Number(number) if *number < 20 => {
            payload.push(RPARAM_SMALL_NUM_BASE + (*number as u8));
        }
        EncodedValue::Number(number) => {
            payload.push(RPARAM_NUM8);
            payload.extend_from_slice(&number.to_le_bytes());
        }
        EncodedValue::String(value) => encode_string(payload, value)?,
        EncodedValue::OwnedString(value) => encode_string(payload, value)?,
        EncodedValue::Hash(entries) => {
            payload.push(RPARAM_HASH);
            for (key, value) in entries {
                encode_string(payload, key)?;
                encode_value(payload, value)?;
            }
            payload.push(RPARAM_END);
        }
        EncodedValue::Array(entries) => {
            payload.push(RPARAM_ARRAY);
            for value in entries {
                encode_value(payload, value)?;
            }
            payload.push(RPARAM_END);
        }
    }

    Ok(())
}

fn encode_string(payload: &mut Vec<u8>, value: &str) -> Result<(), io::Error> {
    const RPARAM_SHORT_STR_BASE: u8 = 100;
    if value.len() > 49 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "development response encoder only supports short strings",
        ));
    }
    payload.push(RPARAM_SHORT_STR_BASE + value.len() as u8);
    payload.extend_from_slice(value.as_bytes());
    Ok(())
}

/// Test-only mock fixture for the `auth_backend` subsystem.
///
/// Promoted from the `pcloud-fs` mock-backend pattern (R18 wave-01
/// audit ask) so this backend can be driven by integration tests
/// without a live transport or store. The fixture wraps the shared
/// [`crate::mock::MockFixture`] recorders and exposes a representative
/// call helper that records the canonical protocol command this
/// backend issues on its happy path.
///
/// The fixture is `Send + Sync`, deterministic (no sleeps or clocks),
/// and cheap to construct via [`Default`].
pub mod mock {
    use crate::mock::{MockEvent, MockFixture};

    /// Canonical protocol command exercised by [`Fixture::record_representative_call`].
    pub const REPRESENTATIVE_COMMAND: &str = "userinfo";

    /// Thin wrapper around [`MockFixture`] specialised for this backend.
    #[derive(Debug, Default)]
    pub struct Fixture {
        /// Underlying shared recorders.
        pub fixture: MockFixture,
    }

    impl Fixture {
        /// Construct a new mock fixture for this backend.
        pub fn new() -> Self {
            Self::default()
        }

        /// Record the representative auth runtime call (userinfo).
        ///
        /// Returns the recorded event so integration tests can assert
        /// on the exact command name without re-reading the recorder.
        pub fn record_representative_call(&self) -> MockEvent {
            self.fixture.proto.call(REPRESENTATIVE_COMMAND, "mock");
            MockEvent::with_payload("proto", REPRESENTATIVE_COMMAND, "mock")
        }
    }
}
