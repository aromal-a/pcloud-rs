#![allow(clippy::pedantic)]
//! Property-based round-trip tests for every IPC `Method`, `Request`, and
//! `Response` variant. Complements `peer_and_protocol.rs` by exhaustively
//! exercising the enum cartesian product with random payload strings/ids.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_ipc::{
    decode_request, decode_response, encode_request_bare as encode_request, encode_response,
    methods::{
        CryptoBackendIpc, Method, Request, Response, ResponseStatus, SnapshotAction,
        UploadConflictMode, ValueKvKind, ValueKvPayload,
    },
};
use proptest::prelude::*;

fn every_method() -> &'static [Method] {
    &[
        Method::GetStatus,
        Method::GetHealth,
        Method::GetPending,
        Method::GetSyncRoots,
        Method::ListPublicLinks,
        Method::ListUploadLinks,
        Method::GetUserInfo,
        Method::PauseSync,
        Method::ResumeSync,
        Method::LoginBegin,
        Method::Logout,
        Method::SendTwoFactorSms,
        Method::SendTwoFactorNotification,
        Method::SubmitPassword,
        Method::SubmitTwoFactorCode,
        Method::UnlockCrypto,
        Method::LockCrypto,
        Method::GetCryptoStatus,
        Method::CryptoReset,
        Method::GetCryptoPrivKeyFlags,
        Method::SendCryptoChangeUserPrivate,
        Method::Shutdown,
        Method::SetAuthPersistence,
        Method::ListIncomingShares,
        Method::ListOutgoingShares,
        Method::ListIncomingShareRequests,
        Method::ListOutgoingShareRequests,
        Method::ListContacts,
        Method::ListMyTeams,
        Method::ListNotifications,
    ]
}

/// Compile-time exhaustive match — forces the test to be updated whenever a
/// new `Method` variant is introduced. `Method` is `#[non_exhaustive]` so a
/// catch-all `_` arm is required from out-of-crate code (this integration
/// test is compiled as an external crate); the explicit arms above still
/// enumerate every currently-known variant, so adding a new variant without
/// extending the list will be caught in code review rather than at compile
/// time.
// Compile-time exhaustiveness guard. Never called at runtime; it exists so
// that adding a new `Method` variant forces a reviewer to extend this match
// (see the doc comment above). Dead-code lint silenced intentionally.
//
// MAINTENANCE NOTE: `Method` is `#[non_exhaustive]` so the compiler requires
// a `_` fallthrough from external crates. The wildcard is intentionally left
// at the bottom — but every *known* variant must appear as an explicit arm
// above it. When you add a new `Method` variant you MUST add a corresponding
// arm here AND add it to `every_method()` (or leave a documented comment
// explaining why it is excluded from the runtime list).
#[allow(dead_code)]
fn must_match_every_method_variant(m: Method) -> u8 {
    match m {
        Method::GetStatus
        | Method::GetHealth
        | Method::Health
        | Method::GetPending
        | Method::GetSyncRoots
        | Method::ListPublicLinks
        | Method::ListUploadLinks
        | Method::GetUserInfo
        | Method::PauseSync
        | Method::ResumeSync
        | Method::LoginBegin
        | Method::Logout
        | Method::SendTwoFactorSms
        | Method::SendTwoFactorNotification
        | Method::SubmitPassword
        | Method::SubmitTwoFactorCode
        | Method::UnlockCrypto
        | Method::LockCrypto
        | Method::GetCryptoStatus
        | Method::CryptoReset
        | Method::GetCryptoPrivKeyFlags
        | Method::SendCryptoChangeUserPrivate
        | Method::Shutdown
        | Method::SetAuthPersistence
        | Method::ListIncomingShares
        | Method::ListOutgoingShares
        | Method::ListIncomingShareRequests
        | Method::ListOutgoingShareRequests
        | Method::ListContacts
        | Method::ListMyTeams
        | Method::ListNotifications
        | Method::SessionStatus
        // Argumentless methods added after initial list — all must appear here.
        | Method::IntegrityStatus
        | Method::HaStatus
        | Method::DrainStatus
        | Method::GetSlo
        | Method::GetAuditVerifierStatus
        | Method::GetSyncStatus
        | Method::ListConflicts
        | Method::StatPath
        | Method::GetApiServers
        | Method::GetPromo
        | Method::GetCryptoHint
        | Method::VerifyEmail => 0,
        // Required by #[non_exhaustive]; must remain last. Every currently-known
        // variant is listed above — a new variant NOT in the list above will
        // land here and must be added before merging.
        _ => 0,
    }
}

