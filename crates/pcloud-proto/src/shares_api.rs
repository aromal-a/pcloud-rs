//! Shares / business / teams protocol client: share request list, share
//! CRUD, accept/decline/cancel, contacts, my teams, account team-share,
//! and crypto-aware retained variants. Consumed by
//! `pcloud-backends::shares_backend`.
//!
//! ## Role in the request pipeline
//!
//! Wraps the pCloud share / business / team commands and projects
//! responses into `pcloud-model` types. Crypto-aware variants
//! accept temppass material produced from an unlocked crypto key
//! by the caller; this module never unlocks or persists the key.
//!
//! ## Security considerations
//!
//! - Share permissions are surfaced as a typed struct
//!   ([`pcloud_model::shares::SharePermissions`]), not raw
//!   integers, to prevent accidental escalation.
//! - Temppass material is accepted as `&[u8]` and never logged.
//! - Server-returned request / share ids are untrusted; callers
//!   must authenticate that they refer to the active user before
//!   acting on them.
//!
//! Portable; no platform gating.

use pcloud_model::shares::{
    ContactEntry, ShareDirection, ShareEntry, ShareMutationResult, SharePermissions,
    ShareRequestEntry,
};
use thiserror::Error;

use crate::{
    ProtocolMethod,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    methods::shares::{
        AcceptShareRequestRequest, AccountModifyShareRequest, AccountStopShareRequest,
        AccountTeamShareRequest, CancelShareRequestRequest, ContactListRequest,
        DeclineShareRequestRequest, ListShareRequestsRequest, ListSharesRequest,
        ModifyShareRequest, RemoveShareRequest, ShareFolderRequest,
    },
    response::{HashView, Value},
};

/// `SharesApi` — shares api.
#[derive(Debug)]
pub struct SharesApi<T> {
    transport: T,
}

