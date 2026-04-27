//! Per-method builders grouped by subsystem (account, auth, backup,
//! crypto, diff, download, folder, notifications, public_links, shares,
//! upload). Each sub-module wraps the binary/HTTP wire format for one
//! family of server methods and is consumed by the sibling `*_api`
//! client modules.
//!
//! ## Design choices
//!
//! Every request type is an owned struct that carries exactly the
//! parameters the server expects. The unifying [`ProtocolMethod`]
//! trait forces each type to expose:
//!
//! - [`ProtocolMethod::command_name`] — the static command name,
//! - [`ProtocolMethod::params`] — the typed parameter vector,
//! - [`ProtocolMethod::encode`] — the default implementation which
//!   composes the above via [`crate::encode_request`].
//!
//! Keeping builders as concrete structs (rather than a single
//! generic builder with a `HashMap<String, BinaryParamValue>`) means
//! callers get compile-time checks for required fields and the
//! compiler can monomorphise each `encode` call into flat,
//! panic-free code.
//!
//! ## Security considerations
//!
//! Builders never log their fields. Secret-bearing fields (passwords,
//! TFA codes) are expected to be held in `pcloud-secret` wrappers
//! upstream and unwrapped only to build the parameter vector
//! immediately before encoding. This module does not allocate
//! intermediate copies of secret material beyond what the wire
//! format demands.
//!
//! Portable; no platform gating.

pub mod account;
pub mod auth;
pub mod backup;
pub mod crypto;
pub mod diff;
pub mod download;
pub mod folder;
pub mod notifications;
pub mod public_links;
pub mod shares;
pub mod upload;

use crate::{
    binary_api::{BinaryParam, EncodedRequest, FrameParseError, encode_request},
    response::Value,
};

/// Unifying trait every method-builder in this module implements.
///
/// The trait pins the three operations needed to drive a request
/// end-to-end — naming the command, producing the typed parameter
/// vector, and composing the final [`EncodedRequest`]. Keeping these
/// on a trait (rather than inherent methods on each struct) lets
/// callers write generic helpers (retry wrappers, logging decorators,
/// replay harnesses) against any method type.
///
/// ## Design choices
///
/// - **Trait, not enum**: the set of methods is open — new server
///   commands appear routinely — and a trait keeps each builder's
///   fields strongly typed without a central enum explosion.
/// - **`&'static str` for command names** so the wire tag lives in
///   the binary without a heap allocation per request.
/// - **`encode` has a default impl** that attaches a zero-length
///   raw body (the common case). Builders that send a non-empty
///   body (e.g. `upload_write`) override `encode` to pass the
///   correct `raw_body_len` through to [`encode_request`].
pub trait ProtocolMethod {
    /// Static command name written to the wire, e.g. `"login"` or
    /// `"listfolder"`.
    fn command_name(&self) -> &'static str;

    /// Typed parameter vector. The default [`Self::encode`] hands
    /// this straight to [`encode_request`].
    ///
    /// Builders should pre-size the returned `Vec` with
    /// `Vec::with_capacity(N)` where `N` is the exact final count;
    /// the crate-level `clippy::vec_init_then_push` allow preserves
    /// that capacity hint as documentation of the on-wire parameter
    /// count.
    fn params(&self) -> Vec<BinaryParam>;

    /// Build the fully encoded wire frame.
    ///
    /// # Errors
    ///
    /// Returns [`FrameParseError`] if the request cannot be
    /// represented on the wire (name too long, frame too large).
    fn encode(&self) -> Result<EncodedRequest, FrameParseError> {
        encode_request(self.command_name(), &self.params(), Some(0))
    }
}

/// Decoded pCloud response envelope shared by every API module.
///
/// ## Wire layout
///
/// A pCloud response is a [`Value::Hash`] at the root; by convention
/// the `result` key carries a numeric status and the rest of the
/// hash is the method-specific payload. `ParsedEnvelope` projects
/// that convention into a pair: the `result_code` (absent when the
/// server omits it — some legacy commands do) and the raw payload
/// tree for the caller's typed projection.
///
/// ## Design choices
///
/// `result_code` is `Option<u64>` rather than `u64`-with-sentinel so
/// callers cannot accidentally treat "missing" as "success". The
/// payload is owned rather than a [`Value`] reference so envelopes
/// can be retained after the parse buffer has been dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEnvelope {
    /// Numeric status code from the `result` field, if present.
    ///
    /// `0` is success; any non-zero value should be paired with the
    /// accompanying `error` string (in the payload) for display.
    pub result_code: Option<u64>,
    /// The full response tree, unmodified, ready for domain
    /// projection via [`crate::response::HashView`] accessors.
    pub payload: Value,
}

