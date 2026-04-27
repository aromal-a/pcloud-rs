//! Binary protocol DTOs for the crypto-password-change family.
//!
//! Mirrors the C `crypto_changeuserprivate` and `crypto_sendchangeuserprivate`
//! commands from `pclsync/pcryptofolder.c` / `pclsync/psynclib.c`.
//!
//! ## Secret handling
//!
//! These DTOs carry only the re-encoded *ciphertext* private key material and
//! its signature — neither the old nor the new passphrase is sent on the wire
//! in any of these requests (the passphrase is used client-side only, to
//! re-encode the private key). No secret fields therefore live on these
//! structs. The `auth` field is an authenticated session token, never a
//! password; it follows the same transit-only lifetime as every other
//! `auth` field in this crate (see the audit H1 note in `account.rs`).
//!
//! The recovery `code` and user-supplied `hint` are opaque strings delivered
//! from the email-link flow (see `SendChangeUserPrivateRequest`) and a
//! user-visible label respectively — they are never secrets by themselves,
//! but the Rust path still passes the `code` through short-lived locals so
//! that it is not retained on long-lived state.

// **PLATFORM:** all
// **GATING:** none (portable).

use crate::binary_api::{BinaryParam, BinaryParamValue};
use crate::methods::ProtocolMethod;
use crate::redacted::RedactedProtoString;
use crate::response::{ResponseParseError, Value, parse_response_frame};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;

/// `crypto_changeuserprivate` — upload a private key that has been
/// re-encoded with a new passphrase. The server replaces the currently
/// stored encrypted private key for the account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeUserPrivateRequest {
    /// Authenticated session token.
    pub auth_token: RedactedProtoString,
    /// Re-encoded private key (ciphertext blob, hex / pcloud-custom
    /// serialization — opaque at this layer).
    pub private_key: String,
    /// Signature over the re-encoded key.
    pub signature: String,
    /// User-supplied non-secret password hint.
    pub hint: String,
    /// Confirmation code from `crypto_sendchangeuserprivate`.
    pub code: String,
}

impl ChangeUserPrivateRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "crypto_changeuserprivate"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        vec![
            BinaryParam {
                name: "auth".to_owned(),
                value: BinaryParamValue::String(self.auth_token.expose_secret().to_owned()),
            },
            BinaryParam {
                name: "privatekey".to_owned(),
                value: BinaryParamValue::String(self.private_key.clone()),
            },
            BinaryParam {
                name: "signature".to_owned(),
                value: BinaryParamValue::String(self.signature.clone()),
            },
            BinaryParam {
                name: "hint".to_owned(),
                value: BinaryParamValue::String(self.hint.clone()),
            },
            BinaryParam {
                name: "code".to_owned(),
                value: BinaryParamValue::String(self.code.clone()),
            },
        ]
    }
}

impl ProtocolMethod for ChangeUserPrivateRequest {
    fn command_name(&self) -> &'static str {
        Self::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        Self::params(self)
    }
}

/// `crypto_sendchangeuserprivate` — ask the server to email a confirmation
/// code required to complete the subsequent `crypto_changeuserprivate` call.
/// Takes only an auth token; returns `result=0` on success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendChangeUserPrivateRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
}

impl SendChangeUserPrivateRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "crypto_sendchangeuserprivate"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        vec![BinaryParam {
            name: "auth".to_owned(),
            value: BinaryParamValue::String(self.auth_token.expose_secret().to_owned()),
        }]
    }
}

impl ProtocolMethod for SendChangeUserPrivateRequest {
    fn command_name(&self) -> &'static str {
        Self::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        Self::params(self)
    }
}