/// `SharesApiError` — shares api error.
#[derive(Debug, Error)]
pub enum SharesApiError<E: std::error::Error + Send + Sync + 'static> {
    /// `Encode` variant (encode).
    #[error(transparent)]
    Encode(#[from] crate::FrameParseError),
    /// `Transport` variant (transport).
    #[error("transport failed: {0}")]
    Transport(E),
    /// `Result` variant (result).
    #[error("share method returned non-zero result code {result} ({message:?})")]
    Result {
        /// The `result` field (result).
        result: u64,
        /// The `message` field (message).
        message: Option<String>,
    },
    /// `Malformed` variant (malformed).
    #[error("response was malformed: {0}")]
    Malformed(&'static str),
}

impl<T> SharesApi<T> {
    /// `new` — new.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T> SharesApi<T>
where
    T: ProtocolTransport + ApiServerHintConsumer,
{
    /// `apply_api_server_hint` — apply api server hint.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn apply_api_server_hint(&self, api_server: &str) {
        self.transport.apply_api_server_hint(api_server);
    }

    /// `list_share_requests` — list share requests.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn list_share_requests(
        &self,
        auth_token: impl Into<String>,
        incoming: bool,
    ) -> Result<Vec<ShareRequestEntry>, SharesApiError<T::Error>> {
        let req = ListShareRequestsRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            incoming,
        };
        let response = self
            .transport
            .execute(&req.encode()?)
            .map_err(SharesApiError::Transport)?;
        let hash = response.as_hash().ok_or(SharesApiError::Malformed(
            "listsharerequests response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        let direction = if incoming {
            ShareDirection::Incoming
        } else {
            ShareDirection::Outgoing
        };
        let key = if incoming { "incoming" } else { "outgoing" };
        let list = hash.get_array(key).or_else(|| hash.get_array("requests"));
        let Some(list) = list else {
            return Ok(Vec::new());
        };
        list.iter()
            .map(|v| parse_share_request::<T::Error>(v, direction))
            .collect()
    }

    /// `list_shares` — list shares.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn list_shares(
        &self,
        auth_token: impl Into<String>,
        incoming: bool,
    ) -> Result<Vec<ShareEntry>, SharesApiError<T::Error>> {
        let req = ListSharesRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            incoming,
        };
        let response = self
            .transport
            .execute(&req.encode()?)
            .map_err(SharesApiError::Transport)?;
        let hash = response.as_hash().ok_or(SharesApiError::Malformed(
            "listshares response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        let direction = if incoming {
            ShareDirection::Incoming
        } else {
            ShareDirection::Outgoing
        };
        let key = if incoming { "incoming" } else { "outgoing" };
        let list = hash.get_array(key).or_else(|| hash.get_array("shares"));
        let Some(list) = list else {
            return Ok(Vec::new());
        };
        list.iter()
            .map(|v| parse_share::<T::Error>(v, direction))
            .collect()
    }

    /// `share_folder` — share folder.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn share_folder(
        &self,
        auth_token: impl Into<String>,
        folder_id: u64,
        name: impl Into<String>,
        mail: impl Into<String>,
        message: impl Into<String>,
        permissions: SharePermissions,
        hint: Option<String>,
    ) -> Result<ShareMutationResult, SharesApiError<T::Error>> {
        let req = ShareFolderRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            folder_id,
            name: name.into(),
            mail: mail.into(),
            message: message.into(),
            permissions_bits: permissions.to_bits(),
            hint,
            private_key: None,
            signature: None,
            strict_mode: false,
        };
        let response = self
            .transport
            .execute(&req.encode()?)
            .map_err(SharesApiError::Transport)?;
        let hash = response.as_hash().ok_or(SharesApiError::Malformed(
            "sharefolder response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        Ok(ShareMutationResult {
            share_request_id: hash.get_number("sharerequestid"),
        })
    }

    /// `cancel_share_request` — cancel share request.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn cancel_share_request(
        &self,
        auth_token: impl Into<String>,
        share_request_id: u64,
    ) -> Result<(), SharesApiError<T::Error>> {
        let req = CancelShareRequestRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            share_request_id,
        };
        self.execute_unit(&req, "cancelsharerequest")
    }

    /// `decline_share_request` — decline share request.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn decline_share_request(
        &self,
        auth_token: impl Into<String>,
        share_request_id: u64,
    ) -> Result<(), SharesApiError<T::Error>> {
        let req = DeclineShareRequestRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            share_request_id,
        };
        self.execute_unit(&req, "declineshare")
    }

    /// `accept_share_request` — accept share request.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn accept_share_request(
        &self,
        auth_token: impl Into<String>,
        share_request_id: u64,
        to_folder_id: u64,
        name: Option<String>,
    ) -> Result<(), SharesApiError<T::Error>> {
        let req = AcceptShareRequestRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            share_request_id,
            to_folder_id,
            name,
        };
        self.execute_unit(&req, "acceptshare")
    }

    /// `remove_share` — remove share.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn remove_share(
        &self,
        auth_token: impl Into<String>,
        share_id: u64,
    ) -> Result<(), SharesApiError<T::Error>> {
        let req = RemoveShareRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            share_id,
        };
        self.execute_unit(&req, "removeshare")
    }

    /// `modify_share` — modify share.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn modify_share(
        &self,
        auth_token: impl Into<String>,
        share_id: u64,
        permissions: SharePermissions,
    ) -> Result<(), SharesApiError<T::Error>> {
        let req = ModifyShareRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            share_id,
            permissions_bits: permissions.to_bits(),
        };
        self.execute_unit(&req, "changeshare")
    }

    /// `account_stop_share` — account stop share.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn account_stop_share(
        &self,
        auth_token: impl Into<String>,
        user_share_ids: Vec<u64>,
        team_share_ids: Vec<u64>,
    ) -> Result<(), SharesApiError<T::Error>> {
        let req = AccountStopShareRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            user_share_ids,
            team_share_ids,
        };
        self.execute_unit(&req, "account_stopshare")
    }

    /// `account_modify_share` — account modify share.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn account_modify_share(
        &self,
        auth_token: impl Into<String>,
        user_shares: Vec<(u64, SharePermissions)>,
        team_shares: Vec<(u64, SharePermissions)>,
    ) -> Result<(), SharesApiError<T::Error>> {
        let req = AccountModifyShareRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            user_shares: user_shares
                .into_iter()
                .map(|(id, p)| (id, p.to_bits()))
                .collect(),
            team_shares: team_shares
                .into_iter()
                .map(|(id, p)| (id, p.to_bits()))
                .collect(),
        };
        self.execute_unit(&req, "account_modifyshare")
    }

    /// `account_team_share` — account team share.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn account_team_share(
        &self,
        auth_token: impl Into<String>,
        folder_id: u64,
        name: impl Into<String>,
        team_id: u64,
        message: impl Into<String>,
        permissions: SharePermissions,
        hint: Option<String>,
    ) -> Result<ShareMutationResult, SharesApiError<T::Error>> {
        let req = AccountTeamShareRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            folder_id,
            name: name.into(),
            team_id,
            message: message.into(),
            permissions_bits: permissions.to_bits(),
            hint,
            private_key: None,
            signature: None,
        };
        let response = self
            .transport
            .execute(&req.encode()?)
            .map_err(SharesApiError::Transport)?;
        let hash = response.as_hash().ok_or(SharesApiError::Malformed(
            "account_teamshare response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        Ok(ShareMutationResult {
            share_request_id: hash.get_number("sharerequestid"),
        })
    }

    /// `psync_crypto_share_folder` wire-level sibling of
    /// [`Self::share_folder`]: the caller has already derived the
    /// base64 `private_key` + `signature` via
    /// `pcloud_crypto::derive_temppass_wire` and we forward them
    /// alongside `hint` and `strictmode=1`. The *derivation* of the
    /// blob lives in `pcloud-crypto` so no master-key material ever
    /// crosses into the proto crate.
    #[allow(clippy::too_many_arguments)]
    pub fn crypto_share_folder(
        &self,
        auth_token: impl Into<String>,
        folder_id: u64,
        name: impl Into<String>,
        mail: impl Into<String>,
        message: impl Into<String>,
        permissions: SharePermissions,
        hint: Option<String>,
        private_key_b64: String,
        signature_b64: String,
    ) -> Result<ShareMutationResult, SharesApiError<T::Error>> {
        let req = ShareFolderRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            folder_id,
            name: name.into(),
            mail: mail.into(),
            message: message.into(),
            permissions_bits: permissions.to_bits(),
            hint,
            private_key: Some(private_key_b64),
            signature: Some(signature_b64),
            strict_mode: true,
        };
        let response = self
            .transport
            .execute(&req.encode()?)
            .map_err(SharesApiError::Transport)?;
        let hash = response.as_hash().ok_or(SharesApiError::Malformed(
            "sharefolder response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        Ok(ShareMutationResult {
            share_request_id: hash.get_number("sharerequestid"),
        })
    }

    /// `psync_crypto_account_teamshare` wire-level sibling of
    /// [`Self::account_team_share`].
    #[allow(clippy::too_many_arguments)]
    pub fn crypto_account_team_share(
        &self,
        auth_token: impl Into<String>,
        folder_id: u64,
        name: impl Into<String>,
        team_id: u64,
        message: impl Into<String>,
        permissions: SharePermissions,
        hint: Option<String>,
        private_key_b64: String,
        signature_b64: String,
    ) -> Result<ShareMutationResult, SharesApiError<T::Error>> {
        let req = AccountTeamShareRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            folder_id,
            name: name.into(),
            team_id,
            message: message.into(),
            permissions_bits: permissions.to_bits(),
            hint,
            private_key: Some(private_key_b64),
            signature: Some(signature_b64),
        };
        let response = self
            .transport
            .execute(&req.encode()?)
            .map_err(SharesApiError::Transport)?;
        let hash = response.as_hash().ok_or(SharesApiError::Malformed(
            "account_teamshare response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        Ok(ShareMutationResult {
            share_request_id: hash.get_number("sharerequestid"),
        })
    }

    /// `contact_list` — contact list.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn contact_list(
        &self,
        auth_token: impl Into<String>,
    ) -> Result<Vec<ContactEntry>, SharesApiError<T::Error>> {
        let req = ContactListRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
        };
        let response = self
            .transport
            .execute(&req.encode()?)
            .map_err(SharesApiError::Transport)?;
        let hash = response.as_hash().ok_or(SharesApiError::Malformed(
            "contactlist response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        let list = hash
            .get_array("contacts")
            .ok_or(SharesApiError::Malformed("contactlist missing contacts"))?;
        list.iter().map(parse_contact::<T::Error>).collect()
    }

    fn execute_unit<M: ProtocolMethod>(
        &self,
        method: &M,
        ctx: &'static str,
    ) -> Result<(), SharesApiError<T::Error>> {
        let response = self
            .transport
            .execute(&method.encode()?)
            .map_err(SharesApiError::Transport)?;
        let hash = response.as_hash().ok_or_else(|| {
            SharesApiError::Malformed(match ctx {
                "cancelsharerequest" => "cancelsharerequest response was not a hash",
                "declineshare" => "declineshare response was not a hash",
                "acceptshare" => "acceptshare response was not a hash",
                "removeshare" => "removeshare response was not a hash",
                "changeshare" => "changeshare response was not a hash",
                "account_stopshare" => "account_stopshare response was not a hash",
                "account_modifyshare" => "account_modifyshare response was not a hash",
                _ => "share response was not a hash",
            })
        })?;
        expect_ok_result(hash)
    }
}

