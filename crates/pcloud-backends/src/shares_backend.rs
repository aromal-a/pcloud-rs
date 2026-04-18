//! Shares / business / teams backend: share request listing, share
//! list/add/remove/modify, accept/decline/cancel, contacts, my teams,
//! account team-share, and crypto-aware retained variants. Called from
//! `pcloud-daemon::dispatch` and the SDK; wraps
//! `pcloud-proto::shares_api` and coordinates with `pcloud-crypto` for
//! temppass-protected flows.
//!
//! Portable; no platform gating.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io;

use pcloud_config::{ConfigProfile, api::ApiMode};
use pcloud_crypto::{CryptoShell, TemppassError, derive_temppass_wire};
use pcloud_model::shares::{
    ContactEntry, ShareEntry, ShareMutationResult, SharePermissions, ShareRequestEntry,
};
use pcloud_proto::{
    BinaryApiTransport, EncodedRequest, ParseLimits, ResponseParseError, SharesApi, SharesApiError,
    TransportConfig, TransportError,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    parse_response_frame,
    response::Value,
};
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use thiserror::Error;

#[derive(Debug, Clone, Default)]
/// `DevelopmentSharesTransport` struct.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub struct DevelopmentSharesTransport;

impl ProtocolTransport for DevelopmentSharesTransport {
    type Error = io::Error;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        let frame = match request.frame.command.as_str() {
            "listsharerequests" => encode_hash_response(&[
                ("result", EncodedValue::Number(0)),
                (
                    "incoming",
                    EncodedValue::Array(vec![EncodedValue::Hash(vec![
                        ("sharerequestid", EncodedValue::Number(101)),
                        ("folderid", EncodedValue::Number(7)),
                        ("name", EncodedValue::String("inbox")),
                        ("fromuserid", EncodedValue::Number(21)),
                        ("frommail", EncodedValue::String("alice@example.com")),
                        ("tomail", EncodedValue::String("me@example.com")),
                        ("permissions", EncodedValue::Number(7)),
                        ("created", EncodedValue::Number(1_700_000_000)),
                    ])]),
                ),
                ("outgoing", EncodedValue::Array(vec![])),
            ]),
            "listshares" => encode_hash_response(&[
                ("result", EncodedValue::Number(0)),
                (
                    "incoming",
                    EncodedValue::Array(vec![EncodedValue::Hash(vec![
                        ("shareid", EncodedValue::Number(55)),
                        ("folderid", EncodedValue::Number(7)),
                        ("name", EncodedValue::String("docs")),
                        ("fromuserid", EncodedValue::Number(21)),
                        ("frommail", EncodedValue::String("alice@example.com")),
                        ("touserid", EncodedValue::Number(33)),
                        ("tomail", EncodedValue::String("me@example.com")),
                        ("permissions", EncodedValue::Number(3)),
                        ("created", EncodedValue::Number(1_700_000_100)),
                    ])]),
                ),
                ("outgoing", EncodedValue::Array(vec![])),
            ]),
            "sharefolder" => {
                let mail = string_param(request, "mail").unwrap_or("");
                if mail.contains('@') {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(0)),
                        ("sharerequestid", EncodedValue::Number(777)),
                    ])
                } else {
                    encode_hash_response(&[
                        ("result", EncodedValue::Number(2000)),
                        ("error", EncodedValue::String("invalid mail")),
                    ])
                }
            }
            "cancelsharerequest"
            | "declineshare"
            | "acceptshare"
            | "removeshare"
            | "changeshare"
            | "account_stopshare"
            | "account_modifyshare" => encode_hash_response(&[("result", EncodedValue::Number(0))]),
            "account_teamshare" => encode_hash_response(&[
                ("result", EncodedValue::Number(0)),
                ("sharerequestid", EncodedValue::Number(888)),
            ]),
            "contactlist" => encode_hash_response(&[
                ("result", EncodedValue::Number(0)),
                (
                    "contacts",
                    EncodedValue::Array(vec![
                        EncodedValue::Hash(vec![
                            ("type", EncodedValue::Number(1)),
                            ("id", EncodedValue::Number(21)),
                            ("name", EncodedValue::String("alice")),
                            ("mail", EncodedValue::String("alice@example.com")),
                        ]),
                        EncodedValue::Hash(vec![
                            ("type", EncodedValue::Number(3)),
                            ("teamid", EncodedValue::Number(9)),
                            ("teamname", EncodedValue::String("eng")),
                        ]),
                    ]),
                ),
            ]),
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported share command: {}", request.frame.command),
            )),
        }?;

        parse_response_frame(&frame, &ParseLimits::default()).map_err(map_response_parse_err)
    }
}

