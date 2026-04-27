//! Wire-level method builders for share operations (list, add, remove,
//! modify, accept/decline/cancel, contacts, teams). Consumed by
//! `shares_api`.

use crate::binary_api::{BinaryParam, BinaryParamValue};

use super::ProtocolMethod;
use crate::redacted::RedactedProtoString;

fn auth_param(token: &RedactedProtoString) -> BinaryParam {
    BinaryParam {
        name: "auth".to_owned(),
        value: BinaryParamValue::String(token.expose_secret().to_owned()),
    }
}

fn number(name: &str, value: u64) -> BinaryParam {
    BinaryParam {
        name: name.to_owned(),
        value: BinaryParamValue::Number(value),
    }
}

fn string(name: &str, value: impl Into<String>) -> BinaryParam {
    BinaryParam {
        name: name.to_owned(),
        value: BinaryParamValue::String(value.into()),
    }
}

/// `ListShareRequestsRequest` — list share requests request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListShareRequestsRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `incoming` field (incoming).
    pub incoming: bool,
}

impl ProtocolMethod for ListShareRequestsRequest {
    fn command_name(&self) -> &'static str {
        "listsharerequests"
    }
    fn params(&self) -> Vec<BinaryParam> {
        vec![
            auth_param(&self.auth_token),
            string("timeformat", "timestamp"),
            number("incoming", if self.incoming { 1 } else { 0 }),
        ]
    }
}

/// `ListSharesRequest` — list shares request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListSharesRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `incoming` field (incoming).
    pub incoming: bool,
}

impl ProtocolMethod for ListSharesRequest {
    fn command_name(&self) -> &'static str {
        "listshares"
    }
    fn params(&self) -> Vec<BinaryParam> {
        vec![
            auth_param(&self.auth_token),
            string("timeformat", "timestamp"),
            number("norequests", 1),
            number("incoming", if self.incoming { 1 } else { 0 }),
        ]
    }
}

/// `ShareFolderRequest` — share folder request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareFolderRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `folder_id` field (folder id).
    pub folder_id: u64,
    /// The `name` field (name).
    pub name: String,
    /// The `mail` field (mail).
    pub mail: String,
    /// The `message` field (message).
    pub message: String,
    /// The `permissions_bits` field (permissions bits).
    pub permissions_bits: u32,
    /// Optional crypto hint (retained crypto_share_folder variant).
    pub hint: Option<String>,
    /// Optional temppass-derived base64 re-wrapped private key. Paired
    /// with [`Self::signature`]. Mirrors `privatekey` in C
    /// `pclsync/psynclib.c` @ 1353.
    pub private_key: Option<String>,
    /// Detached signature for the temppass-derived wrapper. Mirrors
    /// `signature` in C `pclsync/psynclib.c` @ 1354.
    pub signature: Option<String>,
    /// Strict-mode flag (C always sends `strictmode=1` on the crypto
    /// variants and the non-crypto strict path).
    pub strict_mode: bool,
    /// Optional base64 RSA-4096-OAEP ciphertext wrapping the sharer's
    /// folder `sym_key_ver1` against the recipient's public key. Mirrors
    /// the `sharedfolderkey` parameter of the C client's crypto share
    /// path (`pclsync/psynclib.c:1322` / `pssl.c:718..740`). Wired by
    /// [`pcloud_crypto::share_rsa::wrap_share_invitation_b64`].
    pub shared_folder_key: Option<String>,
}

impl ProtocolMethod for ShareFolderRequest {
    fn command_name(&self) -> &'static str {
        "sharefolder"
    }
    fn params(&self) -> Vec<BinaryParam> {
        let mut params = vec![
            auth_param(&self.auth_token),
            number("folderid", self.folder_id),
            string("name", self.name.clone()),
            string("mail", self.mail.clone()),
            string("message", self.message.clone()),
            number("permissions", u64::from(self.permissions_bits)),
        ];
        if let Some(hint) = self.hint.clone() {
            params.push(string("hint", hint));
        }
        if let Some(pk) = self.private_key.clone() {
            params.push(string("privatekey", pk));
        }
        if let Some(sig) = self.signature.clone() {
            params.push(string("signature", sig));
        }
        if let Some(sfk) = self.shared_folder_key.clone() {
            params.push(string("sharedfolderkey", sfk));
        }
        if self.strict_mode {
            params.push(number("strictmode", 1));
        }
        params
    }
}

/// `CancelShareRequestRequest` — cancel share request request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelShareRequestRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `share_request_id` field (share request id).
    pub share_request_id: u64,
}

