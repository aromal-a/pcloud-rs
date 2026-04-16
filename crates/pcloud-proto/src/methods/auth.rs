//! Wire-level method builders for auth (login, TFA, userinfo,
//! refresh). Consumed by `auth_api`. Secrets transit via
//! `pcloud-secret` wrappers.

use crate::binary_api::BinaryParam;
use crate::methods::ProtocolMethod;
use sha1::{Digest, Sha1};

/// `AuthRequestContext` — auth request context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRequestContext {
    /// The `timeformat` field (timeformat).
    pub timeformat: String,
    /// The `os_version` field (os version).
    pub os_version: String,
    /// The `app_version` field (app version).
    pub app_version: String,
    /// The `device_id` field (device id).
    pub device_id: String,
    /// The `device_name` field (device name).
    pub device_name: String,
    /// The `os_id` field (os id).
    pub os_id: u64,
    /// The `get_auth` field (get auth).
    pub get_auth: bool,
    /// The `crypto_keys_sign` field (crypto keys sign).
    pub crypto_keys_sign: bool,
    /// The `get_api_server` field (get api server).
    pub get_api_server: bool,
    /// The `get_last_subscription` field (get last subscription).
    pub get_last_subscription: bool,
}

impl Default for AuthRequestContext {
    fn default() -> Self {
        Self {
            timeformat: "timestamp".to_owned(),
            os_version: "Desktop, Linux".to_owned(),
            app_version: "1.5.1".to_owned(),
            device_id: legacy_compatible_device_id(),
            device_name: "Desktop, Linux, 1.5.1".to_owned(),
            os_id: 7,
            get_auth: true,
            crypto_keys_sign: true,
            get_api_server: true,
            get_last_subscription: true,
        }
    }
}

impl AuthRequestContext {
    /// `standard_params` — standard params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn standard_params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(10);
        params.push(BinaryParam::string("timeformat", self.timeformat.as_str()));
        params.push(BinaryParam::string("osversion", self.os_version.as_str()));
        params.push(BinaryParam::string("appversion", self.app_version.as_str()));
        params.push(BinaryParam::string("deviceid", self.device_id.as_str()));
        params.push(BinaryParam::string("device", self.device_name.as_str()));
        params.push(BinaryParam::bool("getauth", self.get_auth));
        params.push(BinaryParam::bool("cryptokeyssign", self.crypto_keys_sign));
        params.push(BinaryParam::bool("getapiserver", self.get_api_server));
        params.push(BinaryParam::bool(
            "getlastsubscription",
            self.get_last_subscription,
        ));
        params.push(BinaryParam::number("os", self.os_id));
        params
    }
}

/// `GetDigestRequest` — get digest request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetDigestRequest;

impl GetDigestRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "getdigest"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(1);
        params.push(BinaryParam::string("MS", "sucks"));
        params
    }
}

impl ProtocolMethod for GetDigestRequest {
    fn command_name(&self) -> &'static str {
        GetDigestRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        GetDigestRequest::params(self)
    }
}

/// `LoginRequest` — login request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginRequest {
    /// The `username` field (username).
    pub username: String,
}

impl LoginRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "login"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(1);
        params.push(BinaryParam::string("username", self.username.as_str()));
        params
    }

    /// `password_params` — password params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn password_params(&self, password: &str) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(2);
        params.push(BinaryParam::string("username", self.username.as_str()));
        params.push(BinaryParam::string("password", password));
        params
    }
}

impl ProtocolMethod for LoginRequest {
    fn command_name(&self) -> &'static str {
        LoginRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        LoginRequest::params(self)
    }
}

/// `UserInfoRequest` — user info request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfoRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: String,
    /// The `context` field (context).
    pub context: AuthRequestContext,
}

impl UserInfoRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "userinfo"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(1 + 10);
        params.push(BinaryParam::string("auth", self.auth_token.as_str()));
        params.extend(self.context.standard_params());
        params
    }
}

impl ProtocolMethod for UserInfoRequest {
    fn command_name(&self) -> &'static str {
        UserInfoRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        UserInfoRequest::params(self)
    }
}

/// `TwoFactorLoginRequest` — two factor login request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoFactorLoginRequest {
    /// The `token` field (token).
    pub token: String,
    /// The `code` field (code).
    pub code: String,
    /// The `trust_device` field (trust device).
    pub trust_device: bool,
    /// The `recovery_code` field (recovery code).
    pub recovery_code: bool,
    /// The `context` field (context).
    pub context: AuthRequestContext,
}

impl TwoFactorLoginRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        if self.recovery_code {
            "tfa_loginwithrecoverycode"
        } else {
            "tfa_login"
        }
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(3 + 10);
        params.push(BinaryParam::string("token", self.token.as_str()));
        params.push(BinaryParam::string("code", self.code.as_str()));
        params.push(BinaryParam::bool("trustdevice", self.trust_device));
        params.extend(self.context.standard_params());
        params
    }
}

impl ProtocolMethod for TwoFactorLoginRequest {
    fn command_name(&self) -> &'static str {
        TwoFactorLoginRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        TwoFactorLoginRequest::params(self)
    }
}

/// `TwoFactorSendSmsRequest` — two factor send sms request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoFactorSendSmsRequest {
    /// The `token` field (token).
    pub token: String,
}

impl TwoFactorSendSmsRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "tfa_sendcodeviasms"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(1);
        params.push(BinaryParam::string("token", self.token.as_str()));
        params
    }
}

impl ProtocolMethod for TwoFactorSendSmsRequest {
    fn command_name(&self) -> &'static str {
        TwoFactorSendSmsRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        TwoFactorSendSmsRequest::params(self)
    }
}

/// `TwoFactorSendNotificationRequest` — two factor send notification request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoFactorSendNotificationRequest {
    /// The `token` field (token).
    pub token: String,
}

impl TwoFactorSendNotificationRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "tfa_sendcodeviasysnotification"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(1);
        params.push(BinaryParam::string("token", self.token.as_str()));
        params
    }
}

impl ProtocolMethod for TwoFactorSendNotificationRequest {
    fn command_name(&self) -> &'static str {
        TwoFactorSendNotificationRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        TwoFactorSendNotificationRequest::params(self)
    }
}

/// `LoginDigestRequest` — login digest request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginDigestRequest {
    /// The `username` field (username).
    pub username: String,
    /// The `digest_token` field (digest token).
    pub digest_token: String,
    /// The `password_digest` field (password digest).
    pub password_digest: String,
    /// The `code` field (code).
    pub code: Option<String>,
    /// The `context` field (context).
    pub context: AuthRequestContext,
}

impl LoginDigestRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "login"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let base = 3 + usize::from(self.code.is_some());
        let mut params = Vec::with_capacity(base + 10);
        params.push(BinaryParam::string("username", self.username.as_str()));
        params.push(BinaryParam::string("digest", self.digest_token.as_str()));
        params.push(BinaryParam::string(
            "passworddigest",
            self.password_digest.as_str(),
        ));
        if let Some(code) = &self.code {
            params.push(BinaryParam::string("code", code.as_str()));
        }
        params.extend(self.context.standard_params());
        params
    }
}

fn legacy_compatible_device_id() -> String {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "pcloud-rust".to_owned());
    let mut hasher = Sha1::new();
    hasher.update(host.as_bytes());
    hasher.update(b":pcloud-rust-dev");
    let hex = hex_lower(&hasher.finalize());
    hex[..32].to_owned()
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

impl ProtocolMethod for LoginDigestRequest {
    fn command_name(&self) -> &'static str {
        LoginDigestRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        LoginDigestRequest::params(self)
    }
}