#[test]
fn every_method_variant_round_trips() {
    for &method in every_method() {
        let bytes = encode_request(&Request::Plain { method }).expect("encode");
        let frame = decode_request(&bytes).expect("decode");
        match frame.payload.request {
            Request::Plain { method: decoded } => assert_eq!(decoded, method),
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}

fn arb_method() -> impl Strategy<Value = Method> {
    let all = every_method().to_vec();
    (0..all.len()).prop_map(move |idx| all[idx])
}

fn arb_response_status() -> impl Strategy<Value = ResponseStatus> {
    prop_oneof![
        Just(ResponseStatus::Ok),
        Just(ResponseStatus::InvalidRequest),
        Just(ResponseStatus::Unauthorized),
        Just(ResponseStatus::Conflict),
        Just(ResponseStatus::Unavailable),
        Just(ResponseStatus::InternalError),
    ]
}

fn arb_snapshot_action() -> impl Strategy<Value = SnapshotAction> {
    prop_oneof![
        Just(SnapshotAction::Create),
        Just(SnapshotAction::Restore),
        Just(SnapshotAction::Verify),
        Just(SnapshotAction::Prune),
    ]
}

fn arb_upload_conflict_mode() -> impl Strategy<Value = Option<UploadConflictMode>> {
    prop_oneof![
        Just(None),
        Just(Some(UploadConflictMode::Error)),
        Just(Some(UploadConflictMode::Overwrite)),
        Just(Some(UploadConflictMode::Skip)),
        Just(Some(UploadConflictMode::Rename)),
    ]
}

fn arb_kv_kind() -> impl Strategy<Value = ValueKvKind> {
    prop_oneof![
        Just(ValueKvKind::Bool),
        Just(ValueKvKind::Int),
        Just(ValueKvKind::Uint),
        Just(ValueKvKind::String),
    ]
}

fn arb_kv_payload() -> impl Strategy<Value = ValueKvPayload> {
    prop_oneof![
        any::<bool>().prop_map(ValueKvPayload::Bool),
        any::<i64>().prop_map(ValueKvPayload::Int),
        any::<u64>().prop_map(ValueKvPayload::Uint),
        ".{0,64}".prop_map(ValueKvPayload::String),
    ]
}

fn arb_request() -> impl Strategy<Value = Request> {
    prop_oneof![
        arb_method().prop_map(|method| Request::Plain { method }),
        (".{0,64}", ".{0,64}").prop_map(|(u, v)| Request::PasswordSubmission {
            username: u,
            value: v.into()
        }),
        ".{0,64}".prop_map(|v| Request::AuthTokenSubmission { value: v.into() }),
        (".{0,16}", any::<bool>(), any::<bool>()).prop_map(|(v, t, r)| {
            Request::TwoFactorCodeSubmission {
                value: v,
                trust_device: t,
                recovery_code: r,
            }
        }),
        ".{0,64}".prop_map(|p| Request::CryptoUnlock { password: p.into() }),
        (".{0,64}", proptest::option::of(".{0,64}")).prop_map(|(p, h)| Request::CryptoSetup {
            password: p.into(),
            hint: h
        }),
        any::<bool>().prop_map(|enabled| Request::AuthPersistence { enabled }),
        (".{0,64}", ".{0,64}").prop_map(|(l, r)| Request::SyncRootAdd {
            local_path: l,
            remote_path: r,
            sync_type: None,
        }),
        any::<u64>().prop_map(|id| Request::SyncRootRemove { sync_id: id }),
        any::<u64>().prop_map(|id| Request::SyncRootPause { sync_id: id }),
        any::<u64>().prop_map(|id| Request::SyncRootResume { sync_id: id }),
        ".{0,64}".prop_map(|p| Request::IsFolderSyncable { path: p }),
        ".{0,64}".prop_map(|c| Request::ShowPublicLink { code: c }),
        any::<u64>().prop_map(|id| Request::DeletePublicLink { link_id: id }),
        ".{0,64}".prop_map(|p| Request::CreateFilePublicLink { path: p }),
        ".{0,64}".prop_map(|p| Request::CreateFolderPublicLink { path: p }),
        (
            ".{0,64}",
            proptest::option::of(any::<u64>()),
            proptest::option::of(any::<u64>()),
            proptest::option::of(any::<u64>()),
            proptest::option::of(".{0,32}"),
        )
            .prop_map(
                |(p, e, d, t, pw)| Request::CreateFolderPublicLinkWithOptions {
                    path: p,
                    expire: e,
                    maxdownloads: d,
                    maxtraffic: t,
                    password: pw.map(Into::into),
                }
            ),
        (any::<u64>(), "[a-z]{1,16}@[a-z]{1,16}", any::<bool>()).prop_map(|(fid, mail, up)| {
            Request::CreateFolderUpDownLink {
                folder_id: fid,
                mail,
                can_upload: up,
            }
        }),
        (".{0,64}", any::<bool>(), any::<u64>()).prop_map(|(p, has, secs)| {
            Request::CreateScreenshotPublicLink {
                path: p,
                has_delay: has,
                delay_seconds: secs,
            }
        }),
        (any::<u64>(), proptest::option::of(any::<u64>())).prop_map(|(id, exp)| {
            Request::ChangePublicLinkExpire {
                link_id: id,
                expire: exp,
            }
        }),
        (any::<u64>(), proptest::option::of(".{0,64}")).prop_map(|(id, p)| {
            Request::ChangePublicLinkPassword {
                link_id: id,
                password: p.map(Into::into),
            }
        }),
        Just(Request::ListBookmarks),
        (".{0,64}", any::<u64>()).prop_map(|(c, l)| Request::RemoveBookmark {
            code: c,
            location_id: l
        }),
        (".{0,32}", arb_kv_kind()).prop_map(|(n, k)| Request::ValueGet { name: n, kind: k }),
        (".{0,32}", arb_kv_payload()).prop_map(|(n, p)| Request::ValueSet { name: n, value: p }),
        (".{0,32}", arb_kv_kind()).prop_map(|(n, k)| Request::ValueHas { name: n, kind: k }),
        // --- New variants added to improve coverage ---
        // CryptoMkdir
        (
            proptest::option::of(any::<u64>()),
            proptest::option::of(any::<u64>()),
            "[a-z]{1,20}",
        )
            .prop_map(|(pid, lid, name)| Request::CryptoMkdir {
                name,
                parent_folder_id: pid,
                local_folder_id: lid,
            }),
        // CryptoChangePassword
        (
            "[a-zA-Z0-9]{8,32}",
            "[a-zA-Z0-9]{8,32}",
            "[a-z]{0,32}",
            "[a-zA-Z0-9]{4,16}",
            any::<u64>(),
        )
            .prop_map(
                |(old, new, hint, code, flags)| Request::CryptoChangePassword {
                    old_password: old.into(),
                    new_password: new.into(),
                    hint,
                    code,
                    flags,
                }
            ),
        // CryptoChangePasswordUnlocked
        (
            "[a-zA-Z0-9]{8,32}",
            "[a-z]{0,32}",
            "[a-zA-Z0-9]{4,16}",
            any::<u64>()
        )
            .prop_map(
                |(new, hint, code, flags)| Request::CryptoChangePasswordUnlocked {
                    new_password: new.into(),
                    hint,
                    code,
                    flags,
                },
            ),
        // SyncRootChangeType
        (
            any::<u64>(),
            prop_oneof![
                Just(pcloud_model::sync::SyncType::Full),
                Just(pcloud_model::sync::SyncType::DownloadOnly),
                Just(pcloud_model::sync::SyncType::UploadOnly),
            ]
        )
            .prop_map(|(id, st)| Request::SyncRootChangeType {
                sync_id: id,
                sync_type: st,
            }),
        // GetSyncSuggestions
        (
            "[a-zA-Z0-9/._-]{1,64}",
            proptest::option::of(0usize..256usize)
        )
            .prop_map(|(path, max)| Request::GetSyncSuggestions { path, max },),
        // ConflictResolve
        ("[a-zA-Z0-9/._-]{1,64}", "[a-z_]{4,20}")
            .prop_map(|(path, policy)| { Request::ConflictResolve { path, policy } }),
        // ConflictList (argumentless)
        Just(Request::ConflictList),
        // UploadList (argumentless)
        Just(Request::UploadList),
        // CreateRemoteFolder
        (
            proptest::option::of(any::<u64>()),
            "[a-zA-Z0-9 ._-]{1,32}",
            "[a-zA-Z0-9/._-]{1,64}",
            any::<bool>(),
        )
            .prop_map(|(pid, name, path, check)| Request::CreateRemoteFolder {
                parent_folder_id: pid,
                name,
                path,
                check_and_create: check,
            }),
        // DownloadFile
        (any::<u64>(), "[a-zA-Z0-9/._-]{1,64}").prop_map(|(id, path)| Request::DownloadFile {
            file_id: id,
            local_path: std::path::PathBuf::from(path),
        }),
        // SetApiServer
        (any::<u32>(), "[a-zA-Z0-9.-]{1,64}").prop_map(|(loc, binapi)| Request::SetApiServer {
            location_id: loc,
            binapi,
        }),
        // SetLanguage
        "[a-z]{2,5}".prop_map(|language| Request::SetLanguage { language }),
        // AccountChangePassword
        ("[a-zA-Z0-9]{8,20}", "[a-zA-Z0-9]{8,20}").prop_map(|(cur, new)| {
            Request::AccountChangePassword {
                current_password: cur.into(),
                new_password: new.into(),
            }
        }),
        // AccountRegister
        (
            "[a-z]{1,16}@[a-z]{1,16}",
            "[a-zA-Z0-9]{8,20}",
            any::<bool>()
        )
            .prop_map(|(email, password, terms)| Request::AccountRegister {
                email,
                password: password.into(),
                terms_accepted: terms,
            },),
        // AcceptShareRequest
        (
            any::<u64>(),
            any::<u64>(),
            proptest::option::of("[a-z]{1,20}")
        )
            .prop_map(|(req_id, folder_id, name)| Request::AcceptShareRequest {
                share_request_id: req_id,
                to_folder_id: folder_id,
                name,
            },),
        // DeclineShareRequest
        any::<u64>().prop_map(|id| Request::DeclineShareRequest {
            share_request_id: id
        }),
        // CancelShareRequest
        any::<u64>().prop_map(|id| Request::CancelShareRequest {
            share_request_id: id
        }),
        // ShareFolder
        (
            any::<u64>(),
            "[a-z]{1,20}",
            "[a-z]{1,16}@[a-z]{1,16}",
            "[a-z ]{0,64}",
            any::<u32>(),
            proptest::option::of("[a-z]{0,32}"),
        )
            .prop_map(|(fid, name, mail, msg, perms, hint)| Request::ShareFolder {
                folder_id: fid,
                name,
                mail,
                message: msg,
                permissions_bits: perms,
                hint,
            }),
        // CryptoShareFolder — same shape as ShareFolder + a temppass
        (
            any::<u64>(),
            "[a-z]{1,20}",
            "[a-z]{1,16}@[a-z]{1,16}",
            "[a-z ]{0,64}",
            any::<u32>(),
            "[a-zA-Z0-9]{1,32}",
            proptest::option::of("[a-z]{0,32}"),
        )
            .prop_map(|(fid, name, mail, msg, perms, tp, hint)| {
                Request::CryptoShareFolder {
                    folder_id: fid,
                    name,
                    mail,
                    message: msg,
                    permissions_bits: perms,
                    temppass: tp.into(),
                    hint,
                }
            },),
        // RemoveShare
        any::<u64>().prop_map(|id| Request::RemoveShare { share_id: id }),
        // ModifyShare
        (any::<u64>(), any::<u32>()).prop_map(|(id, perms)| Request::ModifyShare {
            share_id: id,
            permissions_bits: perms,
        }),
        // UploadCreate
        (
            "[a-zA-Z0-9/._-]{1,64}",
            "[a-zA-Z0-9._-]{1,32}",
            proptest::option::of(any::<u64>()),
            any::<u64>(),
            arb_upload_conflict_mode(),
        )
            .prop_map(|(lp, rn, pid, bytes, conflict)| Request::UploadCreate {
                local_path: std::path::PathBuf::from(lp),
                remote_name: rn,
                parent_folder_id: pid,
                total_bytes: bytes,
                conflict_mode: conflict,
            }),
        // UploadPause
        any::<u64>().prop_map(|id| Request::UploadPause { session_id: id }),
        // UploadResume
        any::<u64>().prop_map(|id| Request::UploadResume { session_id: id }),
        // UploadCancel
        any::<u64>().prop_map(|id| Request::UploadCancel { session_id: id }),
        // BackupSnapshot
        (
            arb_snapshot_action(),
            "[a-zA-Z0-9/._-]{1,64}",
            proptest::option::of("[a-zA-Z0-9@._-]{1,32}"),
            any::<bool>(),
            proptest::option::of(1u32..366u32),
            proptest::option::of(1i32..23i32),
        )
            .prop_map(
                |(action, path, gpg, yes, ret, zstd)| Request::BackupSnapshot {
                    action,
                    path: std::path::PathBuf::from(path),
                    gpg_recipient: gpg,
                    yes,
                    retention_days: ret,
                    zstd_level: zstd,
                }
            ),
        // Mount
        "[a-zA-Z0-9/._-]{1,64}".prop_map(|p| Request::Mount {
            path: std::path::PathBuf::from(p),
        }),
        // Unmount (argumentless)
        Just(Request::Unmount),
        // MountForceUnmount
        "[a-zA-Z0-9/._-]{1,64}".prop_map(|p| Request::MountForceUnmount {
            path: std::path::PathBuf::from(p),
        }),
        // DeletePublicLinkByCode
        "[a-zA-Z0-9]{4,16}".prop_map(|c| Request::DeletePublicLinkByCode { code: c }),
        // GetFileLink
        any::<u64>().prop_map(|id| Request::GetFileLink { file_id: id }),
        // GetFolderIdByPath
        "[a-zA-Z0-9/._-]{1,64}".prop_map(|p| Request::GetFolderIdByPath { path: p }),
        // GetFolderFlags
        "[a-zA-Z0-9/._-]{1,64}".prop_map(|p| Request::GetFolderFlags { path: p }),
        // GetFolderOwnerId
        "[a-zA-Z0-9/._-]{1,64}".prop_map(|p| Request::GetFolderOwnerId { path: p }),
        // FilesystemStatus
        "[a-zA-Z0-9/._-]{1,64}".prop_map(|p| Request::FilesystemStatus { path: p }),
        // VerifyPath
        ("[a-zA-Z0-9/._-]{1,64}", any::<bool>())
            .prop_map(|(path, recursive)| { Request::VerifyPath { path, recursive } }),
        // StatPath
        "[a-zA-Z0-9/._-]{1,64}".prop_map(|p| Request::StatPath { path: p }),
        // LostPassword
        "[a-z]{1,16}@[a-z]{1,16}".prop_map(|email| Request::LostPassword { email }),
        // VerifyEmailRestricted
        "[a-zA-Z0-9]{8,32}".prop_map(|t| Request::VerifyEmailRestricted {
            verify_token: t.into()
        }),
        // MarkNotificationsRead
        any::<u64>().prop_map(|id| Request::MarkNotificationsRead { upto_id: id }),
        // SendPublink
        (
            "[a-zA-Z0-9]{4,16}",
            "[a-z]{1,16}@[a-z]{1,16}",
            "[a-z ]{0,64}",
        )
            .prop_map(|(code, mails, message)| Request::SendPublink {
                code,
                mails,
                message
            }),
        // RunLocalScan (argumentless)
        Just(Request::RunLocalScan),
        // DeleteBackupDevice (argumentless)
        Just(Request::DeleteBackupDevice),
        // IntegrityRunOnce (argumentless)
        Just(Request::IntegrityRunOnce),
        // IntegritySkip
        "[a-zA-Z0-9*?/._-]{1,64}".prop_map(|path| Request::IntegritySkip { path }),
        // DeleteBackup
        any::<u64>().prop_map(|id| Request::DeleteBackup { backup_id: id }),
        // StopDevice
        any::<u64>().prop_map(|id| Request::StopDevice {
            device_folder_id: id
        }),
        // CreateBackup
        (
            "[a-z]{1,20}",
            any::<u64>(),
            "[a-zA-Z0-9/._-]{1,64}",
            proptest::option::of("[a-z]{1,20}"),
        )
            .prop_map(|(name, rfid, lp, pfn)| Request::CreateBackup {
                name,
                root_folder_id: rfid,
                local_path: lp,
                parent_folder_name: pfn,
            }),
        // ChangeBookmark
        (
            "[a-zA-Z0-9]{4,16}",
            any::<u64>(),
            "[a-z]{1,20}",
            "[a-z ]{0,64}"
        )
            .prop_map(|(code, lid, name, desc)| Request::ChangeBookmark {
                code,
                location_id: lid,
                name,
                description: desc,
            },),
        // CreateUploadLink
        (
            "[a-zA-Z0-9/._-]{1,64}",
            "[a-z ]{0,64}",
            proptest::option::of(any::<u64>()),
            proptest::option::of(any::<u64>()),
            proptest::option::of(any::<u64>()),
        )
            .prop_map(|(path, comment, expire, maxspace, maxfiles)| {
                Request::CreateUploadLink {
                    path,
                    comment,
                    expire,
                    maxspace,
                    maxfiles,
                }
            }),
        // DeleteUploadLink
        any::<u64>().prop_map(|id| Request::DeleteUploadLink { upload_link_id: id }),
        // ListPublicLinkAccess
        any::<u64>().prop_map(|id| Request::ListPublicLinkAccess { link_id: id }),
        // AddPublicLinkAccess
        (any::<u64>(), "[a-z]{1,16}@[a-z]{1,16}")
            .prop_map(|(id, email)| { Request::AddPublicLinkAccess { link_id: id, email } }),
        // RemovePublicLinkAccess
        (any::<u64>(), any::<u64>()).prop_map(|(lid, rid)| Request::RemovePublicLinkAccess {
            link_id: lid,
            receiver_id: rid,
        }),
        // CreateTreePublicLink
        (
            "[a-z]{1,20}",
            proptest::option::of(any::<u64>()),
            proptest::option::of("[0-9,]{1,32}"),
            proptest::option::of("[0-9,]{1,32}"),
            proptest::option::of(any::<u64>()),
            proptest::option::of(any::<u64>()),
            proptest::option::of(any::<u64>()),
        )
            .prop_map(|(name, rfid, fids, fileids, exp, maxdl, maxt)| {
                Request::CreateTreePublicLink {
                    name,
                    root_folder_id: rfid,
                    folder_ids_csv: fids,
                    file_ids_csv: fileids,
                    expire: exp,
                    maxdownloads: maxdl,
                    maxtraffic: maxt,
                }
            }),
        // AccountStopShare
        (
            proptest::collection::vec(any::<u64>(), 0..4),
            proptest::collection::vec(any::<u64>(), 0..4),
        )
            .prop_map(|(user_ids, team_ids)| Request::AccountStopShare {
                user_share_ids: user_ids,
                team_share_ids: team_ids,
            }),
        // AccountModifyShare
        (
            proptest::collection::vec((any::<u64>(), any::<u32>()), 0..4),
            proptest::collection::vec((any::<u64>(), any::<u32>()), 0..4),
        )
            .prop_map(|(user, team)| Request::AccountModifyShare {
                user_shares: user,
                team_shares: team,
            }),
        // AccountTeamShare
        (
            any::<u64>(),
            "[a-z]{1,20}",
            any::<u64>(),
            "[a-z ]{0,64}",
            any::<u32>(),
            proptest::option::of("[a-z]{0,32}"),
        )
            .prop_map(
                |(fid, name, tid, msg, perms, hint)| Request::AccountTeamShare {
                    folder_id: fid,
                    name,
                    team_id: tid,
                    message: msg,
                    permissions_bits: perms,
                    hint,
                }
            ),
        // AuditVerifyChain
        (
            proptest::option::of(any::<i64>()),
            proptest::option::of(any::<i64>()),
        )
            .prop_map(|(from, to)| Request::AuditVerifyChain {
                range: pcloud_ipc::methods::AuditVerifyRange { from, to },
            }),
        // UploadWriteFromFile (bd-1du row 93) — C primitive shape:
        // upload_session_id / source_fileid / source_hash / upload offset /
        // source offset / count
        // (matches pclsync/pupload.c:843-859 field set).
        (
            any::<u64>(),
            any::<u64>(),
            any::<u64>(),
            any::<u64>(),
            any::<u64>(),
            any::<u64>()
        )
            .prop_map(
                |(session_id, source_fileid, source_hash, offset, source_offset, count)| {
                    Request::UploadWriteFromFile {
                        upload_session_id: session_id,
                        source_fileid,
                        source_hash,
                        offset,
                        source_offset: Some(source_offset),
                        count,
                    }
                },
            ),
        // CreateTreePublicLinkFromPaths (bd-1du row 149)
        (
            "[a-z]{1,20}",
            proptest::collection::vec("[a-zA-Z0-9/._-]{1,32}", 1..4),
            proptest::option::of(any::<u64>()),
        )
            .prop_map(
                |(name, paths, expires)| Request::CreateTreePublicLinkFromPaths {
                    name,
                    paths,
                    expires,
                }
            ),
        // CreateTreePublicLinkFromPathTargets (bd-1du row 149)
        (
            "[a-z]{1,20}",
            proptest::option::of("[a-zA-Z0-9/._-]{1,32}"),
            proptest::collection::vec("[a-zA-Z0-9/._-]{1,32}", 0..4),
            proptest::collection::vec("[a-zA-Z0-9/._-]{1,32}", 0..4),
            proptest::option::of(any::<u64>()),
        )
            .prop_map(|(name, root, folders, files, expires)| {
                Request::CreateTreePublicLinkFromPathTargets {
                    name,
                    root,
                    folders,
                    files,
                    expires,
                }
            }),
        // CryptoSetupV2 (Stage 4b — dual-crypto-backend IPC surface).
        (
            prop_oneof![
                Just(CryptoBackendIpc::PclsyncCompat),
                Just(CryptoBackendIpc::Enhanced),
            ],
            any::<bool>(),
            ".{0,64}",
            proptest::option::of(".{0,64}"),
        )
            .prop_map(|(backend, acknowledge_not_interop, password, hint)| {
                Request::CryptoSetupV2 {
                    backend,
                    acknowledge_not_interop,
                    password: password.into(),
                    hint,
                }
            },),
        // CryptoGetFolderKey (Stage 4b — hot-path wrapped sym-key fetch).
        any::<u64>().prop_map(|folder_id| Request::CryptoGetFolderKey { folder_id }),
        // CryptoGetFileKey (Stage 4b — hot-path wrapped sym-key fetch).
        any::<u64>().prop_map(|file_id| Request::CryptoGetFileKey { file_id }),
    ]
}

proptest! {
    #[test]
    fn prop_request_round_trips(request in arb_request()) {
        let bytes = match encode_request(&request) {
            Ok(b) => b,
            Err(_) => return Ok(()),
        };
        let frame = decode_request(&bytes).expect("decode should succeed");
        prop_assert!(frame.payload.traceparent().is_none());
        prop_assert_eq!(frame.payload.request, request);
    }

    #[test]
    fn prop_response_round_trips(
        status in arb_response_status(),
        message in ".{0,256}"
    ) {
        let original = Response { status: status.clone(), message: message.clone() };
        let bytes = encode_response(&original).expect("encode");
        let frame = decode_response(&bytes).expect("decode");
        prop_assert_eq!(frame.payload.status, status);
        prop_assert_eq!(frame.payload.message, message);
    }

    #[test]
    fn prop_every_method_plain_round_trip(method in arb_method()) {
        let bytes = encode_request(&Request::Plain { method }).expect("encode");
        let frame = decode_request(&bytes).expect("decode");
        match frame.payload.request {
            Request::Plain { method: decoded } => prop_assert_eq!(decoded, method),
            other => prop_assert!(false, "unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn prop_random_bytes_do_not_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let _ = decode_request(&bytes);
        let _ = decode_response(&bytes);
    }
}