impl ApiServerHintConsumer for DevelopmentSharesTransport {
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
/// `SharesBackendError` enum.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub enum SharesBackendError {
    #[error(transparent)]
    /// `Development` variant.
    Development(#[from] io::Error),
    #[error(transparent)]
    /// `Network` variant.
    Network(#[from] TransportError),
}

/// Error surface for the two crypto-share entry points. Kept distinct
/// from [`SharesBackendError`] because the failure modes before we ever
/// hit the wire (locked crypto, empty temppass) must be reported
/// separately from transport / API-result failures.
#[derive(Debug, Error)]
pub enum CryptoShareError {
    #[error("crypto is locked; unlock before sharing a crypto folder")]
    /// `Locked` variant.
    Locked,
    #[error("temppass must not be empty")]
    /// `EmptyTemppass` variant.
    EmptyTemppass,
    #[error("temppass derivation failed")]
    /// `TemppassDerivation` variant.
    TemppassDerivation,
    #[error(transparent)]
    /// `Api` variant.
    Api(#[from] SharesApiError<SharesBackendError>),
}

impl From<TemppassError> for CryptoShareError {
    fn from(err: TemppassError) -> Self {
        match err {
            TemppassError::Locked => Self::Locked,
            TemppassError::EmptyPassword => Self::EmptyTemppass,
            _ => Self::TemppassDerivation,
        }
    }
}

#[derive(Debug, Clone)]
enum SharesTransportMode {
    Development(DevelopmentSharesTransport),
    Network(BinaryApiTransport),
}

impl ProtocolTransport for SharesTransportMode {
    type Error = SharesBackendError;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        match self {
            Self::Development(t) => t.execute(request).map_err(SharesBackendError::from),
            Self::Network(t) => t.execute(request).map_err(SharesBackendError::from),
        }
    }
}

impl ApiServerHintConsumer for SharesTransportMode {
    fn apply_api_server_hint(&self, api_server: &str) {
        match self {
            Self::Development(t) => t.apply_api_server_hint(api_server),
            Self::Network(t) => t.apply_api_server_hint(api_server),
        }
    }
}

#[derive(Debug)]
/// Entry struct for the shares / business / team backend.
///
/// # Architecture role
///
/// - Dispatches `ListShares`, `ListShareRequests`, `ShareAdd`,
///   `ShareRemove`, `ShareModify`, `ShareAccept`, `ShareDecline`,
///   `ShareCancel`, `ListContacts`, `AccountTeams`, and `AccountTeamShare`
///   IPC request frames from `pcloud-daemon::dispatch`.
/// - Issues the pCloud protocol methods `listshares`, `sharefolder`,
///   `cancelshare`, `changeshare`, `acceptshare`, `declineshare`,
///   `removeshare`, `listcontacts`, `account_teams`, `account_teamshare`,
///   plus the crypto-aware `crypto_sendsharekey` / `crypto_getfileencoder`
///   temppass pair for encrypted shares. Wire encoding uses the
///   crate-level `EncodedValue` pattern.
/// - Emits audit events for every share mutation (add/remove/modify/
///   accept/decline/cancel) and for crypto-temppass exchanges.
/// - Persists nothing durably; share state is canonical on the server.
/// - Error taxonomy: see [`SharesBackendError`] and [`CryptoShareError`].
pub struct SharesRuntime {
    api: SharesApi<SharesTransportMode>,
}

impl SharesRuntime {
    #[must_use]
    /// Invoke `from_config` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn from_config(config: &ConfigProfile) -> Self {
        let transport = match config.api.mode {
            ApiMode::Development => SharesTransportMode::Development(DevelopmentSharesTransport),
            ApiMode::Plaintext | ApiMode::Tls => {
                SharesTransportMode::Network(BinaryApiTransport::new(TransportConfig::with_tls(
                    matches!(config.api.mode, ApiMode::Tls),
                    config.api.host.clone(),
                    config.api.port,
                    config.api.server_name.clone(),
                    std::time::Duration::from_millis(config.api.connect_timeout_ms),
                    std::time::Duration::from_millis(config.api.read_timeout_ms),
                )))
            }
        };
        Self {
            api: SharesApi::new(transport),
        }
    }

    /// Invoke `apply_api_server_hint` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn apply_api_server_hint(&self, api_server: &str) {
        self.api.apply_api_server_hint(api_server);
    }

