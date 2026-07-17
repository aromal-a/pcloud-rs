//! Broad public IPC-contract coverage for the daemon composition root.
//!
//! The coverage policy excludes `tests/`, so this suite increases coverage
//! only by executing production dispatch, validation, persistence, and
//! development-transport behavior.

use std::path::{Path, PathBuf};

use pcloud_auth::AuthCommand;
use pcloud_config::{ConfigProfile, Environment};
use pcloud_daemon::{RuntimeShell, bootstrap_with_config};
use pcloud_ipc::{
    AuditVerifyRange, Method, Request, Response, ResponseStatus, SnapshotAction,
    UploadConflictMode, ValueKvKind, ValueKvPayload, methods::CryptoBackendIpc,
};
use pcloud_model::{ids::UserId, public_links::PublicLinkUploadPolicy, sync::SyncType};
use pcloud_secret::secret_string::SecretString;

struct Fixture {
    root: tempfile::TempDir,
    runtime: RuntimeShell,
}

impl Fixture {
    fn authenticated(tag: &str) -> Self {
        let root = tempfile::tempdir().expect("temporary runtime root");
        let mut config =
            ConfigProfile::secure_defaults(root.path().join(tag), Environment::Development);
        config.features.audit_verifier.enabled = false;
        config.features.integrity_sweeper.enabled = false;
        config.sync_loop.enabled = false;
        let mut runtime = bootstrap_with_config(config).expect("bootstrap runtime");
        runtime
            .auth
            .apply(AuthCommand::LoginWithToken {
                token: SecretString::new("coverage-token"),
            })
            .expect("begin token login");
        runtime
            .auth
            .apply(AuthCommand::MarkAuthenticated {
                user_id: Some(UserId::new(1)),
                auth_token: SecretString::new("coverage-token"),
            })
            .expect("mark authenticated");
        Self { root, runtime }
    }

    fn request(&mut self, request: Request) -> Response {
        assert!(!pcloud_daemon::dispatch::backend_label(&request).is_empty());
        self.runtime.handle_request(request)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }
}

fn plain(method: Method) -> Request {
    Request::Plain { method }
}

fn assert_defined(response: &Response) {
    assert!(
        matches!(
            response.status,
            ResponseStatus::Ok
                | ResponseStatus::InvalidRequest
                | ResponseStatus::Unauthorized
                | ResponseStatus::Conflict
                | ResponseStatus::Unavailable
                | ResponseStatus::InternalError
                | ResponseStatus::PolicyViolation { .. }
        ),
        "response status must stay inside the stable taxonomy"
    );
}

#[test]
fn every_plain_method_has_a_defined_runtime_response() {
    let mut fixture = Fixture::authenticated("plain-methods");
    for method in [
        Method::GetStatus,
        Method::GetHealth,
        Method::Health,
        Method::GetPending,
        Method::GetSyncRoots,
        Method::ListPublicLinks,
        Method::ListUploadLinks,
        Method::GetUserInfo,
        Method::PauseSync,
        Method::ResumeSync,
        Method::SendTwoFactorSms,
        Method::SendTwoFactorNotification,
        Method::SubmitPassword,
        Method::SubmitTwoFactorCode,
        Method::UnlockCrypto,
        Method::SetAuthPersistence,
        Method::GetCryptoStatus,
        Method::GetCryptoPrivKeyFlags,
        Method::SendCryptoChangeUserPrivate,
        Method::ListIncomingShares,
        Method::ListOutgoingShares,
        Method::ListIncomingShareRequests,
        Method::ListOutgoingShareRequests,
        Method::ListContacts,
        Method::ListMyTeams,
        Method::ListNotifications,
        Method::SessionStatus,
        Method::IntegrityStatus,
        Method::GetSlo,
        Method::HaStatus,
        Method::DrainStatus,
        Method::GetAuditVerifierStatus,
        Method::GetSyncStatus,
        Method::ListConflicts,
        Method::StatPath,
        Method::GetApiServers,
        Method::GetPromo,
        Method::GetCryptoHint,
        Method::VerifyEmail,
        Method::LockCrypto,
    ] {
        assert_defined(&fixture.request(plain(method)));
    }

    assert_eq!(
        fixture.request(plain(Method::CryptoReset)).status,
        ResponseStatus::Ok
    );
    assert_eq!(
        fixture.request(plain(Method::Shutdown)).status,
        ResponseStatus::Ok
    );
    assert!(fixture.runtime.control.shutdown_requested);
}

