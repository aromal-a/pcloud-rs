//! Wire-level method builders for the account family (verify/change
//! password, register, promo, api-servers, language). Consumed by
//! `account_api`.

use crate::binary_api::{BinaryParam, BinaryParamValue};
use crate::methods::ProtocolMethod;
use crate::redacted::RedactedProtoString;

/// `GetPromoRequest` — get promo request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetPromoRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
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
                value: BinaryParamValue::String(self.auth_token.expose_secret().to_owned()),
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
    pub auth_token: RedactedProtoString,
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
                value: BinaryParamValue::String(self.auth_token.expose_secret().to_owned()),
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
    pub auth_token: Option<RedactedProtoString>,
    /// The `verify_token` field (verify token).
    pub verify_token: Option<RedactedProtoString>,
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
                value: BinaryParamValue::String(auth_token.expose_secret().to_owned()),
            });
        }
        if let Some(verify_token) = &self.verify_token {
            params.push(BinaryParam {
                name: "verifytoken".to_owned(),
                value: BinaryParamValue::String(verify_token.expose_secret().to_owned()),
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

/// Wire DTO for the `changepassword` endpoint (audit H1 fixed). All secret
/// fields use `RedactedProtoString` so `Debug` output never leaks credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangePasswordRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `current_password` field (current password).
    pub current_password: RedactedProtoString,
    /// The `new_password` field (new password).
    pub new_password: RedactedProtoString,
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
                value: BinaryParamValue::String(self.auth_token.expose_secret().to_owned()),
            },
            BinaryParam {
                name: "oldpassword".to_owned(),
                value: BinaryParamValue::String(self.current_password.expose_secret().to_owned()),
            },
            BinaryParam {
                name: "newpassword".to_owned(),
                value: BinaryParamValue::String(self.new_password.expose_secret().to_owned()),
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
    pub password: RedactedProtoString,
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
                value: BinaryParamValue::String(self.password.expose_secret().to_owned()),
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
