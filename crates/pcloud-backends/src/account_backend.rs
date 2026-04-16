//! Account-level operations backend: email verification, lost/change
//! password, registration, promo codes, API-server selection, and
//! language preference. Called from `pcloud-daemon::dispatch` and the
//! SDK surface; wraps `pcloud-proto::account_api`.
//!
//! Portable; no platform gating.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io;

use pcloud_config::{ConfigProfile, api::ApiMode};
use pcloud_proto::{
    ApiServerInfo, BinaryApiTransport, EncodedRequest, ParseLimits, PromoInfo, ResponseParseError,
    TransportConfig, TransportError,
    account_api::{AccountApi, AccountApiError, PasswordChangeResult},
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    parse_response_frame,
    response::Value,
};
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use thiserror::Error;

#[derive(Debug, Clone, Default)]
/// `DevelopmentAccountTransport` struct.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub struct DevelopmentAccountTransport;

impl ProtocolTransport for DevelopmentAccountTransport {
    type Error = io::Error;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        let frame = match request.frame.command.as_str() {
            "getlocationapi" => encode_hash_response(&[(
                "locations",
                EncodedValue::Array(vec![
                    EncodedValue::Hash(vec![
                        ("label", EncodedValue::String("Europe")),
                        ("api", EncodedValue::String("api.pcloud.com")),
                        ("binapi", EncodedValue::String("bineapi-eu.pcloud.com")),
                        ("id", EncodedValue::Number(2)),
                    ]),
                    EncodedValue::Hash(vec![
                        ("label", EncodedValue::String("US")),
                        ("api", EncodedValue::String("api-us.pcloud.com")),
                        ("binapi", EncodedValue::String("bineapi-us.pcloud.com")),
                        ("id", EncodedValue::Number(1)),
                    ]),
                ]),
            )]),
            "getpromourl" => encode_hash_response(&[
                ("haspromo", EncodedValue::Bool(true)),
                ("url", EncodedValue::String("https://promo.example/banner")),
                ("width", EncodedValue::Number(640)),
                ("height", EncodedValue::Number(480)),
            ]),
            "setlanguage" => {
                let language = string_param(request, "language").unwrap_or("");
                if language.len() != 2 {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2000)),
                        ("error", EncodedValue::String("invalid language")),
                    ])
                } else {
                    encode_hash_response(&[("result", EncodedValue::Number(0))])
                }
            }
            "sendverificationemail" => {
                let has_auth = string_param(request, "auth").is_some();
                let has_verify_token = string_param(request, "verifytoken").is_some();
                if !has_auth && !has_verify_token {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2000)),
                        (
                            "error",
                            EncodedValue::String("missing verification context"),
                        ),
                    ])
                } else {
                    encode_hash_response(&[("result", EncodedValue::Number(0))])
                }
            }
            "lostpassword" => {
                let mail = string_param(request, "mail").unwrap_or("");
                if !mail.contains('@') {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2000)),
                        ("error", EncodedValue::String("unknown email")),
                    ])
                } else {
                    encode_hash_response(&[("result", EncodedValue::Number(0))])
                }
            }
            "changepassword" => {
                let oldpassword = string_param(request, "oldpassword").unwrap_or("");
                let newpassword = string_param(request, "newpassword").unwrap_or("");
                if oldpassword.is_empty() || newpassword.len() < 3 {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2000)),
                        ("error", EncodedValue::String("invalid password change")),
                    ])
                } else {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(0)),
                        ("auth", EncodedValue::String("rotated-auth-token")),
                    ])
                }
            }
            "register" => {
                let mail = string_param(request, "mail").unwrap_or("");
                let password = string_param(request, "password").unwrap_or("");
                let terms = string_param(request, "termsaccepted").unwrap_or("");
                if !mail.contains('@') || password.len() < 6 {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2000)),
                        ("error", EncodedValue::String("invalid register params")),
                    ])
                } else if terms != "yes" {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2001)),
                        ("error", EncodedValue::String("terms not accepted")),
                    ])
                } else {
                    encode_hash_response(&[("result", EncodedValue::Number(0))])
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

impl ApiServerHintConsumer for DevelopmentAccountTransport {
    fn apply_api_server_hint(&self, _api_server: &str) {}
}

fn string_param<'a>(request: &'a EncodedRequest, name: &str) -> Option<&'a str> {
    request.params.iter().find_map(|param| {
        if param.name == name {
            match &param.value {
                pcloud_proto::BinaryParamValue::String(value) => Some(value.as_str()),
                _ => None,
            }
        } else {
            None
        }
    })
}

