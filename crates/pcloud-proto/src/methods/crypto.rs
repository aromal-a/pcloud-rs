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

/// `crypto_changeuserprivate` — upload a private key that has been
/// re-encoded with a new passphrase. The server replaces the currently
/// stored encrypted private key for the account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeUserPrivateRequest {
    /// Authenticated session token.
    pub auth_token: String,
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
                value: BinaryParamValue::String(self.auth_token.clone()),
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
    pub auth_token: String,
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
            value: BinaryParamValue::String(self.auth_token.clone()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::methods::ProtocolMethod;

    #[test]
    fn change_user_private_encodes_with_five_params() {
        let req = ChangeUserPrivateRequest {
            auth_token: "token".to_owned(),
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
            auth_token: "token".to_owned(),
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
}
