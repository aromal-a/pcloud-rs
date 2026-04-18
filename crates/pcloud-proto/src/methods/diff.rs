//! Wire-level method builders for the `diff` long-poll stream used by
//! the sync engine to observe server-side changes. Consumed by the
//! sync runtime in `pcloud-engine`.

use crate::binary_api::{BinaryParam, BinaryParamValue};
use crate::methods::ProtocolMethod;
use crate::redacted::RedactedProtoString;

/// `DiffRequest` — diff request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRequest {
    /// The `cursor` field (cursor).
    pub cursor: u64,
    /// The `limit` field (limit).
    pub limit: u64,
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
}

impl DiffRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "diff"
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
                name: "timeformat".to_owned(),
                value: BinaryParamValue::String("timestamp".to_owned()),
            },
            BinaryParam {
                name: "limit".to_owned(),
                value: BinaryParamValue::Number(self.limit),
            },
            BinaryParam {
                name: "diffid".to_owned(),
                value: BinaryParamValue::Number(self.cursor),
            },
        ]
    }
}

impl ProtocolMethod for DiffRequest {
    fn command_name(&self) -> &'static str {
        DiffRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        DiffRequest::params(self)
    }
}
