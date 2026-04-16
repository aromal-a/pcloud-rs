//! Wire-level method builders for the account family (verify/change
//! password, register, promo, api-servers, language). Consumed by
//! `account_api`.

use crate::binary_api::{BinaryParam, BinaryParamValue};
use crate::methods::ProtocolMethod;

/// `GetPromoRequest` — get promo request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetPromoRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: String,
    /// The `os_id` field (os id).
    pub os_id: u64,
}

impl GetPromoRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "getpromourl"
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
                name: "os".to_owned(),
                value: BinaryParamValue::Number(self.os_id),
            },
        ]
    }
}

impl ProtocolMethod for GetPromoRequest {
    fn command_name(&self) -> &'static str {
        GetPromoRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        GetPromoRequest::params(self)
    }
}

/// `SetLanguageRequest` — set language request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetLanguageRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: String,
    /// The `language` field (language).
    pub language: String,
}

impl SetLanguageRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "setlanguage"
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
                name: "language".to_owned(),
                value: BinaryParamValue::String(self.language.clone()),
            },
        ]
    }
}

impl ProtocolMethod for SetLanguageRequest {
    fn command_name(&self) -> &'static str {
        SetLanguageRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        SetLanguageRequest::params(self)
    }
}

/// `GetLocationApiRequest` — get location api request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetLocationApiRequest;

impl GetLocationApiRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "getlocationapi"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        vec![BinaryParam {
            name: "timeformat".to_owned(),
            value: BinaryParamValue::String("timestamp".to_owned()),
        }]
    }
}

impl ProtocolMethod for GetLocationApiRequest {
    fn command_name(&self) -> &'static str {
        GetLocationApiRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        GetLocationApiRequest::params(self)
    }
}

/// `VerifyEmailRequest` — verify email request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyEmailRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: Option<String>,
    /// The `verify_token` field (verify token).
    pub verify_token: Option<String>,
}

impl VerifyEmailRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "sendverificationemail"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::new();
        if let Some(auth_token) = &self.auth_token {
            params.push(BinaryParam {
                name: "auth".to_owned(),
                value: BinaryParamValue::String(auth_token.clone()),
            });
        }
        if let Some(verify_token) = &self.verify_token {
            params.push(BinaryParam {
                name: "verifytoken".to_owned(),
                value: BinaryParamValue::String(verify_token.clone()),
            });
        }
        params
    }
}

impl ProtocolMethod for VerifyEmailRequest {
    fn command_name(&self) -> &'static str {
        VerifyEmailRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        VerifyEmailRequest::params(self)
    }
}

/// `LostPasswordRequest` — lost password request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LostPasswordRequest {
    /// The `email` field (email).
    pub email: String,
}

impl LostPasswordRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "lostpassword"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        vec![BinaryParam {
            name: "mail".to_owned(),
            value: BinaryParamValue::String(self.email.clone()),
        }]
    }
}

impl ProtocolMethod for LostPasswordRequest {
    fn command_name(&self) -> &'static str {
        LostPasswordRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        LostPasswordRequest::params(self)
    }
}

/// NOTE (audit H1): `auth_token`, `current_password`, and `new_password`
/// are stored as plain `String` because this DTO is a send-and-forget
/// request builder: it is constructed by `account_api.rs`, immediately
/// serialized via `params()` into a `BinaryParam` vector for the wire,
/// and dropped in the same function scope. The secrets never outlive a
/// single HTTPS transaction and are never persisted, logged, or surfaced
/// via `Debug` on a long-lived struct. If this struct ever starts being
/// stored on runtime state, these fields must be converted to
/// `SecretString` and `params()` updated to call `.expose_secret()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangePasswordRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: String,
    /// The `current_password` field (current password).
    pub current_password: String,
    /// The `new_password` field (new password).
    pub new_password: String,
    /// The `device` field (device).
    pub device: String,
}

impl ChangePasswordRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "changepassword"
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
                name: "oldpassword".to_owned(),
                value: BinaryParamValue::String(self.current_password.clone()),
            },
            BinaryParam {
                name: "newpassword".to_owned(),
                value: BinaryParamValue::String(self.new_password.clone()),
            },
            BinaryParam {
                name: "device".to_owned(),
                value: BinaryParamValue::String(self.device.clone()),
            },
            BinaryParam {
                name: "regetauth".to_owned(),
                value: BinaryParamValue::Bool(true),
            },
        ]
    }
}

impl ProtocolMethod for ChangePasswordRequest {
    fn command_name(&self) -> &'static str {
        ChangePasswordRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        ChangePasswordRequest::params(self)
    }
}

/// `RegisterRequest` — register request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterRequest {
    /// The `email` field (email).
    pub email: String,
    /// The `password` field (password).
    pub password: String,
    /// The `terms_accepted` field (terms accepted).
    pub terms_accepted: bool,
    /// The `os_id` field (os id).
    pub os_id: u64,
}

impl RegisterRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "register"
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
                name: "mail".to_owned(),
                value: BinaryParamValue::String(self.email.clone()),
            },
            BinaryParam {
                name: "password".to_owned(),
                value: BinaryParamValue::String(self.password.clone()),
            },
            BinaryParam {
                name: "termsaccepted".to_owned(),
                value: BinaryParamValue::String(if self.terms_accepted {
                    "yes".to_owned()
                } else {
                    "0".to_owned()
                }),
            },
            BinaryParam {
                name: "os".to_owned(),
                value: BinaryParamValue::Number(self.os_id),
            },
        ]
    }
}

impl ProtocolMethod for RegisterRequest {
    fn command_name(&self) -> &'static str {
        RegisterRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        RegisterRequest::params(self)
    }
}