impl ProtocolMethod for CancelShareRequestRequest {
    fn command_name(&self) -> &'static str {
        "cancelsharerequest"
    }
    fn params(&self) -> Vec<BinaryParam> {
        vec![
            auth_param(&self.auth_token),
            number("sharerequestid", self.share_request_id),
        ]
    }
}

/// `DeclineShareRequestRequest` — decline share request request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclineShareRequestRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `share_request_id` field (share request id).
    pub share_request_id: u64,
}

impl ProtocolMethod for DeclineShareRequestRequest {
    fn command_name(&self) -> &'static str {
        "declineshare"
    }
    fn params(&self) -> Vec<BinaryParam> {
        vec![
            auth_param(&self.auth_token),
            number("sharerequestid", self.share_request_id),
        ]
    }
}

/// `AcceptShareRequestRequest` — accept share request request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptShareRequestRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `share_request_id` field (share request id).
    pub share_request_id: u64,
    /// The `to_folder_id` field (to folder id).
    pub to_folder_id: u64,
    /// The `name` field (name).
    pub name: Option<String>,
}

impl ProtocolMethod for AcceptShareRequestRequest {
    fn command_name(&self) -> &'static str {
        "acceptshare"
    }
    fn params(&self) -> Vec<BinaryParam> {
        let mut params = vec![
            auth_param(&self.auth_token),
            number("sharerequestid", self.share_request_id),
            number("folderid", self.to_folder_id),
        ];
        if let Some(name) = self.name.clone() {
            params.push(string("name", name));
        }
        params
    }
}

/// `RemoveShareRequest` — remove share request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveShareRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `share_id` field (share id).
    pub share_id: u64,
}

impl ProtocolMethod for RemoveShareRequest {
    fn command_name(&self) -> &'static str {
        "removeshare"
    }
    fn params(&self) -> Vec<BinaryParam> {
        vec![
            auth_param(&self.auth_token),
            number("shareid", self.share_id),
        ]
    }
}

/// `ModifyShareRequest` — modify share request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModifyShareRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `share_id` field (share id).
    pub share_id: u64,
    /// The `permissions_bits` field (permissions bits).
    pub permissions_bits: u32,
}

impl ProtocolMethod for ModifyShareRequest {
    fn command_name(&self) -> &'static str {
        "changeshare"
    }
    fn params(&self) -> Vec<BinaryParam> {
        vec![
            auth_param(&self.auth_token),
            number("shareid", self.share_id),
            number("permissions", u64::from(self.permissions_bits)),
        ]
    }
}

/// `AccountStopShareRequest` — account stop share request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStopShareRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `user_share_ids` field (user share ids).
    pub user_share_ids: Vec<u64>,
    /// The `team_share_ids` field (team share ids).
    pub team_share_ids: Vec<u64>,
}

impl ProtocolMethod for AccountStopShareRequest {
    fn command_name(&self) -> &'static str {
        "account_stopshare"
    }
    fn params(&self) -> Vec<BinaryParam> {
        let mut params = vec![auth_param(&self.auth_token)];
        for id in &self.user_share_ids {
            params.push(number("usershareid", *id));
        }
        for id in &self.team_share_ids {
            params.push(number("teamshareid", *id));
        }
        params
    }
}

/// `AccountModifyShareRequest` — account modify share request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountModifyShareRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `user_shares` field (user shares).
    pub user_shares: Vec<(u64, u32)>,
    /// The `team_shares` field (team shares).
    pub team_shares: Vec<(u64, u32)>,
}

impl ProtocolMethod for AccountModifyShareRequest {
    fn command_name(&self) -> &'static str {
        "account_modifyshare"
    }
    fn params(&self) -> Vec<BinaryParam> {
        let mut params = vec![auth_param(&self.auth_token)];
        for (id, perms) in &self.user_shares {
            params.push(number("usershareid", *id));
            params.push(number("permissions", u64::from(*perms)));
        }
        for (id, perms) in &self.team_shares {
            params.push(number("teamshareid", *id));
            params.push(number("permissions", u64::from(*perms)));
        }
        params
    }
}