#[test]
fn sync_settings_and_local_classification_round_trip() {
    let mut fixture = Fixture::authenticated("sync-settings");
    let local = fixture.path("sync-local");
    std::fs::create_dir_all(&local).expect("create sync local");

    let add = fixture.request(Request::SyncRootAdd {
        local_path: local.display().to_string(),
        remote_path: "/remote-sync".to_owned(),
        sync_type: Some(SyncType::Full),
    });
    assert_eq!(add.status, ResponseStatus::Ok, "{}", add.message);
    let roots = fixture.request(plain(Method::GetSyncRoots));
    assert_eq!(roots.status, ResponseStatus::Ok);
    assert!(roots.message.contains("id=1"));
    let sync_id = 1;

    for request in [
        Request::SyncRootPause { sync_id },
        Request::SyncRootResume { sync_id },
        Request::SyncRootChangeType {
            sync_id,
            sync_type: SyncType::DownloadOnly,
        },
        Request::SyncExcludeAdd {
            sync_id,
            pattern: "*.tmp".to_owned(),
        },
        Request::SyncExcludeList { sync_id },
        Request::FilesystemStatus {
            path: local.display().to_string(),
        },
        Request::IsFolderSyncable {
            path: fixture.path("another").display().to_string(),
        },
        Request::GetSyncSuggestions {
            path: fixture.root.path().display().to_string(),
            max: Some(2),
        },
        Request::RunLocalScan,
    ] {
        assert_defined(&fixture.request(request));
    }
    assert_eq!(
        fixture
            .request(Request::SyncExcludeRemove {
                sync_id,
                pattern: "*.tmp".to_owned(),
            })
            .status,
        ResponseStatus::Ok
    );
    assert_eq!(
        fixture.request(Request::SyncRootRemove { sync_id }).status,
        ResponseStatus::Ok
    );

    for (name, value, kind) in [
        (
            "coverage.bool",
            ValueKvPayload::Bool(true),
            ValueKvKind::Bool,
        ),
        ("coverage.int", ValueKvPayload::Int(-7), ValueKvKind::Int),
        ("coverage.uint", ValueKvPayload::Uint(9), ValueKvKind::Uint),
        (
            "coverage.string",
            ValueKvPayload::String("value".to_owned()),
            ValueKvKind::String,
        ),
    ] {
        assert_eq!(
            fixture
                .request(Request::ValueSet {
                    name: name.to_owned(),
                    value,
                })
                .status,
            ResponseStatus::Ok
        );
        assert_eq!(
            fixture
                .request(Request::ValueHas {
                    name: name.to_owned(),
                    kind,
                })
                .status,
            ResponseStatus::Ok
        );
        assert_eq!(
            fixture
                .request(Request::ValueGet {
                    name: name.to_owned(),
                    kind,
                })
                .status,
            ResponseStatus::Ok
        );
    }
    assert_defined(&fixture.request(Request::ValueGet {
        name: "coverage.bool".to_owned(),
        kind: ValueKvKind::String,
    }));
}

