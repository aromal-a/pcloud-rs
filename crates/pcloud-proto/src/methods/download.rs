//! Wire-level method builders for download (getfilelink, thumb,
//! streaming). Consumed by `transfer_api` and `http_download`.

use crate::binary_api::{BinaryParam, BinaryParamValue};
use crate::methods::ProtocolMethod;

/// `GetFileLinkRequest` — get file link request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetFileLinkRequest {
    /// The `file_id` field (file id).
    pub file_id: u64,
    /// The `auth_token` field (auth token).
    pub auth_token: String,
    /// The `forced_host` field (forced host).
    pub forced_host: Option<String>,
}

impl GetFileLinkRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "getfilelink"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = vec![
            BinaryParam {
                name: "auth".to_owned(),
                value: BinaryParamValue::String(self.auth_token.clone()),
            },
            BinaryParam {
                name: "fileid".to_owned(),
                value: BinaryParamValue::Number(self.file_id),
            },
        ];
        if let Some(host) = &self.forced_host {
            params.push(BinaryParam {
                name: "forcedownloadhost".to_owned(),
                value: BinaryParamValue::String(host.clone()),
            });
        }
        params
    }
}

impl ProtocolMethod for GetFileLinkRequest {
    fn command_name(&self) -> &'static str {
        GetFileLinkRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        GetFileLinkRequest::params(self)
    }
}