/// `AccountTeamShareRequest` — account team share request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountTeamShareRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `folder_id` field (folder id).
    pub folder_id: u64,
    /// The `name` field (name).
    pub name: String,
    /// The `team_id` field (team id).
    pub team_id: u64,
    /// The `message` field (message).
    pub message: String,
    /// The `permissions_bits` field (permissions bits).
    pub permissions_bits: u32,
    /// The `hint` field (hint).
    pub hint: Option<String>,
    /// Optional temppass-derived base64 re-wrapped private key. Mirrors
    /// `privatekey` in C `pclsync/psynclib.c` @ 1404.
    pub private_key: Option<String>,
    /// Detached signature for the temppass-derived wrapper. Mirrors
    /// `signature` in C `pclsync/psynclib.c` @ 1405.
    pub signature: Option<String>,
    /// Optional base64 RSA-4096-OAEP ciphertext wrapping the sharer's
    /// folder `sym_key_ver1` against the team's shared public key.
    /// Mirrors the `teamshare_key` parameter of the C client's crypto
    /// account_teamshare path (`pclsync/psynclib.c:1372`). Wired by
    /// [`pcloud_crypto::share_rsa::wrap_share_invitation_b64`].
    pub team_share_key: Option<String>,
}

impl ProtocolMethod for AccountTeamShareRequest {
    fn command_name(&self) -> &'static str {
        "account_teamshare"
    }
    fn params(&self) -> Vec<BinaryParam> {
        let mut params = vec![
            auth_param(&self.auth_token),
            number("folderid", self.folder_id),
            string("name", self.name.clone()),
            number("teamid", self.team_id),
            string("message", self.message.clone()),
            number("permissions", u64::from(self.permissions_bits)),
        ];
        if let Some(hint) = self.hint.clone() {
            params.push(string("hint", hint));
        }
        if let Some(pk) = self.private_key.clone() {
            params.push(string("privatekey", pk));
        }
        if let Some(sig) = self.signature.clone() {
            params.push(string("signature", sig));
        }
        if let Some(tsk) = self.team_share_key.clone() {
            params.push(string("teamshare_key", tsk));
        }
        params
    }
}

/// `ContactListRequest` — contact list request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactListRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
}

impl ProtocolMethod for ContactListRequest {
    fn command_name(&self) -> &'static str {
        "contactlist"
    }
    fn params(&self) -> Vec<BinaryParam> {
        vec![auth_param(&self.auth_token)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_string<'a>(params: &'a [BinaryParam], name: &str) -> Option<&'a str> {
        params
            .iter()
            .find(|p| p.name == name)
            .and_then(|p| match &p.value {
                BinaryParamValue::String(s) => Some(s.as_str()),
                _ => None,
            })
    }

    #[test]
    fn share_folder_request_emits_sharedfolderkey_when_set() {
        let req = ShareFolderRequest {
            auth_token: "tok".into(),
            folder_id: 42,
            name: "shared".into(),
            mail: "bob@example.com".into(),
            message: "hi".into(),
            permissions_bits: 3,
            hint: None,
            private_key: None,
            signature: None,
            strict_mode: true,
            shared_folder_key: Some("d29vdA==".into()),
        };
        let params = req.params();
        assert_eq!(find_string(&params, "sharedfolderkey"), Some("d29vdA=="));
        assert!(find_string(&params, "privatekey").is_none());
        assert!(find_string(&params, "signature").is_none());
        // strictmode present as number 1.
        assert!(
            params
                .iter()
                .any(|p| p.name == "strictmode" && matches!(p.value, BinaryParamValue::Number(1)))
        );
    }

    #[test]
    fn share_folder_request_omits_sharedfolderkey_when_none() {
        let req = ShareFolderRequest {
            auth_token: "tok".into(),
            folder_id: 42,
            name: "n".into(),
            mail: "a@b".into(),
            message: "m".into(),
            permissions_bits: 3,
            hint: None,
            private_key: None,
            signature: None,
            strict_mode: false,
            shared_folder_key: None,
        };
        let params = req.params();
        assert!(find_string(&params, "sharedfolderkey").is_none());
    }

    #[test]
    fn account_team_share_request_emits_teamsharekey_when_set() {
        let req = AccountTeamShareRequest {
            auth_token: "tok".into(),
            folder_id: 42,
            name: "t".into(),
            team_id: 9,
            message: "m".into(),
            permissions_bits: 7,
            hint: Some("hint".into()),
            private_key: None,
            signature: None,
            team_share_key: Some("Zm9v".into()),
        };
        let params = req.params();
        assert_eq!(find_string(&params, "teamshare_key"), Some("Zm9v"));
        assert!(find_string(&params, "privatekey").is_none());
        assert_eq!(find_string(&params, "hint"), Some("hint"));
    }

    #[test]
    fn account_team_share_request_omits_teamsharekey_when_none() {
        let req = AccountTeamShareRequest {
            auth_token: "tok".into(),
            folder_id: 42,
            name: "t".into(),
            team_id: 9,
            message: "m".into(),
            permissions_bits: 7,
            hint: None,
            private_key: None,
            signature: None,
            team_share_key: None,
        };
        let params = req.params();
        assert!(find_string(&params, "teamshare_key").is_none());
    }
}