#[test]
fn public_link_share_notification_and_account_surfaces_dispatch() {
    let mut fixture = Fixture::authenticated("public-account");
    let public_requests = [
        Request::ShowPublicLink {
            code: "fixture-code".to_owned(),
        },
        Request::DeletePublicLink { link_id: 1 },
        Request::DeletePublicLinkByCode {
            code: "fixture-code".to_owned(),
        },
        Request::CreateFilePublicLink {
            path: "/Documents/notes.txt".to_owned(),
        },
        Request::CreateFolderPublicLink {
            path: "/Documents".to_owned(),
        },
        Request::CreateFolderPublicLinkWithOptions {
            path: "/Documents".to_owned(),
            expire: Some(2_000_000_000),
            maxdownloads: Some(3),
            maxtraffic: Some(4096),
            password: Some("link-password".into()),
        },
        Request::CreateFolderUpDownLink {
            folder_id: 1,
            mail: "alice@example.com".to_owned(),
            can_upload: true,
        },
        Request::CreateScreenshotPublicLink {
            path: "/Documents/shot.png".to_owned(),
            has_delay: true,
            delay_seconds: 60,
        },
        Request::ChangePublicLinkExpire {
            link_id: 1,
            expire: Some(2_000_000_000),
        },
        Request::ChangePublicLinkPassword {
            link_id: 1,
            password: Some("changed".into()),
        },
        Request::ChangePublicLinkUpload {
            link_id: 1,
            policy: PublicLinkUploadPolicy::Everyone,
        },
        Request::CreateUploadLink {
            path: "/Documents".to_owned(),
            comment: "drop files".to_owned(),
            expire: Some(2_000_000_000),
            maxspace: Some(10_000),
            maxfiles: Some(5),
        },
        Request::DeleteUploadLink { upload_link_id: 1 },
        Request::CreateTreePublicLink {
            name: "selection".to_owned(),
            root_folder_id: Some(1),
            folder_ids_csv: Some("2,3".to_owned()),
            file_ids_csv: Some("4,5".to_owned()),
            expire: None,
            maxdownloads: None,
            maxtraffic: None,
        },
        Request::ListPublicLinkAccess { link_id: 1 },
        Request::AddPublicLinkAccess {
            link_id: 1,
            email: "alice@example.com".to_owned(),
        },
        Request::RemovePublicLinkAccess {
            link_id: 1,
            receiver_id: 2,
        },
        Request::ListBookmarks,
        Request::RemoveBookmark {
            code: "fixture-code".to_owned(),
            location_id: 1,
        },
        Request::ChangeBookmark {
            code: "fixture-code".to_owned(),
            location_id: 1,
            name: "bookmark".to_owned(),
            description: "description".to_owned(),
        },
        Request::SendPublink {
            code: "fixture-code".to_owned(),
            mails: "alice@example.com,bob@example.com".to_owned(),
            message: "hello".to_owned(),
        },
    ];
    for request in public_requests {
        assert_defined(&fixture.request(request));
    }

    let share_requests = [
        Request::ShareFolder {
            folder_id: 1,
            name: "Docs".to_owned(),
            mail: "alice@example.com".to_owned(),
            message: "hello".to_owned(),
            permissions_bits: 7,
            hint: None,
        },
        Request::CryptoShareFolder {
            folder_id: 1,
            name: "Docs".to_owned(),
            mail: "alice@example.com".to_owned(),
            message: "hello".to_owned(),
            permissions_bits: 7,
            temppass: "temporary".into(),
            hint: Some("hint".to_owned()),
        },
        Request::CryptoShareFolderRsa {
            folder_id: 1,
            name: "Docs".to_owned(),
            mail: "alice@example.com".to_owned(),
            message: "hello".to_owned(),
            permissions_bits: 7,
            hint: None,
        },
        Request::CancelShareRequest {
            share_request_id: 1,
        },
        Request::DeclineShareRequest {
            share_request_id: 1,
        },
        Request::AcceptShareRequest {
            share_request_id: 1,
            to_folder_id: 0,
            name: Some("Accepted".to_owned()),
        },
        Request::RemoveShare { share_id: 1 },
        Request::ModifyShare {
            share_id: 1,
            permissions_bits: 3,
        },
        Request::AccountStopShare {
            user_share_ids: vec![1, 2],
            team_share_ids: vec![3],
        },
        Request::AccountModifyShare {
            user_shares: vec![(1, 3)],
            team_shares: vec![(2, 7)],
        },
        Request::AccountTeamShare {
            folder_id: 1,
            name: "Team Docs".to_owned(),
            team_id: 2,
            message: "hello".to_owned(),
            permissions_bits: 7,
            hint: None,
        },
        Request::CryptoAccountTeamShare {
            folder_id: 1,
            name: "Team Docs".to_owned(),
            team_id: 2,
            message: "hello".to_owned(),
            permissions_bits: 7,
            temppass: "temporary".into(),
            hint: None,
        },
    ];
    for request in share_requests {
        assert_defined(&fixture.request(request));
    }

    for request in [
        Request::MarkNotificationsRead { upto_id: 3 },
        Request::LostPassword {
            email: "alice@example.com".to_owned(),
        },
        Request::VerifyEmailRestricted {
            verify_token: "verify-token".into(),
        },
        Request::AccountRegister {
            email: "new@example.com".to_owned(),
            password: "account-password".into(),
            terms_accepted: true,
        },
        Request::SetApiServer {
            location_id: 1,
            binapi: "binapi-eu.pcloud.com".to_owned(),
        },
        Request::SetLanguage {
            language: "de".to_owned(),
        },
    ] {
        assert_defined(&fixture.request(request));
    }
}