// ---------------------------------------------------------------------------
// PclsyncCompat crypto setup + folder/file key retrieval
// ---------------------------------------------------------------------------
//
// These mirror the pclsync-compatible (pcloudcom-interop) wire surface
// used by the retained PclsyncCompat crypto backend. Field names on the
// wire are confirmed against `C_CODE/pclsync/pcryptofolder.c`:
//
// - `crypto_setuserkeys` — `pcryptofolder.c:148-197`:
//     PAPI_LSTR("privatekey", ...)     -> line 155
//     PAPI_LSTR("publickey",  ...)     -> line 156
//     PAPI_STR ("hint",       ...)     -> line 157
//   (plus PAPI_STR("timeformat","timestamp") and PAPI_STR("auth", ...))
//   Command string literal at line 168.
// - `crypto_getfolderkey` — `pcryptofolder.c:808-860`:
//     request:  PAPI_NUM("folderid", ...) at line 810
//     response: papi_find_result2(res, "key", PARAM_STR) at line 848,
//               "result" PARAM_NUM at line 837.
// - `crypto_getfilekey` — `pcryptofolder.c:862-914`:
//     request:  PAPI_NUM("fileid", ...) at line 863
//     response: "key" PARAM_STR at line 901, plus "hash" PARAM_NUM at
//               line 900 (file-version hash — exposed on the response
//               struct below).
//
// Design decision (see CryptoSetupV2 IPC note): there is intentionally
// no separate `crypto_changeuserkeys` — password rotation reuses
// `crypto_setuserkeys` with server-side overwrite semantics.

/// `crypto_setuserkeys` — upload the pclsync-compatible priv_key_ver1 /
/// pub_key_ver1 keypair. Mirrors C `setup_do_upload` at
/// `pcryptofolder.c:148` (command issued at `pcryptofolder.c:168`).
///
/// Used for both initial crypto setup and password rotation: the C
/// client calls the same endpoint for both paths and relies on
/// server-side overwrite semantics. The daemon-side dispatcher is free
/// to call this on a freshly-setup shell or on a post-password-change
/// shell. See `crates/pcloud-ipc/src/methods.rs` (`Request::CryptoSetupV2`
/// doc comment) for the IPC-level rationale.
///
/// Payload fields carry **base64-encoded** blobs of the already-sealed
/// `priv_key_ver1` / `pub_key_ver1` structures. The C client transmits
/// the raw bytes via `PAPI_LSTR`, but the Rust wire surface uses a
/// base64 envelope so that the daemon-side dispatcher can keep the
/// material as an ASCII-clean `String` all the way through the typed
/// request pipeline without introducing a non-UTF-8 byte path into
/// `BinaryParamValue::String`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PclsyncSetUserKeysRequest {
    /// Authenticated session token.
    pub auth_token: RedactedProtoString,
    /// Base64 (RFC 4648, `+/`, `=` padding) of the sealed
    /// `priv_key_ver1` blob. Matches the C field `"privatekey"`
    /// (`pcryptofolder.c:155`).
    pub priv_key_ver1_b64: String,
    /// Base64 (RFC 4648) of the sealed `pub_key_ver1` blob. Matches
    /// the C field `"publickey"` (`pcryptofolder.c:156`).
    pub pub_key_ver1_b64: String,
    /// Optional user-supplied non-secret password hint. Matches the C
    /// field `"hint"` (`pcryptofolder.c:157`). Omitted from the wire
    /// when `None`.
    pub hint: Option<String>,
}

impl PclsyncSetUserKeysRequest {
    /// `command_name` — command name.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "crypto_setuserkeys"
    }

    /// `params` — typed parameter vector.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut out = Vec::with_capacity(5);
        out.push(BinaryParam {
            name: "auth".to_owned(),
            value: BinaryParamValue::String(self.auth_token.expose_secret().to_owned()),
        });
        out.push(BinaryParam {
            name: "privatekey".to_owned(),
            value: BinaryParamValue::String(self.priv_key_ver1_b64.clone()),
        });
        out.push(BinaryParam {
            name: "publickey".to_owned(),
            value: BinaryParamValue::String(self.pub_key_ver1_b64.clone()),
        });
        if let Some(hint) = &self.hint {
            out.push(BinaryParam {
                name: "hint".to_owned(),
                value: BinaryParamValue::String(hint.clone()),
            });
        }
        // Mirrors `PAPI_STR("timeformat", "timestamp")` at
        // `pcryptofolder.c:157` so that the server returns the
        // `cryptoexpires` field as a unix timestamp, not an ISO string.
        out.push(BinaryParam {
            name: "timeformat".to_owned(),
            value: BinaryParamValue::String("timestamp".to_owned()),
        });
        out
    }
}

impl ProtocolMethod for PclsyncSetUserKeysRequest {
    fn command_name(&self) -> &'static str {
        Self::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        Self::params(self)
    }
}