#[cfg(test)]
mod tests {
    use crate::methods::account::{
        ChangePasswordRequest, GetLocationApiRequest, GetPromoRequest, LostPasswordRequest,
        RegisterRequest, SetLanguageRequest, VerifyEmailRequest,
    };
    use crate::methods::auth::{AuthRequestContext, LoginDigestRequest, LoginRequest};
    use crate::methods::diff::DiffRequest;
    use crate::methods::download::GetFileLinkRequest;
    use crate::methods::folder::ListFolderByPathRequest;
    use crate::methods::public_links::{
        AddPublicLinkAccessRequest, ChangeBookmarkRequest, ChangePublicLinkExpireRequest,
        ChangePublicLinkPasswordRequest, ChangePublicLinkUploadRequest,
        CreateFilePublicLinkRequest, CreateFolderPublicLinkRequest, CreateTreePublicLinkRequest,
        CreateUploadLinkRequest, DeletePublicLinkRequest, DeleteUploadLinkRequest,
        ListBookmarksRequest, ListPublicLinkAccessRequest, ListPublicLinksRequest,
        ListUploadLinksRequest, RemoveBookmarkRequest, RemovePublicLinkAccessRequest,
        ShowPublicLinkRequest,
    };
    use crate::methods::upload::UploadCreateRequest;
    use pcloud_model::public_links::PublicLinkUploadPolicy;

    use super::ProtocolMethod;

    #[test]
    fn login_request_encodes() {
        let request = LoginRequest {
            username: "alice@example.com".to_owned(),
        };
        let encoded = request.encode().expect("login request should encode");
        assert_eq!(encoded.frame.command, "login");
        assert_eq!(encoded.frame.parameter_count, 1);
    }

    #[test]
    fn account_methods_build_expected_parameter_counts() {
        let promo = GetPromoRequest {
            auth_token: "token".into(),
            os_id: 3,
        };
        let set_language = SetLanguageRequest {
            auth_token: "token".into(),
            language: "en".to_owned(),
        };
        let locations = GetLocationApiRequest;
        let verify_email = VerifyEmailRequest {
            auth_token: Some("token".into()),
            verify_token: None,
        };
        let restricted_verify = VerifyEmailRequest {
            auth_token: None,
            verify_token: Some("verify-token".into()),
        };
        let lost_password = LostPasswordRequest {
            email: "alice@example.com".to_owned(),
        };
        let change_password = ChangePasswordRequest {
            auth_token: "token".into(),
            current_password: "old".into(),
            new_password: "new".into(),
            device: "Desktop".to_owned(),
        };

        assert_eq!(
            promo
                .encode()
                .expect("promo request should encode")
                .frame
                .parameter_count,
            2
        );
        assert_eq!(
            set_language
                .encode()
                .expect("set language request should encode")
                .frame
                .parameter_count,
            2
        );
        assert_eq!(
            locations
                .encode()
                .expect("locations request should encode")
                .frame
                .parameter_count,
            1
        );
        assert_eq!(
            verify_email
                .encode()
                .expect("verify email request should encode")
                .frame
                .parameter_count,
            1
        );
        assert_eq!(
            restricted_verify
                .encode()
                .expect("restricted verify request should encode")
                .frame
                .parameter_count,
            1
        );
        assert_eq!(
            lost_password
                .encode()
                .expect("lost password request should encode")
                .frame
                .parameter_count,
            1
        );
        assert_eq!(
            change_password
                .encode()
                .expect("change password request should encode")
                .frame
                .parameter_count,
            5
        );

        let register = RegisterRequest {
            email: "new@example.com".to_owned(),
            password: "strong".into(),
            terms_accepted: true,
            os_id: 3,
        };
        let encoded = register.encode().expect("register request should encode");
        assert_eq!(encoded.frame.command, "register");
        assert_eq!(encoded.frame.parameter_count, 4);
    }

    #[test]
    fn digest_auth_request_uses_login_command() {
        let request = LoginDigestRequest {
            username: "alice@example.com".to_owned(),
            digest_token: "digest-token".into(),
            password_digest: "password-digest".into(),
            code: None,
            context: AuthRequestContext::default(),
        };
        let encoded = request.encode().expect("digest auth request should encode");
        assert_eq!(encoded.frame.command, "login");
        assert_eq!(encoded.frame.parameter_count, 13);
    }

