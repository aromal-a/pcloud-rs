//! Daemon-side transport runtime for the crypto password-change family.
//!
//! The actual cryptographic work (deriving the new master key, producing
//! the opaque re-encoded "private key" blob and its HMAC signature) happens
//! inside [`pcloud_crypto::CryptoShell`]; this file only owns the binary
//! API transport for `crypto_changeuserprivate` and
//! `crypto_sendchangeuserprivate`.
//!
//! ### Development transport
//!
//! In `ApiMode::Development` we run against an in-process mock that simply
//! echoes `result=0` for well-formed requests so the daemon integration
//! tests can exercise the change-and-reunlock cycle without reaching a
//! real pCloud backend.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io;

use pcloud_config::{ConfigProfile, api::ApiMode};
use pcloud_proto::{
    BinaryApiTransport, BinaryParamValue, EncodedRequest, ParseLimits, ResponseParseError,
    TransportConfig, TransportError,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    crypto_api::{CryptoApi, CryptoApiError},
    parse_response_frame,
    response::Value,
};
use thiserror::Error;

#[derive(Debug, Clone, Default)]
/// `DevelopmentCryptoTransport` struct.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub struct DevelopmentCryptoTransport;

impl ProtocolTransport for DevelopmentCryptoTransport {
    type Error = io::Error;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        let frame = match request.frame.command.as_str() {
            "crypto_sendchangeuserprivate" => {
                // Require an auth param to be present and non-empty.
                let auth = string_param(request, "auth").unwrap_or("");
                if auth.is_empty() {
                    simple_hash(&[("result", 2000u64), ("error_kind", 1u64)])
                } else {
                    simple_ok()
                }
            }
            "crypto_changeuserprivate" => {
                let auth = string_param(request, "auth").unwrap_or("");
                let pk = string_param(request, "privatekey").unwrap_or("");
                let sig = string_param(request, "signature").unwrap_or("");
                let code = string_param(request, "code").unwrap_or("");
                if auth.is_empty() || pk.is_empty() || sig.is_empty() || code.is_empty() {
                    simple_hash(&[("result", 2000u64), ("error_kind", 2u64)])
                } else {
                    simple_ok()
                }
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("unsupported crypto command: {}", request.frame.command),
                ));
            }
        }?;

        parse_response_frame(&frame, &ParseLimits::default()).map_err(map_response_parse_err)
    }
}

impl ApiServerHintConsumer for DevelopmentCryptoTransport {
    fn apply_api_server_hint(&self, _api_server: &str) {}
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

fn simple_ok() -> io::Result<Vec<u8>> {
    encode_hash_response(&[("result", 0u64)])
}

fn simple_hash(entries: &[(&str, u64)]) -> io::Result<Vec<u8>> {
    encode_hash_response(entries)
}

/// Minimal hash-response encoder matching the pcloud binary response
/// protocol. Duplicates the dev-only shape used by `account_backend`
/// deliberately — the intent is for each dev transport to stay
/// self-contained and not take a dep on a sibling backend's internals.
fn encode_hash_response(entries: &[(&str, u64)]) -> io::Result<Vec<u8>> {
    const RPARAM_NUM8: u8 = 15;
    const RPARAM_HASH: u8 = 16;
    const RPARAM_SMALL_NUM_BASE: u8 = 200;
    const RPARAM_END: u8 = 255;
    const RPARAM_SHORT_STR_BASE: u8 = 100;

    let mut payload = vec![RPARAM_HASH];
    for (key, value) in entries {
        if key.len() > 49 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "dev response encoder only supports short key names",
            ));
        }
        payload.push(RPARAM_SHORT_STR_BASE + key.len() as u8);
        payload.extend_from_slice(key.as_bytes());
        if *value < 20 {
            payload.push(RPARAM_SMALL_NUM_BASE + (*value as u8));
        } else {
            payload.push(RPARAM_NUM8);
            payload.extend_from_slice(&value.to_le_bytes());
        }
    }
    payload.push(RPARAM_END);

    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