/// Typed `crypto_setuserkeys` response. Mirrors the C decoder at
/// `pcryptofolder.c:178-195`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PclsyncSetUserKeysResponse {
    /// Server result code. `0` on success; non-zero values map to the
    /// `PSYNC_CRYPTO_SETUP_*` taxonomy in `pcryptofolder.c:187-194`.
    pub result: u32,
    /// Human-readable error description when `result != 0`.
    pub error: Option<String>,
    /// Server-assigned expiration timestamp for the uploaded keypair
    /// (unix seconds). Only present on success; C mirrors this at
    /// `pcryptofolder.c:180`.
    pub cryptoexpires: Option<u64>,
}

impl PclsyncSetUserKeysResponse {
    /// Decode a full response frame (little-endian `u32` length prefix
    /// stripped by the transport; this function takes the frame body
    /// *including* its length prefix, as returned by the resilient
    /// transport).
    ///
    /// # Errors
    ///
    /// Returns [`ResponseParseError`] when the binary frame is
    /// malformed or violates the default parse limits.
    pub fn decode(bytes: &[u8]) -> Result<Self, ResponseParseError> {
        let value = parse_response_frame(bytes, &Default::default())?;
        Self::from_value(&value)
    }

    fn from_value(value: &Value) -> Result<Self, ResponseParseError> {
        let hash = value.as_hash().ok_or(ResponseParseError::UnexpectedEof)?;
        let result = hash
            .get_number("result")
            .ok_or(ResponseParseError::UnexpectedEof)?;
        let result_u32 = u32::try_from(result).unwrap_or(u32::MAX);
        let error = hash.get_string("error").map(str::to_owned);
        let cryptoexpires = hash.get_number("cryptoexpires");
        Ok(Self {
            result: result_u32,
            error,
            cryptoexpires,
        })
    }
}

/// `crypto_getfolderkey` — fetch a folder's RSA-OAEP-wrapped
/// `sym_key_ver1`. Mirrors C `download_fldr_enckey` at
/// `pcryptofolder.c:808` (command issued at line 826).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CryptoGetFolderKeyRequest {
    /// Authenticated session token.
    pub auth_token: RedactedProtoString,
    /// Remote crypto folder id. Matches the C field `"folderid"`
    /// (`pcryptofolder.c:810`).
    pub folder_id: u64,
}

impl CryptoGetFolderKeyRequest {
    /// `command_name` — command name.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "crypto_getfolderkey"
    }

    /// `params` — typed parameter vector.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        vec![
            BinaryParam {
                name: "auth".to_owned(),
                value: BinaryParamValue::String(self.auth_token.expose_secret().to_owned()),
            },
            BinaryParam {
                name: "folderid".to_owned(),
                value: BinaryParamValue::Number(self.folder_id),
            },
        ]
    }
}

impl ProtocolMethod for CryptoGetFolderKeyRequest {
    fn command_name(&self) -> &'static str {
        Self::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        Self::params(self)
    }
}

/// Typed `crypto_getfolderkey` response. The wrapped key is delivered
/// as a base64 string under the `"key"` field (`pcryptofolder.c:848`)
/// and decoded into raw bytes here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CryptoGetFolderKeyResponse {
    /// Server result code. `0` on success.
    pub result: u32,
    /// Human-readable error description when `result != 0`.
    pub error: Option<String>,
    /// The RSA-OAEP-wrapped `sym_key_ver1` bytes. Base64 on the wire;
    /// already decoded here.
    pub wrapped_key: Vec<u8>,
    // TODO(bd-1du.10): additional metadata fields (owner id,
    // modification timestamp) are not surfaced by the C decoder at
    // `pcryptofolder.c:848-859` — if the live server response carries
    // them, extend this struct before the final parity sign-off.
}

impl CryptoGetFolderKeyResponse {
    /// Decode a full response frame.
    ///
    /// # Errors
    ///
    /// Returns [`ResponseParseError`] on a malformed frame or on a
    /// `"key"` field that is not valid base64.
    pub fn decode(bytes: &[u8]) -> Result<Self, ResponseParseError> {
        let value = parse_response_frame(bytes, &Default::default())?;
        Self::from_value(&value)
    }