    #[test]
    fn diff_request_encodes() {
        let request = DiffRequest {
            cursor: 42,
            limit: 256,
            auth_token: "token".into(),
        };
        let encoded = request.encode().expect("diff request should encode");
        assert_eq!(encoded.frame.command, "diff");
        assert_eq!(encoded.frame.parameter_count, 4);
    }

    #[test]
    fn list_folder_request_encodes() {
        let request = ListFolderByPathRequest {
            auth_token: "token".into(),
            path: "/remote-sync".to_owned(),
        };
        let encoded = request.encode().expect("listfolder request should encode");
        assert_eq!(encoded.frame.command, "listfolder");
        assert_eq!(encoded.frame.parameter_count, 2);
    }

    #[test]
    fn download_and_upload_methods_build_expected_parameter_counts() {
        let download = GetFileLinkRequest {
            file_id: 7,
            auth_token: "token".into(),
            forced_host: Some("cdn.example".to_owned()),
        };
        let upload = UploadCreateRequest {
            auth_token: "token".into(),
            parent_folder_id: 9,
            file_name: "report.txt".to_owned(),
            file_size: 1024,
            idempotency_key: None,
        };

        let encoded_download = download.encode().expect("download request should encode");
        let encoded_upload = upload.encode().expect("upload request should encode");

        assert_eq!(encoded_download.frame.parameter_count, 3);
        assert_eq!(encoded_upload.frame.parameter_count, 4);
    }