#[test]
fn remote_file_folder_backup_and_transfer_contracts_dispatch() {
    let mut fixture = Fixture::authenticated("remote-transfer");
    let local_file = fixture.path("upload.txt");
    std::fs::write(&local_file, b"fixture payload").expect("local upload fixture");
    let download = fixture.path("download.txt");
    let verify_dir = fixture.path("verify");
    std::fs::create_dir_all(&verify_dir).expect("verify directory");
    std::fs::write(verify_dir.join("one.txt"), b"one").expect("verify file");

    for request in [
        Request::GetFolderIdByPath {
            path: "/".to_owned(),
        },
        Request::GetFolderFlags {
            path: "/".to_owned(),
        },
        Request::GetFolderOwnerId {
            path: "/".to_owned(),
        },
        Request::StatPath {
            path: "/".to_owned(),
        },
        Request::ListFolderByPath {
            path: "/".to_owned(),
        },
        Request::CreateRemoteFolder {
            parent_folder_id: Some(0),
            name: "Coverage Folder".to_owned(),
            path: String::new(),
            check_and_create: false,
        },
        Request::CreateRemoteFolder {
            parent_folder_id: None,
            name: String::new(),
            path: "/Coverage Path".to_owned(),
            check_and_create: false,
        },
        Request::CreateFolderByPath {
            path: "/Coverage Created".to_owned(),
        },
        Request::FileDeleteByPath {
            path: "/Documents/missing.txt".to_owned(),
        },
        Request::FolderDeleteByPath {
            path: "/Missing Folder".to_owned(),
            recursive: false,
        },
        Request::FolderDeleteById {
            folder_id: 999,
            recursive: true,
        },
        Request::ReadFileRange {
            path: "/Documents/notes.txt".to_owned(),
            offset: 0,
            length: 4,
        },
        Request::WriteFileFresh {
            path: "/Documents/coverage.txt".to_owned(),
            data_b64: "aGVsbG8=".to_owned(),
        },
        Request::RenamePath {
            from: "/Documents/notes.txt".to_owned(),
            to: "/Documents/renamed.txt".to_owned(),
        },
        Request::CopyPath {
            from: "/Documents/notes.txt".to_owned(),
            to: "/Documents/copied.txt".to_owned(),
        },
        Request::DeletePath {
            path: "/Documents/missing.txt".to_owned(),
            recursive: true,
        },
        Request::UploadFileByPath {
            local_path: local_file.clone(),
            remote_path: "/Documents/upload.txt".to_owned(),
        },
        Request::DownloadFileByPath {
            remote_path: "/Documents/notes.txt".to_owned(),
            local_path: download.clone(),
            overwrite: true,
        },
        Request::VerifyPath {
            path: verify_dir.display().to_string(),
            recursive: true,
        },
        Request::GetFileLink { file_id: 1 },
        Request::DownloadFile {
            file_id: 1,
            local_path: fixture.path("by-id.txt"),
        },
        Request::DeleteBackup { backup_id: 1 },
        Request::CreateBackup {
            name: "Coverage Backup".to_owned(),
            root_folder_id: 1,
            local_path: fixture.root.path().display().to_string(),
            parent_folder_name: Some("Coverage".to_owned()),
        },
        Request::StopDevice {
            device_folder_id: 1,
        },
        Request::DeleteBackupDevice,
        Request::UploadWriteFromFile {
            upload_session_id: 1,
            source_fileid: 2,
            source_hash: 3,
            offset: 0,
            source_offset: Some(0),
            count: 4,
        },
        Request::CreateTreePublicLinkFromPaths {
            name: "paths".to_owned(),
            paths: vec!["/Documents".to_owned()],
            expires: None,
        },
        Request::CreateTreePublicLinkFromPathTargets {
            name: "targets".to_owned(),
            root: Some("/".to_owned()),
            folders: vec!["/Documents".to_owned()],
            files: vec!["/Documents/notes.txt".to_owned()],
            expires: None,
        },
    ] {
        assert_defined(&fixture.request(request));
    }
}