    fn from_value(value: &Value) -> Result<Self, ResponseParseError> {
        let hash = value.as_hash().ok_or(ResponseParseError::UnexpectedEof)?;
        let result = hash
            .get_number("result")
            .ok_or(ResponseParseError::UnexpectedEof)?;
        let result_u32 = u32::try_from(result).unwrap_or(u32::MAX);
        let error = hash.get_string("error").map(str::to_owned);
        let wrapped_key = if result_u32 == 0 {
            let key_b64 = hash
                .get_string("key")
                .ok_or(ResponseParseError::UnexpectedEof)?;
            B64.decode(key_b64)
                .map_err(|_| ResponseParseError::UnexpectedEof)?
        } else {
            Vec::new()
        };
        Ok(Self {
            result: result_u32,
            error,
            wrapped_key,
        })
    }
}

// ---------------------------------------------------------------------------
// crypto_getpubkey — fetch a user's / team's raw `pub_key_ver1` blob for
// share-invitation RSA-OAEP wrapping.
// ---------------------------------------------------------------------------
//
// C reference: the pcloudcc client calls this via `crypto_getpubkey`
// (addressed by `userid` or `mail`), consumed by
// `psync_crypto_share_folder` / `psync_crypto_account_teamshare`
// (`pclsync/psynclib.c:1322` / `:1372`). The server returns a hex-encoded
// (`publickey`) or base64-encoded blob of the recipient's RSA-4096
// `pub_key_ver1` structure. This Rust encoder accepts the hex form
// (matching the C decoder at `pssl.c:583..`); the API layer will decode
// either hex or base64 transparently.

/// `crypto_getpubkey` — fetch a recipient's RSA-4096 `pub_key_ver1` blob.
///
/// One of [`userid`] or [`mail`] must be set; the server looks up the
/// recipient by whichever is provided. For team-share (account_teamshare)
/// flows, the C client reuses this same endpoint with `teamid` — the
/// enum `Recipient::Team(teamid)` variant wires that form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CryptoGetPubKeyRequest {
    /// Authenticated session token.
    pub auth_token: RedactedProtoString,
    /// Recipient selector.
    pub recipient: CryptoPubKeyRecipient,
}

/// Recipient selector for [`CryptoGetPubKeyRequest`]. Mirrors the C
/// client's mutually-exclusive `userid` / `mail` / `teamid` parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoPubKeyRecipient {
    /// Look up by numeric user id.
    UserId(u64),
    /// Look up by email address.
    Mail(String),
    /// Look up by numeric team id (for account_teamshare flows).
    TeamId(u64),
}

impl CryptoGetPubKeyRequest {
    /// `command_name` — command name.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "crypto_getpubkey"
    }

    /// `params` — typed parameter vector.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut out = Vec::with_capacity(2);
        out.push(BinaryParam {
            name: "auth".to_owned(),
            value: BinaryParamValue::String(self.auth_token.expose_secret().to_owned()),
        });
        match &self.recipient {
            CryptoPubKeyRecipient::UserId(id) => out.push(BinaryParam {
                name: "userid".to_owned(),
                value: BinaryParamValue::Number(*id),
            }),
            CryptoPubKeyRecipient::Mail(mail) => out.push(BinaryParam {
                name: "mail".to_owned(),
                value: BinaryParamValue::String(mail.clone()),
            }),
            CryptoPubKeyRecipient::TeamId(id) => out.push(BinaryParam {
                name: "teamid".to_owned(),
                value: BinaryParamValue::Number(*id),
            }),
        }
        out
    }
}

impl ProtocolMethod for CryptoGetPubKeyRequest {
    fn command_name(&self) -> &'static str {
        Self::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        Self::params(self)
    }
}

/// `crypto_getfilekey` — fetch a file's RSA-OAEP-wrapped
/// `sym_key_ver1`. Mirrors C `download_file_enckey` at
/// `pcryptofolder.c:862` (command issued at line 879).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CryptoGetFileKeyRequest {
    /// Authenticated session token.
    pub auth_token: RedactedProtoString,
    /// Remote crypto file id. Matches the C field `"fileid"`
    /// (`pcryptofolder.c:863`).
    pub file_id: u64,
}