#[derive(Debug, Error)]
/// `AccountBackendError` enum.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub enum AccountBackendError {
    #[error(transparent)]
    /// `Development` variant.
    Development(#[from] io::Error),
    #[error(transparent)]
    /// `Network` variant.
    Network(#[from] TransportError),
}

#[derive(Debug, Clone)]
enum AccountTransportMode {
    Development(DevelopmentAccountTransport),
    Network(BinaryApiTransport),
}

impl ProtocolTransport for AccountTransportMode {
    type Error = AccountBackendError;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        match self {
            Self::Development(transport) => transport
                .execute(request)
                .map_err(AccountBackendError::from),
            Self::Network(transport) => transport
                .execute(request)
                .map_err(AccountBackendError::from),
        }
    }
}

impl ApiServerHintConsumer for AccountTransportMode {
    fn apply_api_server_hint(&self, api_server: &str) {
        match self {
            Self::Development(transport) => transport.apply_api_server_hint(api_server),
            Self::Network(transport) => transport.apply_api_server_hint(api_server),
        }
    }
}

#[derive(Debug)]
/// Entry struct for the account-operations backend.
///
/// # Architecture role
///
/// - Dispatches `GetApiServers`, `SetApiServer`, `SetLanguage`,
///   `GetPromo`, `VerifyEmail`, `VerifyEmailRestricted`, `LostPassword`,
///   `ChangePassword`, and `Register` IPC request frames from
///   `pcloud-daemon::dispatch` and the SDK surface.
/// - Issues the pCloud protocol methods `getlocationapi`, `getpromourl`,
///   `setlanguage`, `sendverificationemail`,
///   `sendverificationemailrestricted`, `lostpassword`, `changepassword`,
///   and `register` via a pooled [`BinaryApiTransport`]. Wire encoding
///   uses the crate-level `EncodedValue` pattern.
/// - Emits audit events for password mutations and API-server selection
///   changes; no audit is emitted for read-only promo/language queries.
/// - Persists nothing durably. API-server selection is applied to the
///   in-memory `ConfigProfile::api_mode` only; disk persistence of the
///   chosen server is the caller's responsibility (CLI/SDK).
/// - Error taxonomy: see [`AccountBackendError`].
pub struct AccountRuntime {
    api: AccountApi<AccountTransportMode>,
}

impl AccountRuntime {
    #[must_use]
    /// Invoke `from_config` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn from_config(config: &ConfigProfile) -> Self {
        let transport = match config.api.mode {
            ApiMode::Development => AccountTransportMode::Development(DevelopmentAccountTransport),
            ApiMode::Plaintext | ApiMode::Tls => {
                AccountTransportMode::Network(BinaryApiTransport::new(TransportConfig {
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
            api: AccountApi::new(transport),
        }
    }

    /// Invoke `get_api_servers` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn get_api_servers(
        &self,
    ) -> Result<Vec<ApiServerInfo>, AccountApiError<AccountBackendError>> {
        self.api.get_api_servers()
    }

    /// Invoke `get_promo` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn get_promo(
        &self,
        auth_token: SecretString,
    ) -> Result<Option<PromoInfo>, AccountApiError<AccountBackendError>> {
        self.api.get_promo(auth_token.expose_secret(), 3)
    }

    /// Invoke `set_language` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn set_language(
        &self,
        auth_token: SecretString,
        language: &str,
    ) -> Result<(), AccountApiError<AccountBackendError>> {
        self.api.set_language(auth_token.expose_secret(), language)
    }

