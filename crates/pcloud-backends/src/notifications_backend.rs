//! Notifications runtime backend.
//!
//! Active-path Rust equivalent of the C notifications surface declared in
//! `pclsync/psynclib.h`:
//!
//! * `psync_notification_list_t *psync_get_notifications()` - pclsync/psynclib.c:248
//! * `int psync_mark_notificaitons_read(uint32_t notificationid)` - pclsync/psynclib.c:324
//!
//! The wire-level encoding lives in [`pcloud_proto::notifications_api`].
//! This backend mirrors the transport-selection pattern used by the other
//! runtimes (account/backup/public-link/sync/transfer) so a single runtime
//! can drive either the deterministic development transport or the live
//! binary API transport, never falling back to plaintext by default.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io;

use pcloud_config::{ConfigProfile, api::ApiMode};
use pcloud_proto::{
    BinaryApiTransport, EncodedRequest, ParseLimits, ResponseParseError, TransportConfig,
    TransportError,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    notifications_api::{Notification, NotificationsApi, NotificationsApiError},
    parse_response_frame,
    response::Value,
};
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use thiserror::Error;

/// Deterministic transport for unit/integration tests. Responses match the
/// shape the C client would observe from the live `listnotifications` and
/// `readnotifications` endpoints.
#[derive(Debug, Clone, Default)]
pub struct DevelopmentNotificationsTransport;