    #[test]
    fn public_link_methods_build_expected_parameter_counts() {
        let list = ListPublicLinksRequest {
            auth_token: "token".into(),
        };
        let show = ShowPublicLinkRequest {
            auth_token: "token".into(),
            code: "abc123".to_owned(),
        };
        let delete = DeletePublicLinkRequest {
            auth_token: "token".into(),
            link_id: 7,
        };
        let create_file = CreateFilePublicLinkRequest {
            auth_token: "token".into(),
            path: "/Docs/report.txt".to_owned(),
        };
        let create_folder = CreateFolderPublicLinkRequest {
            auth_token: "token".into(),
            path: "/Docs".to_owned(),
        };
        let change_expire = ChangePublicLinkExpireRequest {
            auth_token: "token".into(),
            link_id: 7,
            expire: Some(123),
        };
        let change_password = ChangePublicLinkPasswordRequest {
            auth_token: "token".into(),
            link_id: 7,
            password: Some("secret".into()),
        };
        let change_upload = ChangePublicLinkUploadRequest {
            auth_token: "token".into(),
            link_id: 7,
            policy: PublicLinkUploadPolicy::Everyone,
        };
        let list_upload = ListUploadLinksRequest {
            auth_token: "token".into(),
        };
        let create_upload = CreateUploadLinkRequest {
            auth_token: "token".into(),
            path: "/Docs".to_owned(),
            comment: "Upload here".to_owned(),
            expire: Some(123),
            maxspace: Some(2048),
            maxfiles: Some(5),
        };
        let delete_upload = DeleteUploadLinkRequest {
            auth_token: "token".into(),
            upload_link_id: 17,
        };
        let create_tree = CreateTreePublicLinkRequest {
            auth_token: "token".into(),
            name: "Quarterly Docs".to_owned(),
            root_folder_id: Some(9),
            folder_ids_csv: Some("9,10".to_owned()),
            file_ids_csv: Some("11,12".to_owned()),
            expire: Some(123),
            maxdownloads: Some(7),
            maxtraffic: Some(2048),
        };
        let list_access = ListPublicLinkAccessRequest {
            auth_token: "token".into(),
            link_id: 7,
        };
        let add_access = AddPublicLinkAccessRequest {
            auth_token: "token".into(),
            link_id: 7,
            email: "alice@example.com".to_owned(),
        };
        let remove_access = RemovePublicLinkAccessRequest {
            auth_token: "token".into(),
            link_id: 7,
            receiver_id: 33,
        };
        let list_bookmarks = ListBookmarksRequest {
            auth_token: "token".into(),
        };
        let remove_bookmark = RemoveBookmarkRequest {
            auth_token: "token".into(),
            code: "alpha123".to_owned(),
            location_id: 8,
        };
        let change_bookmark = ChangeBookmarkRequest {
            auth_token: "token".into(),
            code: "alpha123".to_owned(),
            location_id: 8,
            name: "Pinned Link".to_owned(),
            description: "Updated desc".to_owned(),
        };

        let encoded_list = list.encode().expect("list request should encode");
        let encoded_show = show.encode().expect("show request should encode");
        let encoded_delete = delete.encode().expect("delete request should encode");
        let encoded_create_file = create_file
            .encode()
            .expect("file create request should encode");
        let encoded_create_folder = create_folder
            .encode()
            .expect("folder create request should encode");
        let encoded_change_expire = change_expire
            .encode()
            .expect("change expire request should encode");
        let encoded_change_password = change_password
            .encode()
            .expect("change password request should encode");
        let encoded_change_upload = change_upload
            .encode()
            .expect("change upload request should encode");
        let encoded_list_upload = list_upload
            .encode()
            .expect("list upload request should encode");
        let encoded_create_upload = create_upload
            .encode()
            .expect("create upload request should encode");
        let encoded_delete_upload = delete_upload
            .encode()
            .expect("delete upload request should encode");
        let encoded_create_tree = create_tree
            .encode()
            .expect("create tree request should encode");
        let encoded_list_access = list_access
            .encode()
            .expect("list access request should encode");
        let encoded_add_access = add_access
            .encode()
            .expect("add access request should encode");
        let encoded_remove_access = remove_access
            .encode()
            .expect("remove access request should encode");
        let encoded_list_bookmarks = list_bookmarks
            .encode()
            .expect("list bookmarks request should encode");
        let encoded_remove_bookmark = remove_bookmark
            .encode()
            .expect("remove bookmark request should encode");
        let encoded_change_bookmark = change_bookmark
            .encode()
            .expect("change bookmark request should encode");

        assert_eq!(encoded_list.frame.command, "listpublinks");
        assert_eq!(encoded_list.frame.parameter_count, 3);
        assert_eq!(encoded_show.frame.command, "showpublink");
        assert_eq!(encoded_show.frame.parameter_count, 4);
        assert_eq!(encoded_delete.frame.command, "deletepublink");
        assert_eq!(encoded_delete.frame.parameter_count, 2);
        assert_eq!(encoded_create_file.frame.command, "getfilepublink");
        assert_eq!(encoded_create_file.frame.parameter_count, 2);
        assert_eq!(encoded_create_folder.frame.command, "getfolderpublink");
        assert_eq!(encoded_create_folder.frame.parameter_count, 2);
        assert_eq!(encoded_change_expire.frame.command, "changepublink");
        assert_eq!(encoded_change_expire.frame.parameter_count, 3);
        assert_eq!(encoded_change_password.frame.command, "changepublink");
        assert_eq!(encoded_change_password.frame.parameter_count, 3);
        assert_eq!(encoded_change_upload.frame.command, "changepublink");
        assert_eq!(encoded_change_upload.frame.parameter_count, 4);
        assert_eq!(encoded_list_upload.frame.command, "listuploadlinks");
        assert_eq!(encoded_list_upload.frame.parameter_count, 3);
        assert_eq!(encoded_create_upload.frame.command, "createuploadlink");
        assert_eq!(encoded_create_upload.frame.parameter_count, 6);
        assert_eq!(encoded_delete_upload.frame.command, "deleteuploadlink");
        assert_eq!(encoded_delete_upload.frame.parameter_count, 2);
        assert_eq!(encoded_create_tree.frame.command, "gettreepublink");
        assert_eq!(encoded_create_tree.frame.parameter_count, 8);
        assert_eq!(
            encoded_list_access.frame.command,
            "publink/listemailswithaccess"
        );
        assert_eq!(encoded_list_access.frame.parameter_count, 2);
        assert_eq!(encoded_add_access.frame.command, "publink/addaccess");
        assert_eq!(encoded_add_access.frame.parameter_count, 3);
        assert_eq!(encoded_remove_access.frame.command, "publink/removeaccess");
        assert_eq!(encoded_remove_access.frame.parameter_count, 3);
        assert_eq!(encoded_list_bookmarks.frame.command, "publink/listpins");
        assert_eq!(encoded_list_bookmarks.frame.parameter_count, 2);
        assert_eq!(encoded_remove_bookmark.frame.command, "publink/unpin");
        assert_eq!(encoded_remove_bookmark.frame.parameter_count, 3);
        assert_eq!(encoded_change_bookmark.frame.command, "publink/changepin");
        assert_eq!(encoded_change_bookmark.frame.parameter_count, 5);
    }
}