    /// Invoke `verify_email` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn verify_email(
        &self,
        auth_token: SecretString,
    ) -> Result<(), AccountApiError<AccountBackendError>> {
        self.api.verify_email(auth_token.expose_secret())
    }

    /// Invoke `verify_email_restricted` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn verify_email_restricted(
        &self,
        verify_token: &str,
    ) -> Result<(), AccountApiError<AccountBackendError>> {
        self.api.verify_email_restricted(verify_token)
    }

    /// Invoke `lost_password` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn lost_password(&self, email: &str) -> Result<(), AccountApiError<AccountBackendError>> {
        self.api.lost_password(email)
    }

    /// Invoke `change_password` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn change_password(
        &self,
        auth_token: SecretString,
        current_password: &str,
        new_password: &str,
        device: &str,
    ) -> Result<PasswordChangeResult, AccountApiError<AccountBackendError>> {
        self.api.change_password(
            auth_token.expose_secret(),
            current_password,
            new_password,
            device,
        )
    }

    /// Invoke `register` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn register(
        &self,
        email: &str,
        password: SecretString,
        terms_accepted: bool,
        os_id: u64,
    ) -> Result<(), AccountApiError<AccountBackendError>> {
        self.api
            .register(email, password.expose_secret(), terms_accepted, os_id)
    }

    /// Invoke `apply_api_server_hint` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn apply_api_server_hint(&self, api_server: &str) {
        self.api.apply_api_server_hint(api_server);
    }
}

/// Enforce the data-residency policy at the `set_api_server` call site.
///
/// Refuses to pin the daemon to an API host outside the allow-list when
/// strict mode is enabled. The resolver classifies the raw host string
/// via [`crate::residency::resolve_region_from_host`] — unknown hosts
/// always refuse under strict mode. The audit event carries
/// `action = "set_api_server"` so operators can distinguish it from the
/// sync-root and upload call sites.
#[must_use]
pub fn enforce_set_api_server_residency(
    policy: &pcloud_config::data_residency::DataResidencyPolicy,
    api_server: &str,
) -> (
    crate::residency::ResidencyDecision,
    crate::residency::ResidencyAuditEvent,
) {
    let region = crate::residency::resolve_region_from_host(api_server);
    crate::residency::enforce(policy, region, crate::residency::ACTION_SET_API_SERVER)
}

fn map_response_parse_err(err: ResponseParseError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}

enum EncodedValue<'a> {
    Bool(bool),
    Number(u64),
    String(&'a str),
    Array(Vec<EncodedValue<'a>>),
    Hash(Vec<(&'a str, EncodedValue<'a>)>),
}

fn encode_hash_response(entries: &[(&str, EncodedValue<'_>)]) -> Result<Vec<u8>, io::Error> {
    const RPARAM_NUM8: u8 = 15;
    const RPARAM_HASH: u8 = 16;
    const RPARAM_ARRAY: u8 = 17;
    const RPARAM_BFALSE: u8 = 18;
    const RPARAM_BTRUE: u8 = 19;
    const RPARAM_SMALL_NUM_BASE: u8 = 200;
    const RPARAM_END: u8 = 255;

    fn encode_value(payload: &mut Vec<u8>, value: &EncodedValue<'_>) -> Result<(), io::Error> {
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
            EncodedValue::Array(values) => {
                payload.push(RPARAM_ARRAY);
                for value in values {
                    encode_value(payload, value)?;
                }
                payload.push(RPARAM_END);
            }
            EncodedValue::Hash(entries) => {
                payload.push(RPARAM_HASH);
                for (key, value) in entries {
                    encode_string(payload, key)?;
                    encode_value(payload, value)?;
                }
                payload.push(RPARAM_END);
            }
        }
        Ok(())
    }

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

/// Test-only mock fixture for the `account_backend` subsystem.
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
    pub const REPRESENTATIVE_COMMAND: &str = "setlanguage";

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

        /// Record the representative account runtime call (setlanguage).
        ///
        /// Returns the recorded event so integration tests can assert
        /// on the exact command name without re-reading the recorder.
        pub fn record_representative_call(&self) -> MockEvent {
            self.fixture.proto.call(REPRESENTATIVE_COMMAND, "mock");
            MockEvent::with_payload("proto", REPRESENTATIVE_COMMAND, "mock")
        }
    }
}