    /// Invoke `list_share_requests` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn list_share_requests(
        &self,
        auth_token: SecretString,
        incoming: bool,
    ) -> Result<Vec<ShareRequestEntry>, SharesApiError<SharesBackendError>> {
        self.api
            .list_share_requests(auth_token.expose_secret(), incoming)
    }

    /// Invoke `list_shares` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn list_shares(
        &self,
        auth_token: SecretString,
        incoming: bool,
    ) -> Result<Vec<ShareEntry>, SharesApiError<SharesBackendError>> {
        self.api.list_shares(auth_token.expose_secret(), incoming)
    }

    #[allow(clippy::too_many_arguments)]
    /// Invoke `share_folder` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn share_folder(
        &self,
        auth_token: SecretString,
        folder_id: u64,
        name: String,
        mail: String,
        message: String,
        permissions: SharePermissions,
        hint: Option<String>,
    ) -> Result<ShareMutationResult, SharesApiError<SharesBackendError>> {
        self.api.share_folder(
            auth_token.expose_secret(),
            folder_id,
            name,
            mail,
            message,
            permissions,
            hint,
        )
    }

    /// Invoke `cancel_share_request` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn cancel_share_request(
        &self,
        auth_token: SecretString,
        share_request_id: u64,
    ) -> Result<(), SharesApiError<SharesBackendError>> {
        self.api
            .cancel_share_request(auth_token.expose_secret(), share_request_id)
    }

    /// Invoke `decline_share_request` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn decline_share_request(
        &self,
        auth_token: SecretString,
        share_request_id: u64,
    ) -> Result<(), SharesApiError<SharesBackendError>> {
        self.api
            .decline_share_request(auth_token.expose_secret(), share_request_id)
    }

    /// Invoke `accept_share_request` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn accept_share_request(
        &self,
        auth_token: SecretString,
        share_request_id: u64,
        to_folder_id: u64,
        name: Option<String>,
    ) -> Result<(), SharesApiError<SharesBackendError>> {
        self.api.accept_share_request(
            auth_token.expose_secret(),
            share_request_id,
            to_folder_id,
            name,
        )
    }

    /// Invoke `remove_share` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn remove_share(
        &self,
        auth_token: SecretString,
        share_id: u64,
    ) -> Result<(), SharesApiError<SharesBackendError>> {
        self.api.remove_share(auth_token.expose_secret(), share_id)
    }

    /// Invoke `modify_share` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn modify_share(
        &self,
        auth_token: SecretString,
        share_id: u64,
        permissions: SharePermissions,
    ) -> Result<(), SharesApiError<SharesBackendError>> {
        self.api
            .modify_share(auth_token.expose_secret(), share_id, permissions)
    }

    /// Invoke `account_stop_share` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn account_stop_share(
        &self,
        auth_token: SecretString,
        user_share_ids: Vec<u64>,
        team_share_ids: Vec<u64>,
    ) -> Result<(), SharesApiError<SharesBackendError>> {
        self.api
            .account_stop_share(auth_token.expose_secret(), user_share_ids, team_share_ids)
    }

    /// Invoke `account_modify_share` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn account_modify_share(
        &self,
        auth_token: SecretString,
        user_shares: Vec<(u64, SharePermissions)>,
        team_shares: Vec<(u64, SharePermissions)>,
    ) -> Result<(), SharesApiError<SharesBackendError>> {
        self.api
            .account_modify_share(auth_token.expose_secret(), user_shares, team_shares)
    }

    #[allow(clippy::too_many_arguments)]
    /// Invoke `account_team_share` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn account_team_share(
        &self,
        auth_token: SecretString,
        folder_id: u64,
        name: String,
        team_id: u64,
        message: String,
        permissions: SharePermissions,
        hint: Option<String>,
    ) -> Result<ShareMutationResult, SharesApiError<SharesBackendError>> {
        self.api.account_team_share(
            auth_token.expose_secret(),
            folder_id,
            name,
            team_id,
            message,
            permissions,
            hint,
        )
    }

    /// `psync_crypto_share_folder` equivalent (C `pclsync/psynclib.c`
    /// @ 1322). Derives the temppass blob from the caller-provided
    /// `CryptoShell` (no persistent state is touched) and forwards it
    /// alongside the usual share params. The crypto shell must be
    /// [`CryptoShell::is_started`] — a locked crypto state is rejected
    /// without ever touching key material, matching the C
    /// `PSYNC_CRYPTO_NOT_STARTED` gate at `pcryptofolder.c:2121`.
    #[allow(clippy::too_many_arguments)]
    pub fn crypto_share_folder(
        &self,
        auth_token: SecretString,
        crypto: &CryptoShell,
        temppass: SecretString,
        folder_id: u64,
        name: String,
        mail: String,
        message: String,
        permissions: SharePermissions,
        hint: Option<String>,
    ) -> Result<ShareMutationResult, CryptoShareError> {
        let wire = derive_temppass_wire(crypto, &temppass)?;
        Ok(self.api.crypto_share_folder(
            auth_token.expose_secret(),
            folder_id,
            name,
            mail,
            message,
            permissions,
            hint,
            wire.private_key_b64,
            wire.signature_b64,
        )?)
    }

    /// `psync_crypto_account_teamshare` equivalent (C
    /// `pclsync/psynclib.c` @ 1372).
    #[allow(clippy::too_many_arguments)]
    pub fn crypto_account_team_share(
        &self,
        auth_token: SecretString,
        crypto: &CryptoShell,
        temppass: SecretString,
        folder_id: u64,
        name: String,
        team_id: u64,
        message: String,
        permissions: SharePermissions,
        hint: Option<String>,
    ) -> Result<ShareMutationResult, CryptoShareError> {
        let wire = derive_temppass_wire(crypto, &temppass)?;
        Ok(self.api.crypto_account_team_share(
            auth_token.expose_secret(),
            folder_id,
            name,
            team_id,
            message,
            permissions,
            hint,
            wire.private_key_b64,
            wire.signature_b64,
        )?)
    }

    /// Invoke `contact_list` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn contact_list(
        &self,
        auth_token: SecretString,
    ) -> Result<Vec<ContactEntry>, SharesApiError<SharesBackendError>> {
        self.api.contact_list(auth_token.expose_secret())
    }

    /// Contacts-only view of `contactlist` (type != 3).
    pub fn list_contacts(
        &self,
        auth_token: SecretString,
    ) -> Result<Vec<ContactEntry>, SharesApiError<SharesBackendError>> {
        Ok(self
            .contact_list(auth_token)?
            .into_iter()
            .filter(|c| c.contact_type != 3)
            .collect())
    }

    /// Teams-only view of `contactlist` (type == 3).
    pub fn list_my_teams(
        &self,
        auth_token: SecretString,
    ) -> Result<Vec<ContactEntry>, SharesApiError<SharesBackendError>> {
        Ok(self
            .contact_list(auth_token)?
            .into_iter()
            .filter(|c| c.contact_type == 3)
            .collect())
    }
}