impl CryptoGetFileKeyRequest {
    /// `command_name` — command name.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "crypto_getfilekey"
    }

    /// `params` — typed parameter vector.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        vec![
            BinaryParam {
                name: "auth".to_owned(),
                value: BinaryParamValue::String(self.auth_token.expose_secret().to_owned()),
            },
            BinaryParam {
                name: "fileid".to_owned(),
                value: BinaryParamValue::Number(self.file_id),
            },
        ]
    }
}

impl ProtocolMethod for CryptoGetFileKeyRequest {
    fn command_name(&self) -> &'static str {
        Self::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        Self::params(self)
    }
}

/// Typed `crypto_getfilekey` response. C decoder at
/// `pcryptofolder.c:890-909`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CryptoGetFileKeyResponse {
    /// Server result code. `0` on success.
    pub result: u32,
    /// Human-readable error description when `result != 0`.
    pub error: Option<String>,
    /// The RSA-OAEP-wrapped `sym_key_ver1` bytes. Base64 on the wire;
    /// already decoded here.
    pub wrapped_key: Vec<u8>,
    /// File-version hash associated with the wrapped key. The C decoder
    /// reads this at `pcryptofolder.c:900`; the daemon must carry it
    /// alongside the unwrapped sym-key so that subsequent seal/open
    /// calls can verify they target the same file version.
    pub hash: Option<u64>,
    // TODO(bd-1du.10): owner id / timestamp metadata not present in
    // the C decoder; extend if live responses show additional fields.
}

impl CryptoGetFileKeyResponse {
    /// Decode a full response frame.
    ///
    /// # Errors
    ///
    /// Returns [`ResponseParseError`] on a malformed frame or on a
    /// `"key"` field that is not valid base64.
    pub fn decode(bytes: &[u8]) -> Result<Self, ResponseParseError> {
        let value = parse_response_frame(bytes, &Default::default())?;
        Self::from_value(&value)
    }