#[derive(Debug, Error)]
/// `CryptoBackendError` enum.
///
/// See the module-level docs for how this type participates in the
/// backend's dispatch pipeline and `EncodedValue` wire translation.
pub enum CryptoBackendError {
    #[error(transparent)]
    /// `Development` variant.
    Development(#[from] io::Error),
    #[error(transparent)]
    /// `Network` variant.
    Network(#[from] TransportError),
}

#[derive(Debug, Clone)]
enum CryptoTransportMode {
    Development(DevelopmentCryptoTransport),
    Network(BinaryApiTransport),
}

impl ProtocolTransport for CryptoTransportMode {
    type Error = CryptoBackendError;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        match self {
            Self::Development(t) => t.execute(request).map_err(CryptoBackendError::from),
            Self::Network(t) => t.execute(request).map_err(CryptoBackendError::from),
        }
    }
}

impl ApiServerHintConsumer for CryptoTransportMode {
    fn apply_api_server_hint(&self, api_server: &str) {
        match self {
            Self::Development(t) => t.apply_api_server_hint(api_server),
            Self::Network(t) => t.apply_api_server_hint(api_server),
        }
    }
}

#[derive(Debug)]
/// Entry struct for the crypto backend.
///
/// # Architecture role
///
/// - Dispatches `CryptoSetup`, `CryptoStart`, `CryptoStop`, `CryptoReset`,
///   `CryptoUnlock`, `CryptoLock`, `CryptoCreateFolder`,
///   `CryptoVerifyFingerprint`, and `CryptoChangePass` (partial — see
///   `bd-1du.10`) IPC request frames from `pcloud-daemon::dispatch`.
/// - Issues the pCloud protocol methods `crypto_getuserkeys`,
///   `crypto_setuserkeys`, `crypto_reset`, `crypto_createfolder`. Local
///   unlock/lock is enforced entirely within the runtime and never
///   round-trips to the server. Wire encoding uses the crate-level
///   `EncodedValue` pattern.
/// - Emits audit events for setup, unlock, lock, reset, and fingerprint
///   mismatches. Audit records never include key material.
/// - Persists nothing cleartext: private keys stay in `SecretBytes` for
///   their entire lifetime and are zeroized on drop. Any serialized form
///   that transits the store is sealed with the user-supplied passphrase.
/// - Error taxonomy: see [`CryptoBackendError`].
pub struct CryptoRuntime {
    api: CryptoApi<CryptoTransportMode>,
}

impl CryptoRuntime {
    #[must_use]
    /// Invoke `from_config` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn from_config(config: &ConfigProfile) -> Self {
        let transport = match config.api.mode {
            ApiMode::Development => CryptoTransportMode::Development(DevelopmentCryptoTransport),
            ApiMode::Plaintext | ApiMode::Tls => {
                CryptoTransportMode::Network(BinaryApiTransport::new(TransportConfig::with_tls(
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
            api: CryptoApi::new(transport),
        }
    }

    /// Invoke `send_change_user_private` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn send_change_user_private(
        &self,
        auth_token: &str,
    ) -> Result<(), CryptoApiError<CryptoBackendError>> {
        self.api.send_change_user_private(auth_token)
    }

    /// Invoke `change_user_private` on this backend.
    ///
    /// See the module-level documentation for the dispatch contract, error
    /// translation, and side-effect ordering shared by all entry points.
    pub fn change_user_private(
        &self,
        auth_token: &str,
        private_key: &str,
        signature: &str,
        hint: &str,
        code: &str,
    ) -> Result<(), CryptoApiError<CryptoBackendError>> {
        self.api
            .change_user_private(auth_token, private_key, signature, hint, code)
    }

    /// Upload a PclsyncCompat crypto-setup keypair via
    /// `crypto_setuserkeys`. Stage 4b.3 daemon wiring.
    ///
    /// # Errors
    /// Transport failures and non-zero server result codes surface via
    /// [`CryptoApiError`] in the usual way. Known codes: 1000 (not
    /// logged in), 2000 (can't connect), 2110 (already set up).
    pub fn set_user_keys(
        &self,
        auth_token: &str,
        priv_key_ver1_b64: &str,
        pub_key_ver1_b64: &str,
        hint: Option<&str>,
    ) -> Result<(), CryptoApiError<CryptoBackendError>> {
        self.api
            .set_user_keys(auth_token, priv_key_ver1_b64, pub_key_ver1_b64, hint)
    }

    /// Fetch an RSA-OAEP-wrapped folder sym-key via `crypto_getfolderkey`.
    ///
    /// Returns the wrapped-key bytes; the caller RSA-OAEP-unwraps the
    /// blob against the unlocked private key and caches the plaintext
    /// `SymKeyVer1` via [`pcloud_crypto::CryptoShell::unwrap_and_cache_folder_key`].
    pub fn get_folder_key(
        &self,
        auth_token: &str,
        folder_id: u64,
    ) -> Result<Vec<u8>, CryptoApiError<CryptoBackendError>> {
        self.api.get_folder_key(auth_token, folder_id)
    }

    /// Fetch an RSA-OAEP-wrapped file sym-key via `crypto_getfilekey`.
    ///
    /// Returns `(file_hash, wrapped_key)`. The file hash is the
    /// server-reported file-version hash (`pcryptofolder.c:900`).
    pub fn get_file_key(
        &self,
        auth_token: &str,
        file_id: u64,
    ) -> Result<(u64, Vec<u8>), CryptoApiError<CryptoBackendError>> {
        self.api.get_file_key(auth_token, file_id)
    }

    /// Fetch a recipient's `pub_key_ver1` blob via `crypto_getpubkey`.
    /// CLAUDEREV deferred-set D6 (fire 56). Used by the daemon-side
    /// `crypto_share_folder_rsa` orchestrator to produce the
    /// `recipient_pub_blob` argument that
    /// `SharesRuntime::crypto_share_folder_rsa` needs.
    pub fn get_pub_key(
        &self,
        auth_token: &str,
        recipient: pcloud_proto::methods::crypto::CryptoPubKeyRecipient,
    ) -> Result<Vec<u8>, CryptoApiError<CryptoBackendError>> {
        self.api.get_pub_key(auth_token, recipient)
    }
}

fn map_response_parse_err(err: ResponseParseError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}

/// Test-only mock fixture for the `crypto_backend` subsystem.
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
    pub const REPRESENTATIVE_COMMAND: &str = "crypto_getuserkeys";

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

        /// Record the representative crypto runtime call (crypto_getuserkeys).
        ///
        /// Returns the recorded event so integration tests can assert
        /// on the exact command name without re-reading the recorder.
        pub fn record_representative_call(&self) -> MockEvent {
            self.fixture.proto.call(REPRESENTATIVE_COMMAND, "mock");
            MockEvent::with_payload("proto", REPRESENTATIVE_COMMAND, "mock")
        }
    }
}