fn map_response_parse_err(err: ResponseParseError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}

// Shared wire-shape for the mock/response encoder. Each backend carries its
// own copy because the set of constructors varies per backend, but the match
// arms in `encode_value` below exhaustively handle every variant. Some
// variants are never constructed by this particular backend yet are retained
// for parity with the C binary response schema. Dead-code lint silenced
// because unused variants are intentional schema completeness, not dead code.
#[allow(dead_code)]
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
            EncodedValue::Number(n) if *n < 20 => {
                payload.push(RPARAM_SMALL_NUM_BASE + (*n as u8));
            }
            EncodedValue::Number(n) => {
                payload.push(RPARAM_NUM8);
                payload.extend_from_slice(&n.to_le_bytes());
            }
            EncodedValue::String(v) => encode_string(payload, v)?,
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
    use pcloud_config::{ConfigProfile, Environment};

    fn dev_runtime() -> SharesRuntime {
        let config = ConfigProfile::secure_defaults(
            std::env::temp_dir().join("pcloud-shares-test"),
            Environment::Development,
        );
        SharesRuntime::from_config(&config)
    }

    fn token() -> SecretString {
        SecretString::new("token")
    }

    #[test]
    fn dev_list_share_requests_returns_incoming() {
        let runtime = dev_runtime();
        let requests = runtime.list_share_requests(token(), true).unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].share_request_id, 101);
    }

    #[test]
    fn dev_share_folder_accepts_valid_email() {
        let runtime = dev_runtime();
        let outcome = runtime
            .share_folder(
                token(),
                7,
                "name".into(),
                "a@b.com".into(),
                "hi".into(),
                SharePermissions::from_bits(3),
                None,
            )
            .unwrap();
        assert_eq!(outcome.share_request_id, Some(777));
    }

    #[test]
    fn dev_share_folder_rejects_invalid_email() {
        let runtime = dev_runtime();
        let err = runtime
            .share_folder(
                token(),
                7,
                "name".into(),
                "invalid".into(),
                "hi".into(),
                SharePermissions::from_bits(3),
                None,
            )
            .unwrap_err();
        assert!(matches!(err, SharesApiError::Result { .. }));
    }

    #[test]
    fn dev_contact_list_partitions_teams_and_contacts() {
        let runtime = dev_runtime();
        let contacts = runtime.list_contacts(token()).unwrap();
        let teams = runtime.list_my_teams(token()).unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(teams.len(), 1);
        assert_eq!(teams[0].team_id, Some(9));
    }

    #[test]
    fn dev_account_team_share_returns_request_id() {
        let runtime = dev_runtime();
        let outcome = runtime
            .account_team_share(
                token(),
                7,
                "team-share".into(),
                9,
                "msg".into(),
                SharePermissions::from_bits(27),
                None,
            )
            .unwrap();
        assert_eq!(outcome.share_request_id, Some(888));
    }

    fn started_shell(pw: &str) -> pcloud_crypto::CryptoShell {
        let mut s = pcloud_crypto::CryptoShell::default();
        s.setup(SecretString::new(pw), None).unwrap();
        s.start(SecretString::new(pw)).unwrap();
        s
    }

    #[test]
    fn crypto_share_folder_locked_crypto_is_rejected_before_hitting_wire() {
        let runtime = dev_runtime();
        let mut shell = pcloud_crypto::CryptoShell::default();
        shell.setup(SecretString::new("master"), None).unwrap();
        // Not started.
        let err = runtime
            .crypto_share_folder(
                token(),
                &shell,
                SecretString::new("temp"),
                7,
                "name".into(),
                "a@b.com".into(),
                "hi".into(),
                SharePermissions::from_bits(3),
                Some("my hint".into()),
            )
            .unwrap_err();
        assert!(matches!(err, CryptoShareError::Locked));
    }

    #[test]
    fn crypto_share_folder_empty_temppass_rejected() {
        let runtime = dev_runtime();
        let shell = started_shell("master");
        let err = runtime
            .crypto_share_folder(
                token(),
                &shell,
                SecretString::new(""),
                7,
                "name".into(),
                "a@b.com".into(),
                "hi".into(),
                SharePermissions::from_bits(3),
                None,
            )
            .unwrap_err();
        assert!(matches!(err, CryptoShareError::EmptyTemppass));
    }

    #[test]
    fn crypto_share_folder_happy_path_derives_and_forwards() {
        let runtime = dev_runtime();
        let shell = started_shell("master");
        let outcome = runtime
            .crypto_share_folder(
                token(),
                &shell,
                SecretString::new("temp"),
                7,
                "name".into(),
                "a@b.com".into(),
                "hi".into(),
                SharePermissions::from_bits(3),
                Some("hint".into()),
            )
            .unwrap();
        assert_eq!(outcome.share_request_id, Some(777));
    }

    #[test]
    fn crypto_account_team_share_happy_path() {
        let runtime = dev_runtime();
        let shell = started_shell("master");
        let outcome = runtime
            .crypto_account_team_share(
                token(),
                &shell,
                SecretString::new("temp"),
                7,
                "team-crypto".into(),
                9,
                "msg".into(),
                SharePermissions::from_bits(27),
                Some("hint".into()),
            )
            .unwrap();
        assert_eq!(outcome.share_request_id, Some(888));
    }

    #[test]
    fn crypto_account_team_share_rejects_locked_crypto() {
        let runtime = dev_runtime();
        let shell = pcloud_crypto::CryptoShell::default();
        let err = runtime
            .crypto_account_team_share(
                token(),
                &shell,
                SecretString::new("temp"),
                7,
                "t".into(),
                9,
                "m".into(),
                SharePermissions::from_bits(3),
                None,
            )
            .unwrap_err();
        assert!(matches!(err, CryptoShareError::Locked));
    }

    #[test]
    fn dev_mutations_succeed() {
        let runtime = dev_runtime();
        runtime.cancel_share_request(token(), 101).unwrap();
        runtime.decline_share_request(token(), 101).unwrap();
        runtime.accept_share_request(token(), 101, 7, None).unwrap();
        runtime.remove_share(token(), 55).unwrap();
        runtime
            .modify_share(token(), 55, SharePermissions::from_bits(15))
            .unwrap();
        runtime
            .account_stop_share(token(), vec![55], vec![])
            .unwrap();
        runtime
            .account_modify_share(token(), vec![(55, SharePermissions::from_bits(7))], vec![])
            .unwrap();
    }
}

/// Test-only mock fixture for the `shares_backend` subsystem.
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
    pub const REPRESENTATIVE_COMMAND: &str = "listshares";

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

        /// Record the representative shares runtime call (listshares).
        ///
        /// Returns the recorded event so integration tests can assert
        /// on the exact command name without re-reading the recorder.
        pub fn record_representative_call(&self) -> MockEvent {
            self.fixture.proto.call(REPRESENTATIVE_COMMAND, "mock");
            MockEvent::with_payload("proto", REPRESENTATIVE_COMMAND, "mock")
        }
    }
}