fn expect_ok_result<E>(hash: HashView<'_>) -> Result<(), SharesApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let result = hash.get_number("result").unwrap_or(0);
    if result == 0 {
        return Ok(());
    }
    Err(SharesApiError::Result {
        result,
        message: hash.get_string("error").map(ToOwned::to_owned),
    })
}

fn parse_share_request<E>(
    value: &Value,
    direction: ShareDirection,
) -> Result<ShareRequestEntry, SharesApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let hash = value.as_hash().ok_or(SharesApiError::Malformed(
        "share request entry was not a hash",
    ))?;
    Ok(ShareRequestEntry {
        share_request_id: hash
            .get_number("sharerequestid")
            .ok_or(SharesApiError::Malformed(
                "share request missing sharerequestid",
            ))?,
        folder_id: hash.get_number("folderid").unwrap_or(0),
        share_name: hash
            .get_string("sharename")
            .or_else(|| hash.get_string("name"))
            .unwrap_or("")
            .to_owned(),
        from_user_id: hash.get_number("fromuserid").unwrap_or(0),
        from_email: hash.get_string("frommail").unwrap_or("").to_owned(),
        to_email: hash.get_string("tomail").unwrap_or("").to_owned(),
        permissions: SharePermissions::from_bits(hash.get_number("permissions").unwrap_or(0) as u32),
        created: hash.get_number("created").unwrap_or(0),
        message: hash.get_string("message").map(ToOwned::to_owned),
        direction,
    })
}