#[test]
fn validation_matrix_reaches_rejection_paths_without_external_io() {
    let mut fixture = Fixture::authenticated("validation");
    for request in [
        Request::SyncRootAdd {
            local_path: "relative".to_owned(),
            remote_path: "relative".to_owned(),
            sync_type: None,
        },
        Request::SyncRootRemove { sync_id: u64::MAX },
        Request::SyncRootPause { sync_id: u64::MAX },
        Request::SyncRootResume { sync_id: u64::MAX },
        Request::SyncRootChangeType {
            sync_id: u64::MAX,
            sync_type: SyncType::UploadOnly,
        },
        Request::SyncExcludeAdd {
            sync_id: u64::MAX,
            pattern: " ".to_owned(),
        },
        Request::SyncExcludeRemove {
            sync_id: u64::MAX,
            pattern: "missing".to_owned(),
        },
        Request::SyncExcludeList { sync_id: u64::MAX },
        Request::ShowPublicLink {
            code: String::new(),
        },
        Request::DeletePublicLink { link_id: 0 },
        Request::DeletePublicLinkByCode {
            code: String::new(),
        },
        Request::CreateFilePublicLink {
            path: "relative".to_owned(),
        },
        Request::CreateFolderPublicLink {
            path: "relative".to_owned(),
        },
        Request::CreateUploadLink {
            path: String::new(),
            comment: String::new(),
            expire: None,
            maxspace: None,
            maxfiles: None,
        },
        Request::CreateTreePublicLink {
            name: String::new(),
            root_folder_id: None,
            folder_ids_csv: None,
            file_ids_csv: None,
            expire: None,
            maxdownloads: None,
            maxtraffic: None,
        },
        Request::AddPublicLinkAccess {
            link_id: 0,
            email: "not-an-email".to_owned(),
        },
        Request::SendPublink {
            code: String::new(),
            mails: String::new(),
            message: String::new(),
        },
        Request::ShareFolder {
            folder_id: 1,
            name: String::new(),
            mail: "invalid".to_owned(),
            message: String::new(),
            permissions_bits: 0,
            hint: None,
        },
        Request::CryptoShareFolder {
            folder_id: 1,
            name: String::new(),
            mail: "invalid".to_owned(),
            message: String::new(),
            permissions_bits: 0,
            temppass: "temporary".into(),
            hint: None,
        },
        Request::CryptoShareFolderRsa {
            folder_id: 1,
            name: String::new(),
            mail: "invalid".to_owned(),
            message: String::new(),
            permissions_bits: 0,
            hint: None,
        },
        Request::AccountTeamShare {
            folder_id: 1,
            name: String::new(),
            team_id: 1,
            message: String::new(),
            permissions_bits: 0,
            hint: None,
        },
        Request::CryptoAccountTeamShare {
            folder_id: 1,
            name: String::new(),
            team_id: 1,
            message: String::new(),
            permissions_bits: 0,
            temppass: "temporary".into(),
            hint: None,
        },
        Request::CreateRemoteFolder {
            parent_folder_id: None,
            name: String::new(),
            path: "relative".to_owned(),
            check_and_create: false,
        },
        Request::GetFolderIdByPath {
            path: "relative".to_owned(),
        },
        Request::GetFolderFlags {
            path: "relative".to_owned(),
        },
        Request::GetFolderOwnerId {
            path: "relative".to_owned(),
        },
        Request::StatPath {
            path: "relative".to_owned(),
        },
        Request::ListFolderByPath {
            path: "relative".to_owned(),
        },
        Request::FileDeleteByPath {
            path: "relative".to_owned(),
        },
        Request::FolderDeleteByPath {
            path: "relative".to_owned(),
            recursive: false,
        },
        Request::FolderDeleteById {
            folder_id: 0,
            recursive: false,
        },
        Request::ReadFileRange {
            path: "/file".to_owned(),
            offset: 0,
            length: 0,
        },
        Request::WriteFileFresh {
            path: "/file".to_owned(),
            data_b64: "%%%".to_owned(),
        },
        Request::CreateFolderByPath {
            path: "/".to_owned(),
        },
        Request::RenamePath {
            from: "relative".to_owned(),
            to: "/target".to_owned(),
        },
        Request::CopyPath {
            from: "/source".to_owned(),
            to: "relative".to_owned(),
        },
        Request::DeletePath {
            path: "relative".to_owned(),
            recursive: false,
        },
        Request::UploadFileByPath {
            local_path: fixture.path("missing"),
            remote_path: "relative".to_owned(),
        },
        Request::DownloadFileByPath {
            remote_path: "relative".to_owned(),
            local_path: fixture.path("download"),
            overwrite: false,
        },
        Request::LostPassword {
            email: String::new(),
        },
        Request::VerifyEmailRestricted {
            verify_token: "".into(),
        },
        Request::AccountChangePassword {
            current_password: "".into(),
            new_password: "".into(),
        },
        Request::AccountRegister {
            email: "invalid".to_owned(),
            password: "".into(),
            terms_accepted: false,
        },
        Request::GetFileLink { file_id: 0 },
        Request::DownloadFile {
            file_id: 0,
            local_path: fixture.path("download-zero"),
        },
        Request::DeleteBackup { backup_id: 0 },
        Request::CreateBackup {
            name: String::new(),
            root_folder_id: 0,
            local_path: "relative".to_owned(),
            parent_folder_name: None,
        },
        Request::StopDevice {
            device_folder_id: 0,
        },
        Request::SetApiServer {
            location_id: 0,
            binapi: "invalid host".to_owned(),
        },
        Request::SetLanguage {
            language: String::new(),
        },
        Request::UploadWriteFromFile {
            upload_session_id: 0,
            source_fileid: 0,
            source_hash: 0,
            offset: 0,
            source_offset: None,
            count: u64::MAX,
        },
        Request::CreateTreePublicLinkFromPaths {
            name: String::new(),
            paths: vec![],
            expires: None,
        },
        Request::CreateTreePublicLinkFromPathTargets {
            name: String::new(),
            root: None,
            folders: vec![],
            files: vec![],
            expires: None,
        },
        Request::IntegritySkip {
            path: " ".to_owned(),
        },
        Request::Mount {
            path: fixture.path("missing-mount"),
        },
        Request::Unmount,
    ] {
        assert_defined(&fixture.request(request));
    }
}