    fn from_value(value: &Value) -> Result<Self, ResponseParseError> {
        let hash = value.as_hash().ok_or(ResponseParseError::UnexpectedEof)?;
        let result = hash
            .get_number("result")
            .ok_or(ResponseParseError::UnexpectedEof)?;
        let result_u32 = u32::try_from(result).unwrap_or(u32::MAX);
        let error = hash.get_string("error").map(str::to_owned);
        let (wrapped_key, file_hash) = if result_u32 == 0 {
            let key_b64 = hash
                .get_string("key")
                .ok_or(ResponseParseError::UnexpectedEof)?;
            let bytes = B64
                .decode(key_b64)
                .map_err(|_| ResponseParseError::UnexpectedEof)?;
            (bytes, hash.get_number("hash"))
        } else {
            (Vec::new(), None)
        };
        Ok(Self {
            result: result_u32,
            error,
            wrapped_key,
            hash: file_hash,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::methods::ProtocolMethod;

    #[test]
    fn change_user_private_encodes_with_five_params() {
        let req = ChangeUserPrivateRequest {
            auth_token: "token".into(),
            private_key: "ZW5jcnlwdGVkX3ByaXZhdGVfa2V5".to_owned(),
            signature: "c2lnbmF0dXJl".to_owned(),
            hint: "memory of first pet".to_owned(),
            code: "AB12-CD34".to_owned(),
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.command, "crypto_changeuserprivate");
        assert_eq!(encoded.frame.parameter_count, 5);

        // Verify ordering by param names in the serialized struct.
        let params = req.params();
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["auth", "privatekey", "signature", "hint", "code"]
        );
    }

    #[test]
    fn send_change_user_private_encodes_with_one_param() {
        let req = SendChangeUserPrivateRequest {
            auth_token: "token".into(),
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.command, "crypto_sendchangeuserprivate");
        assert_eq!(encoded.frame.parameter_count, 1);
    }

    #[test]
    fn change_user_private_preserves_payload_bytes() {
        // Ensure that the private_key blob is passed through byte-for-byte,
        // i.e. no accidental utf-8 normalization trimmed anything.
        let payload = "ABCDEFG_12345_../=".to_owned();
        let req = ChangeUserPrivateRequest {
            auth_token: "t".into(),
            private_key: payload.clone(),
            signature: "sig".into(),
            hint: "".into(),
            code: "c".into(),
        };
        let found = req
            .params()
            .into_iter()
            .find(|p| p.name == "privatekey")
            .and_then(|p| match p.value {
                BinaryParamValue::String(s) => Some(s),
                _ => None,
            })
            .expect("privatekey present");
        assert_eq!(found, payload);
    }

    // -----------------------------------------------------------------
    // PclsyncCompat crypto setup + folder/file key tests (Stage 4b.2)
    // -----------------------------------------------------------------

    fn find_string(params: &[BinaryParam], name: &str) -> Option<String> {
        params
            .iter()
            .find(|p| p.name == name)
            .and_then(|p| match &p.value {
                BinaryParamValue::String(s) => Some(s.clone()),
                _ => None,
            })
    }

    fn find_number(params: &[BinaryParam], name: &str) -> Option<u64> {
        params
            .iter()
            .find(|p| p.name == name)
            .and_then(|p| match &p.value {
                BinaryParamValue::Number(n) => Some(*n),
                _ => None,
            })
    }

    #[test]
    fn pclsync_set_user_keys_encode_has_privatekey_and_publickey() {
        let req = PclsyncSetUserKeysRequest {
            auth_token: "tok".into(),
            priv_key_ver1_b64: "cHJpdg==".to_owned(), // base64 "priv"
            pub_key_ver1_b64: "cHViYmxpYw==".to_owned(), // base64 "pubblic"
            hint: Some("remember-me".to_owned()),
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.command, "crypto_setuserkeys");
        // auth + privatekey + publickey + hint + timeformat
        assert_eq!(encoded.frame.parameter_count, 5);

        let params = req.params();
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"privatekey"));
        assert!(names.contains(&"publickey"));
        assert!(names.contains(&"hint"));
        assert!(names.contains(&"timeformat"));
        assert!(names.contains(&"auth"));

        assert_eq!(
            find_string(&params, "privatekey").as_deref(),
            Some("cHJpdg==")
        );
        assert_eq!(
            find_string(&params, "publickey").as_deref(),
            Some("cHViYmxpYw==")
        );
        assert_eq!(find_string(&params, "hint").as_deref(), Some("remember-me"));
        assert_eq!(
            find_string(&params, "timeformat").as_deref(),
            Some("timestamp")
        );

        // Sanity-check that the claimed base64 values decode cleanly,
        // so an accidental field mix-up would surface here.
        assert_eq!(B64.decode("cHJpdg==").unwrap(), b"priv");
        assert_eq!(B64.decode("cHViYmxpYw==").unwrap(), b"pubblic");
    }