fn parse_share<E>(value: &Value, direction: ShareDirection) -> Result<ShareEntry, SharesApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let hash = value
        .as_hash()
        .ok_or(SharesApiError::Malformed("share entry was not a hash"))?;
    Ok(ShareEntry {
        share_id: hash
            .get_number("shareid")
            .ok_or(SharesApiError::Malformed("share missing shareid"))?,
        folder_id: hash.get_number("folderid").unwrap_or(0),
        share_name: hash
            .get_string("sharename")
            .or_else(|| hash.get_string("name"))
            .unwrap_or("")
            .to_owned(),
        from_user_id: hash.get_number("fromuserid").unwrap_or(0),
        from_email: hash.get_string("frommail").unwrap_or("").to_owned(),
        to_user_id: hash.get_number("touserid").unwrap_or(0),
        to_email: hash.get_string("tomail").unwrap_or("").to_owned(),
        permissions: SharePermissions::from_bits(hash.get_number("permissions").unwrap_or(0) as u32),
        created: hash.get_number("created").unwrap_or(0),
        direction,
        is_team: hash.get_bool("isteam").unwrap_or(false),
        team_id: hash.get_number("teamid"),
    })
}

fn parse_contact<E>(value: &Value) -> Result<ContactEntry, SharesApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let hash = value
        .as_hash()
        .ok_or(SharesApiError::Malformed("contact entry was not a hash"))?;
    let contact_type = hash.get_number("type").unwrap_or(1) as u32;
    Ok(ContactEntry {
        contact_type,
        contact_id: hash
            .get_number("id")
            .or_else(|| hash.get_number("teamid"))
            .or_else(|| hash.get_number("userid"))
            .unwrap_or(0),
        name: hash
            .get_string("name")
            .or_else(|| hash.get_string("teamname"))
            .unwrap_or("")
            .to_owned(),
        email: hash.get_string("mail").map(ToOwned::to_owned),
        team_id: hash.get_number("teamid"),
    })
}
