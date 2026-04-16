//! Wire-level method builders for share operations (list, add, remove,
//! modify, accept/decline/cancel, contacts, teams). Consumed by
//! `shares_api`.

use crate::binary_api::{BinaryParam, BinaryParamValue};

use super::ProtocolMethod;

fn auth_param(token: &str) -> BinaryParam {
    BinaryParam {
        name: "auth".to_owned(),
        value: BinaryParamValue::String(token.to_owned()),
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
    pub auth_token: String,
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
    pub auth_token: String,
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
    pub auth_token: String,
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
    pub auth_token: String,
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
    pub auth_token: String,
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
    pub auth_token: String,
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
    pub auth_token: String,
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
    pub auth_token: String,
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
    pub auth_token: String,
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
    pub auth_token: String,
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
    pub auth_token: String,
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
        params
    }
}

/// `ContactListRequest` — contact list request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactListRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: String,
}

impl ProtocolMethod for ContactListRequest {
    fn command_name(&self) -> &'static str {
        "contactlist"
    }
    fn params(&self) -> Vec<BinaryParam> {
        vec![auth_param(&self.auth_token)]
    }
}