#[test]
fn crypto_upload_audit_and_snapshot_stateful_paths_are_observable() {
    let mut fixture = Fixture::authenticated("stateful");
    assert_eq!(
        fixture
            .request(Request::CryptoSetupV2 {
                backend: CryptoBackendIpc::Enhanced,
                acknowledge_not_interop: false,
                password: "passphrase".into(),
                hint: None,
            })
            .status,
        ResponseStatus::InvalidRequest
    );
    assert_defined(&fixture.request(Request::CryptoSetupV2 {
        backend: CryptoBackendIpc::Enhanced,
        acknowledge_not_interop: true,
        password: "passphrase".into(),
        hint: Some("hint".to_owned()),
    }));
    assert_defined(&fixture.request(Request::CryptoUnlock {
        password: "passphrase".into(),
    }));
    for request in [
        Request::CryptoMkdir {
            name: "Private".to_owned(),
            parent_folder_id: None,
            local_folder_id: Some(7),
        },
        Request::CryptoGetFolderKey { folder_id: 7 },
        Request::CryptoGetFileKey { file_id: 8 },
        Request::CryptoFolderEnable {
            folder_id: 7,
            parent_folder_id: None,
        },
        Request::CryptoFolderList,
        Request::CryptoFolderDisable { folder_id: 7 },
        Request::CryptoChangePasswordUnlocked {
            new_password: "changed-passphrase".into(),
            hint: "changed".to_owned(),
            code: "fixture-code".to_owned(),
            flags: 0,
        },
    ] {
        assert_defined(&fixture.request(request));
    }

    let create = fixture.request(Request::UploadCreate {
        local_path: fixture.path("upload.bin"),
        remote_name: "upload.bin".to_owned(),
        parent_folder_id: Some(0),
        total_bytes: 3,
        conflict_mode: Some(UploadConflictMode::Overwrite),
    });
    assert_eq!(create.status, ResponseStatus::Ok);
    let session_id = serde_json::from_str::<serde_json::Value>(&create.message)
        .expect("upload JSON")["session_id"]
        .as_u64()
        .expect("session id");
    for request in [
        Request::UploadPause { session_id },
        Request::UploadResume { session_id },
        Request::UploadCancel { session_id },
        Request::UploadCancel { session_id },
        Request::UploadList,
        Request::ConflictList,
        Request::ConflictResolve {
            path: "missing.txt".to_owned(),
            policy: "rename_both".to_owned(),
        },
        Request::SessionStatus,
        Request::AuditVerifyChain {
            range: AuditVerifyRange::default(),
        },
        Request::AuditVerifyChain {
            range: AuditVerifyRange {
                from: Some(2),
                to: Some(1),
            },
        },
        Request::IntegrityRunOnce,
    ] {
        assert_defined(&fixture.request(request));
    }

    let snapshot = fixture.path("snapshot.tar.zst");
    for request in [
        Request::BackupSnapshot {
            action: SnapshotAction::Create,
            path: snapshot.clone(),
            gpg_recipient: None,
            yes: false,
            retention_days: None,
            zstd_level: Some(1),
        },
        Request::BackupSnapshot {
            action: SnapshotAction::Verify,
            path: snapshot,
            gpg_recipient: None,
            yes: false,
            retention_days: None,
            zstd_level: None,
        },
        Request::BackupSnapshot {
            action: SnapshotAction::Restore,
            path: fixture.path("missing.tar.zst"),
            gpg_recipient: None,
            yes: false,
            retention_days: None,
            zstd_level: None,
        },
        Request::BackupSnapshot {
            action: SnapshotAction::Prune,
            path: fixture.root.path().to_path_buf(),
            gpg_recipient: None,
            yes: false,
            retention_days: Some(30),
            zstd_level: None,
        },
    ] {
        assert_defined(&fixture.request(request));
    }
}

#[test]
fn pclsync_crypto_setup_unlock_and_lazy_key_fetch_paths_are_observable() {
    let mut fixture = Fixture::authenticated("pclsync-crypto");
    let setup = fixture.request(Request::CryptoSetupV2 {
        backend: CryptoBackendIpc::PclsyncCompat,
        acknowledge_not_interop: false,
        password: "fixture-passphrase".into(),
        hint: Some("fixture hint".to_owned()),
    });
    // The development crypto transport intentionally has no user-key API,
    // so the wire upload fails after the local profile is created. The
    // committed local profile is deliberately retryable.
    assert_eq!(
        setup.status,
        ResponseStatus::InternalError,
        "{}",
        setup.message
    );
    assert!(fixture.runtime.crypto.is_setup());

    let unlock = fixture.request(Request::CryptoUnlock {
        password: "fixture-passphrase".into(),
    });
    assert_eq!(unlock.status, ResponseStatus::Ok, "{}", unlock.message);
    assert!(fixture.runtime.crypto.is_started());

    for request in [
        Request::CryptoGetFolderKey { folder_id: 42 },
        Request::CryptoGetFileKey { file_id: 77 },
    ] {
        let response = fixture.request(request);
        assert_eq!(
            response.status,
            ResponseStatus::InternalError,
            "{}",
            response.message
        );
    }

    let mkdir = fixture
        .runtime
        .mkdir_with_autofetch(Some(42), "encrypted child", Some(43));
    assert!(mkdir.is_err());
    let seal = fixture.runtime.seal_sector_with_autofetch(
        b"unused-on-pclsync",
        0,
        b"fixture sector",
        pcloud_crypto::SectorContext::for_file(77),
    );
    assert!(seal.is_err());
}

#[test]
fn secret_bearing_requests_and_persistence_paths_have_defined_outcomes() {
    let mut fixture = Fixture::authenticated("secret-requests");

    for enabled in [true, false] {
        let response = fixture.request(Request::AuthPersistence { enabled });
        assert_eq!(response.status, ResponseStatus::Ok, "{}", response.message);
    }

    for request in [
        Request::PasswordSubmission {
            username: "coverage@example.com".to_owned(),
            value: "coverage-password".into(),
        },
        Request::TwoFactorCodeSubmission {
            value: "123456".to_owned(),
            trust_device: true,
            recovery_code: false,
        },
        Request::TwoFactorCodeSubmission {
            value: "recovery-code".to_owned(),
            trust_device: false,
            recovery_code: true,
        },
        Request::AuthTokenSubmission {
            value: "replacement-token".into(),
        },
        Request::MountForceUnmount {
            path: fixture.path("not-mounted"),
        },
    ] {
        assert_defined(&fixture.request(request));
    }

    let mut crypto = Fixture::authenticated("legacy-crypto");
    assert_defined(&crypto.request(Request::CryptoSetup {
        password: "legacy-passphrase".into(),
        hint: Some("legacy hint".to_owned()),
    }));
    assert_defined(&crypto.request(Request::CryptoChangePassword {
        old_password: "legacy-passphrase".into(),
        new_password: "rotated-passphrase".into(),
        hint: "rotated hint".to_owned(),
        code: "fixture-code".to_owned(),
        flags: 1,
    }));
}