impl ProtocolTransport for DevelopmentNotificationsTransport {
    type Error = io::Error;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        let frame = match request.frame.command.as_str() {
            "listnotifications" => encode_hash_response(&[
                ("result", EncodedValue::Number(0)),
                (
                    "notifications",
                    EncodedValue::Array(vec![
                        EncodedValue::Hash(vec![
                            ("notificationid", EncodedValue::Number(7)),
                            ("notification", EncodedValue::String("Welcome to pCloud")),
                            ("mtime", EncodedValue::Number(1_700_000_000)),
                            ("isnew", EncodedValue::Bool(true)),
                            ("iconid", EncodedValue::Number(1)),
                            ("action", EncodedValue::String("openurl")),
                            ("url", EncodedValue::String("https://www.pcloud.com/")),
                        ]),
                        EncodedValue::Hash(vec![
                            ("notificationid", EncodedValue::Number(8)),
                            ("notification", EncodedValue::String("Shared folder update")),
                            ("mtime", EncodedValue::Number(1_700_000_100)),
                            ("isnew", EncodedValue::Bool(false)),
                            ("iconid", EncodedValue::Number(2)),
                            ("action", EncodedValue::String("gotofolder")),
                            ("folderid", EncodedValue::Number(42)),
                        ]),
                    ]),
                ),
            ]),
            "readnotifications" => {
                let notification_id = number_param(request, "notificationid").unwrap_or(0);
                if notification_id == 0 {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(1076)),
                        ("error", EncodedValue::String("invalid notificationid")),
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

impl ApiServerHintConsumer for DevelopmentNotificationsTransport {
    fn apply_api_server_hint(&self, _api_server: &str) {}
}

fn number_param(request: &EncodedRequest, name: &str) -> Option<u64> {
    request.params.iter().find_map(|param| {
        if param.name == name {
            match &param.value {
                pcloud_proto::BinaryParamValue::Number(value) => Some(*value),
                _ => None,
            }
        } else {
            None
        }
    })
}

#[derive(Debug, Error)]
/// `NotificationsBackendError` enum.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub enum NotificationsBackendError {
    #[error(transparent)]
    /// `Development` variant.
    Development(#[from] io::Error),
    #[error(transparent)]
    /// `Network` variant.
    Network(#[from] TransportError),
}

#[derive(Debug, Clone)]
enum NotificationsTransportMode {
    Development(DevelopmentNotificationsTransport),
    Network(BinaryApiTransport),
}

impl ProtocolTransport for NotificationsTransportMode {
    type Error = NotificationsBackendError;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        match self {
            Self::Development(transport) => transport.execute(request).map_err(Into::into),
            Self::Network(transport) => transport.execute(request).map_err(Into::into),
        }
    }
}

impl ApiServerHintConsumer for NotificationsTransportMode {
    fn apply_api_server_hint(&self, api_server: &str) {
        match self {
            Self::Development(transport) => transport.apply_api_server_hint(api_server),
            Self::Network(transport) => transport.apply_api_server_hint(api_server),
        }
    }
}

#[derive(Debug)]
/// Entry struct for the notifications backend.
///
/// # Architecture role
///
/// - Dispatches `NotificationList` and `NotificationMarkRead` IPC
///   request frames from `pcloud-daemon::dispatch`.
/// - Issues the pCloud protocol methods `listnotifications` and
///   `readnotifications`. Wire encoding uses the crate-level
///   `EncodedValue` pattern.
/// - Emits audit events for mark-as-read mutations; list calls are
///   read-through and not audited.
/// - Persists nothing durably; notification state is canonical on the
///   server. Subscribe/filter helpers operate on in-memory request
///   parameters only.
/// - Error taxonomy: see [`NotificationsBackendError`].
pub struct NotificationsRuntime {
    api: NotificationsApi<NotificationsTransportMode>,
}

impl NotificationsRuntime {
    #[must_use]
    /// Invoke `from_config` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn from_config(config: &ConfigProfile) -> Self {
        let transport = match config.api.mode {
            ApiMode::Development => {
                NotificationsTransportMode::Development(DevelopmentNotificationsTransport)
            }
            ApiMode::Plaintext | ApiMode::Tls => {
                NotificationsTransportMode::Network(BinaryApiTransport::new(TransportConfig {
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
            api: NotificationsApi::new(transport),
        }
    }

    /// Invoke `list_notifications` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn list_notifications(
        &self,
        auth_token: SecretString,
        thumb_size: Option<String>,
    ) -> Result<Vec<Notification>, NotificationsApiError<NotificationsBackendError>> {
        self.api
            .list_notifications(auth_token.expose_secret(), thumb_size)
    }

    /// Invoke `mark_notifications_read` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn mark_notifications_read(
        &self,
        auth_token: SecretString,
        upto_id: u64,
    ) -> Result<(), NotificationsApiError<NotificationsBackendError>> {
        self.api
            .mark_notifications_read(auth_token.expose_secret(), upto_id)
    }

    /// Invoke `apply_api_server_hint` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn apply_api_server_hint(&self, api_server: &str) {
        self.api.apply_api_server_hint(api_server);
    }
}

fn map_response_parse_err(err: ResponseParseError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}

// Shared wire-shape for the binary response encoder. Some variants are
// never constructed by this backend but are retained for parity with the
// C response schema; the match arms in `encode_value` handle them all.
#[allow(dead_code)]
enum EncodedValue<'a> {
    Bool(bool),
    Number(u64),
    String(&'a str),
    OwnedString(String),
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
            EncodedValue::OwnedString(value) => encode_string(payload, value)?,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dev_runtime() -> NotificationsRuntime {
        let transport = NotificationsTransportMode::Development(DevelopmentNotificationsTransport);
        NotificationsRuntime {
            api: NotificationsApi::new(transport),
        }
    }

    #[test]
    fn list_notifications_dev_transport_returns_seed() {
        let runtime = dev_runtime();
        let list = runtime
            .list_notifications(SecretString::new("token".to_owned()), None)
            .expect("list should succeed");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, 7);
        assert_eq!(list[0].text, "Welcome to pCloud");
        assert!(
            !list[0].read,
            "first notification should be unread (isnew=true)"
        );
        assert_eq!(list[1].id, 8);
        assert!(
            list[1].read,
            "second notification should be read (isnew=false)"
        );
    }

    #[test]
    fn mark_notifications_read_dev_transport_success() {
        let runtime = dev_runtime();
        runtime
            .mark_notifications_read(SecretString::new("token".to_owned()), 7)
            .expect("mark read should succeed");
    }

    #[test]
    fn mark_notifications_read_dev_transport_rejects_zero_id() {
        let runtime = dev_runtime();
        let err = runtime
            .mark_notifications_read(SecretString::new("token".to_owned()), 0)
            .expect_err("zero id should be rejected");
        assert!(matches!(
            err,
            NotificationsApiError::Result { result: 1076, .. }
        ));
    }
}

/// Test-only mock fixture for the `notifications_backend` subsystem.
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
    pub const REPRESENTATIVE_COMMAND: &str = "listnotifications";

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

        /// Record the representative notifications runtime call (listnotifications).
        ///
        /// Returns the recorded event so integration tests can assert
        /// on the exact command name without re-reading the recorder.
        pub fn record_representative_call(&self) -> MockEvent {
            self.fixture.proto.call(REPRESENTATIVE_COMMAND, "mock");
            MockEvent::with_payload("proto", REPRESENTATIVE_COMMAND, "mock")
        }
    }
}