    #[test]
    fn pclsync_set_user_keys_encode_omits_hint_when_none() {
        let req = PclsyncSetUserKeysRequest {
            auth_token: "tok".into(),
            priv_key_ver1_b64: "AA==".to_owned(),
            pub_key_ver1_b64: "AA==".to_owned(),
            hint: None,
        };
        let params = req.params();
        assert!(params.iter().all(|p| p.name != "hint"));
        // auth + privatekey + publickey + timeformat
        assert_eq!(params.len(), 4);
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.parameter_count, 4);
    }

    #[test]
    fn get_folder_key_encode_folder_id() {
        let req = CryptoGetFolderKeyRequest {
            auth_token: "tok".into(),
            folder_id: 424_242,
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.command, "crypto_getfolderkey");
        assert_eq!(encoded.frame.parameter_count, 2);
        let params = req.params();
        assert_eq!(find_number(&params, "folderid"), Some(424_242));
        assert_eq!(find_string(&params, "auth").as_deref(), Some("tok"));
        // Must NOT also carry a fileid slot.
        assert!(find_number(&params, "fileid").is_none());
    }

    #[test]
    fn get_file_key_encode_file_id() {
        let req = CryptoGetFileKeyRequest {
            auth_token: "tok".into(),
            file_id: 7_777_777,
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.command, "crypto_getfilekey");
        assert_eq!(encoded.frame.parameter_count, 2);
        let params = req.params();
        assert_eq!(find_number(&params, "fileid"), Some(7_777_777));
        assert_eq!(find_string(&params, "auth").as_deref(), Some("tok"));
        assert!(find_number(&params, "folderid").is_none());
    }

    #[test]
    fn decode_set_user_keys_response_result_error_shape() {
        // Success shape.
        let ok = Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            ("cryptoexpires".to_owned(), Value::Number(1_717_000_000)),
        ]);
        let parsed = PclsyncSetUserKeysResponse::from_value(&ok).expect("ok decode");
        assert_eq!(parsed.result, 0);
        assert_eq!(parsed.error, None);
        assert_eq!(parsed.cryptoexpires, Some(1_717_000_000));

        // Error shape — result=2110 (ALREADY_SETUP per pcryptofolder.c:193).
        let err = Value::Hash(vec![
            ("result".to_owned(), Value::Number(2110)),
            (
                "error".to_owned(),
                Value::String("already setup".to_owned()),
            ),
        ]);
        let parsed = PclsyncSetUserKeysResponse::from_value(&err).expect("err decode");
        assert_eq!(parsed.result, 2110);
        assert_eq!(parsed.error.as_deref(), Some("already setup"));
        assert_eq!(parsed.cryptoexpires, None);
    }

    #[test]
    fn decode_folder_key_response_base64_decodes_wrapped_key() {
        let wrapped = b"\x01\x02\x03\x04raw-folder-key";
        let key_b64 = B64.encode(wrapped);
        let ok = Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            ("key".to_owned(), Value::String(key_b64)),
        ]);
        let parsed = CryptoGetFolderKeyResponse::from_value(&ok).expect("ok decode");
        assert_eq!(parsed.result, 0);
        assert_eq!(parsed.error, None);
        assert_eq!(parsed.wrapped_key, wrapped);

        // Error shape — no "key" field required.
        let err = Value::Hash(vec![
            ("result".to_owned(), Value::Number(2009)),
            ("error".to_owned(), Value::String("no crypto".to_owned())),
        ]);
        let parsed = CryptoGetFolderKeyResponse::from_value(&err).expect("err decode");
        assert_eq!(parsed.result, 2009);
        assert_eq!(parsed.error.as_deref(), Some("no crypto"));
        assert!(parsed.wrapped_key.is_empty());
    }

    #[test]
    fn get_pub_key_encode_userid() {
        let req = CryptoGetPubKeyRequest {
            auth_token: "tok".into(),
            recipient: CryptoPubKeyRecipient::UserId(1234),
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.command, "crypto_getpubkey");
        assert_eq!(encoded.frame.parameter_count, 2);
        let params = req.params();
        assert_eq!(find_number(&params, "userid"), Some(1234));
        assert!(find_string(&params, "mail").is_none());
        assert!(find_number(&params, "teamid").is_none());
    }

    #[test]
    fn get_pub_key_encode_mail() {
        let req = CryptoGetPubKeyRequest {
            auth_token: "tok".into(),
            recipient: CryptoPubKeyRecipient::Mail("alice@example.com".into()),
        };
        let encoded = req.encode().expect("encode");
        assert_eq!(encoded.frame.command, "crypto_getpubkey");
        assert_eq!(encoded.frame.parameter_count, 2);
        let params = req.params();
        assert_eq!(
            find_string(&params, "mail").as_deref(),
            Some("alice@example.com")
        );
        assert!(find_number(&params, "userid").is_none());
    }

    #[test]
    fn get_pub_key_encode_teamid() {
        let req = CryptoGetPubKeyRequest {
            auth_token: "tok".into(),
            recipient: CryptoPubKeyRecipient::TeamId(9),
        };
        let params = req.params();
        assert_eq!(find_number(&params, "teamid"), Some(9));
        assert!(find_number(&params, "userid").is_none());
        assert!(find_string(&params, "mail").is_none());
    }

    #[test]
    fn decode_file_key_response_extracts_hash_and_wrapped_key() {
        let wrapped = b"raw-file-key-bytes";
        let key_b64 = B64.encode(wrapped);
        let ok = Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            ("hash".to_owned(), Value::Number(0xdead_beef)),
            ("key".to_owned(), Value::String(key_b64)),
        ]);
        let parsed = CryptoGetFileKeyResponse::from_value(&ok).expect("ok decode");
        assert_eq!(parsed.result, 0);
        assert_eq!(parsed.wrapped_key, wrapped);
        assert_eq!(parsed.hash, Some(0xdead_beef));
    }
}