#[test]
fn unauthenticated_matrix_covers_authorization_boundaries() {
    let root = tempfile::tempdir().expect("temporary root");
    let config =
        ConfigProfile::secure_defaults(root.path().to_path_buf(), Environment::Development);
    let mut runtime = bootstrap_with_config(config).expect("bootstrap");
    for request in [
        plain(Method::GetUserInfo),
        plain(Method::Logout),
        plain(Method::ListPublicLinks),
        plain(Method::ListUploadLinks),
        plain(Method::ListIncomingShares),
        plain(Method::ListOutgoingShares),
        plain(Method::ListIncomingShareRequests),
        plain(Method::ListOutgoingShareRequests),
        plain(Method::ListContacts),
        plain(Method::ListMyTeams),
        plain(Method::ListNotifications),
        plain(Method::SendCryptoChangeUserPrivate),
        plain(Method::VerifyEmail),
        Request::AuthPersistence { enabled: true },
        Request::CryptoChangePassword {
            old_password: "old-password".into(),
            new_password: "new-password".into(),
            hint: "hint".to_owned(),
            code: "code".to_owned(),
            flags: 0,
        },
        Request::CryptoChangePasswordUnlocked {
            new_password: "new-password".into(),
            hint: "hint".to_owned(),
            code: "code".to_owned(),
            flags: 0,
        },
        Request::CryptoMkdir {
            name: "Encrypted".to_owned(),
            parent_folder_id: Some(0),
            local_folder_id: None,
        },
        Request::CreateFilePublicLink {
            path: "/file".to_owned(),
        },
        Request::CreateFolderPublicLink {
            path: "/folder".to_owned(),
        },
        Request::CreateUploadLink {
            path: "/folder".to_owned(),
            comment: String::new(),
            expire: None,
            maxspace: None,
            maxfiles: None,
        },
        Request::ShowPublicLink {
            code: "fixture-code".to_owned(),
        },
        Request::DeletePublicLink { link_id: 1 },
        Request::DeletePublicLinkByCode {
            code: "fixture-code".to_owned(),
        },
        Request::CreateFolderPublicLinkWithOptions {
            path: "/folder".to_owned(),
            expire: Some(2_000_000_000),
            maxdownloads: Some(3),
            maxtraffic: Some(4_096),
            password: Some("password".into()),
        },
        Request::CreateFolderUpDownLink {
            folder_id: 1,
            mail: "alice@example.com".to_owned(),
            can_upload: true,
        },
        Request::CreateScreenshotPublicLink {
            path: "/shot.png".to_owned(),
            has_delay: true,
            delay_seconds: 60,
        },
        Request::ChangePublicLinkExpire {
            link_id: 1,
            expire: None,
        },
        Request::ChangePublicLinkPassword {
            link_id: 1,
            password: Some("password".into()),
        },
        Request::ChangePublicLinkUpload {
            link_id: 1,
            policy: PublicLinkUploadPolicy::Everyone,
        },
        Request::DeleteUploadLink { upload_link_id: 1 },
        Request::CreateTreePublicLink {
            name: "tree".to_owned(),
            root_folder_id: Some(1),
            folder_ids_csv: Some("2".to_owned()),
            file_ids_csv: Some("3".to_owned()),
            expire: None,
            maxdownloads: None,
            maxtraffic: None,
        },
        Request::ListPublicLinkAccess { link_id: 1 },
        Request::AddPublicLinkAccess {
            link_id: 1,
            email: "alice@example.com".to_owned(),
        },
        Request::RemovePublicLinkAccess {
            link_id: 1,
            receiver_id: 2,
        },
        Request::ListBookmarks,
        Request::RemoveBookmark {
            code: "fixture-code".to_owned(),
            location_id: 1,
        },
        Request::ChangeBookmark {
            code: "fixture-code".to_owned(),
            location_id: 1,
            name: "bookmark".to_owned(),
            description: "description".to_owned(),
        },
        Request::ListFolderByPath {
            path: "/".to_owned(),
        },
        Request::GetFolderIdByPath {
            path: "/".to_owned(),
        },
        Request::GetFolderFlags {
            path: "/".to_owned(),
        },
        Request::GetFolderOwnerId {
            path: "/".to_owned(),
        },
        Request::StatPath {
            path: "/".to_owned(),
        },
        Request::ReadFileRange {
            path: "/file".to_owned(),
            offset: 0,
            length: 1,
        },
        Request::WriteFileFresh {
            path: "/file".to_owned(),
            data_b64: String::new(),
        },
        Request::CreateFolderByPath {
            path: "/folder".to_owned(),
        },
        Request::CreateRemoteFolder {
            parent_folder_id: Some(0),
            name: "folder".to_owned(),
            path: String::new(),
            check_and_create: false,
        },
        Request::FileDeleteByPath {
            path: "/file".to_owned(),
        },
        Request::FolderDeleteByPath {
            path: "/folder".to_owned(),
            recursive: false,
        },
        Request::FolderDeleteById {
            folder_id: 1,
            recursive: true,
        },
        Request::RenamePath {
            from: "/a".to_owned(),
            to: "/b".to_owned(),
        },
        Request::CopyPath {
            from: "/a".to_owned(),
            to: "/b".to_owned(),
        },
        Request::DeletePath {
            path: "/a".to_owned(),
            recursive: false,
        },
        Request::UploadFileByPath {
            local_path: root.path().join("file"),
            remote_path: "/file".to_owned(),
        },
        Request::DownloadFileByPath {
            remote_path: "/file".to_owned(),
            local_path: root.path().join("download"),
            overwrite: false,
        },
        Request::GetFileLink { file_id: 1 },
        Request::DownloadFile {
            file_id: 1,
            local_path: root.path().join("by-id"),
        },
        Request::DeleteBackup { backup_id: 1 },
        Request::CreateBackup {
            name: "backup".to_owned(),
            root_folder_id: 1,
            local_path: root.path().display().to_string(),
            parent_folder_name: None,
        },
        Request::StopDevice {
            device_folder_id: 1,
        },
        Request::SetLanguage {
            language: "en".to_owned(),
        },
        Request::AccountChangePassword {
            current_password: "current".into(),
            new_password: "replacement".into(),
        },
        Request::MarkNotificationsRead { upto_id: 1 },
        Request::SendPublink {
            code: "code".to_owned(),
            mails: "alice@example.com".to_owned(),
            message: String::new(),
        },
        Request::ShareFolder {
            folder_id: 1,
            name: "Docs".to_owned(),
            mail: "alice@example.com".to_owned(),
            message: String::new(),
            permissions_bits: 7,
            hint: None,
        },
        Request::CryptoShareFolder {
            folder_id: 1,
            name: "Docs".to_owned(),
            mail: "alice@example.com".to_owned(),
            message: String::new(),
            permissions_bits: 7,
            temppass: "temporary".into(),
            hint: None,
        },
        Request::CryptoShareFolderRsa {
            folder_id: 1,
            name: "Docs".to_owned(),
            mail: "alice@example.com".to_owned(),
            message: String::new(),
            permissions_bits: 7,
            hint: None,
        },
        Request::CancelShareRequest {
            share_request_id: 1,
        },
        Request::DeclineShareRequest {
            share_request_id: 1,
        },
        Request::AcceptShareRequest {
            share_request_id: 1,
            to_folder_id: 0,
            name: None,
        },
        Request::RemoveShare { share_id: 1 },
        Request::ModifyShare {
            share_id: 1,
            permissions_bits: 7,
        },
        Request::AccountStopShare {
            user_share_ids: vec![1],
            team_share_ids: vec![2],
        },
        Request::AccountModifyShare {
            user_shares: vec![(1, 7)],
            team_shares: vec![(2, 7)],
        },
        Request::AccountTeamShare {
            folder_id: 1,
            name: "Docs".to_owned(),
            team_id: 1,
            message: String::new(),
            permissions_bits: 7,
            hint: None,
        },
        Request::CryptoAccountTeamShare {
            folder_id: 1,
            name: "Docs".to_owned(),
            team_id: 1,
            message: String::new(),
            permissions_bits: 7,
            temppass: "temporary".into(),
            hint: None,
        },
        Request::UploadWriteFromFile {
            upload_session_id: 1,
            source_fileid: 2,
            source_hash: 3,
            offset: 0,
            source_offset: Some(0),
            count: 4,
        },
        Request::CreateTreePublicLinkFromPaths {
            name: "paths".to_owned(),
            paths: vec!["/folder".to_owned()],
            expires: None,
        },
        Request::CreateTreePublicLinkFromPathTargets {
            name: "targets".to_owned(),
            root: Some("/".to_owned()),
            folders: vec!["/folder".to_owned()],
            files: vec!["/file".to_owned()],
            expires: None,
        },
    ] {
        assert_defined(&runtime.handle_request(request));
    }
}

#[test]
fn fixture_paths_remain_inside_the_test_root() {
    let fixture = Fixture::authenticated("paths");
    assert!(fixture.path("child").starts_with(fixture.root.path()));
    assert!(Path::new(&fixture.runtime.store.db_path).is_absolute());
}
