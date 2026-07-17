use std::path::Path;

use pcloud_config::Environment;
use pcloud_embedded_sdk::{
    AccountUtilityError, AuthHelperError, BackupHelperError, ConflictMode, CreateFolderHelperError,
    CryptoHelperError, DownloadHelperError, EmbeddedDaemon, EmbeddedDaemonError,
    FileMutationHelperError, FolderMetadataError, MountHelperError, NotificationsHelperError,
    PublinkHelperError, RemoteDriveError, SdkError, SettingKvError, TreePublicLinkHelperError,
    UploadError, UploadHelperError, UploadPayload, UploadRequest, ValueKvError,
};
use pcloud_plugin_api::{
    Plugin, PluginCapability, PluginContext, PluginError, PluginManifest, PluginOperation,
};
use pcloud_secret::secret_string::SecretString;

fn daemon(label: &str) -> (tempfile::TempDir, EmbeddedDaemon) {
    let root = tempfile::Builder::new()
        .prefix(&format!("pcloud-sdk-coverage-{label}-"))
        .tempdir()
        .expect("temporary SDK root");
    let daemon = EmbeddedDaemon::builder(root.path().to_path_buf())
        .environment(Environment::Development)
        .build()
        .expect("development daemon");
    (root, daemon)
}

fn authenticated(label: &str) -> (tempfile::TempDir, EmbeddedDaemon) {
    let (root, mut daemon) = daemon(label);
    daemon
        .login_with_token("coverage-auth-token")
        .expect("development token login");
    (root, daemon)
}

#[test]
fn builder_auth_and_upload_session_surface_is_observable() {
    let (_root, mut daemon) = daemon("builder");
    assert!(!daemon.runtime_summary().is_empty());
    assert_eq!(daemon.config().environment, Environment::Development);
    assert!(daemon.loaded_plugins().is_empty());
    assert!(!daemon.is_authenticated());
    assert_eq!(daemon.current_user_id(), None);
    assert_eq!(daemon.username(), None);
    assert_eq!(daemon.crypto_priv_key_flags(), 0);

    let unauthenticated = daemon.start_upload(UploadRequest::new(
        0,
        "unauthenticated.bin",
        UploadPayload::Bytes(b"payload".to_vec()),
        ConflictMode::CreateIfAbsent,
    ));
    assert!(unauthenticated.await_completion().is_err());

    daemon
        .login_with_token("coverage-auth-token")
        .expect("token login");
    assert!(daemon.is_authenticated());
    assert!(daemon.auth_token_secret().is_some());
    assert!(daemon.userinfo().is_ok());

    let completed = daemon.start_upload(UploadRequest::new(
        0,
        "session.bin",
        UploadPayload::Bytes(b"session payload".to_vec()),
        ConflictMode::IfHashNumeric(7),
    ));
    let metadata = completed
        .await_completion()
        .expect("development session upload");
    assert_eq!(metadata.bytes_uploaded, 15);
    assert_eq!(metadata.parent_folder_id, 0);

    daemon.logout().expect("logout");
    assert!(!daemon.is_authenticated());
}

#[test]
fn folder_creation_validates_inputs_and_uses_development_backend() {
    let (_root, mut daemon) = daemon("folders");
    assert!(matches!(
        daemon.create_remote_folder(0, "project"),
        Err(SdkError::CreateFolder(
            CreateFolderHelperError::NotAuthenticated
        ))
    ));

    daemon
        .login_with_token("coverage-auth-token")
        .expect("token login");
    assert!(matches!(
        daemon.create_remote_folder(0, "   "),
        Err(SdkError::CreateFolder(CreateFolderHelperError::EmptyName))
    ));
    assert!(matches!(
        daemon.create_remote_folder_by_path("relative/path"),
        Err(SdkError::CreateFolder(CreateFolderHelperError::InvalidPath))
    ));
    assert!(matches!(
        daemon.check_and_create_folder(0, ""),
        Err(SdkError::CreateFolder(CreateFolderHelperError::EmptyName))
    ));

    let created = daemon
        .create_remote_folder(0, "project")
        .expect("create folder");
    assert_eq!(created.name, "project");
    assert!(created.suffix_index.is_none());

    let by_path = daemon
        .create_remote_folder_by_path("/Documents/new")
        .expect("create folder by path");
    assert_eq!(by_path.name, "new");

    let idempotent = daemon
        .check_and_create_folder(0, "inbox")
        .expect("check and create folder");
    assert_eq!(idempotent.suffix_index, Some(0));
}

#[test]
fn backup_helpers_cover_missing_state_and_complete_lifecycle() {
    let (_root, mut daemon) = daemon("backup");
    assert!(matches!(
        daemon.create_backup("Documents", None),
        Err(SdkError::Backup(BackupHelperError::NotAuthenticated))
    ));
    assert!(matches!(
        daemon.stop_device(None),
        Err(SdkError::Backup(BackupHelperError::NotAuthenticated))
    ));

    daemon
        .login_with_token("coverage-auth-token")
        .expect("token login");
    assert!(matches!(
        daemon.create_backup(" ", None),
        Err(SdkError::Backup(BackupHelperError::EmptyName))
    ));
    assert!(matches!(
        daemon.create_backup("Documents", None),
        Err(SdkError::Backup(BackupHelperError::BackupRootMissing))
    ));
    assert!(matches!(
        daemon.stop_device(None),
        Err(SdkError::Backup(BackupHelperError::DeviceFolderMissing))
    ));

    daemon
        .set_backup_device_folder_id(41)
        .expect("persist backup device");
    assert_eq!(daemon.backup_device_folder_id(), Some(41));
    let created = daemon
        .create_backup("Documents", Some("Backups".to_owned()))
        .expect("create backup");
    assert_ne!(created.folder_id, 0);
    daemon
        .delete_backup(created.folder_id)
        .expect("delete backup");
    daemon.stop_device(None).expect("stop stored device");
    daemon.stop_device(Some(42)).expect("stop explicit device");
    daemon
        .delete_backup_device()
        .expect("clear backup device state");
    assert_eq!(daemon.backup_device_folder_id(), None);
}

#[test]
fn typed_values_and_settings_round_trip_and_enforce_kinds() {
    let (_root, daemon) = daemon("kv");

    assert_eq!(daemon.get_uint_value("uint").unwrap(), 0);
    assert_eq!(daemon.get_int_value("int").unwrap(), 0);
    assert!(!daemon.get_bool_value("bool").unwrap());
    assert_eq!(daemon.get_string_value("string").unwrap(), None);
    assert!(!daemon.has_uint_value("uint").unwrap());
    assert!(!daemon.has_int_value("int").unwrap());
    assert!(!daemon.has_bool_value("bool").unwrap());
    assert!(!daemon.has_string_value("string").unwrap());

    daemon.set_uint_value("uint", 42).unwrap();
    daemon.set_int_value("int", -7).unwrap();
    daemon.set_bool_value("bool", true).unwrap();
    daemon.set_string_value("string", "value").unwrap();
    assert_eq!(daemon.get_uint_value("uint").unwrap(), 42);
    assert_eq!(daemon.get_int_value("int").unwrap(), -7);
    assert!(daemon.get_bool_value("bool").unwrap());
    assert_eq!(
        daemon.get_string_value("string").unwrap().as_deref(),
        Some("value")
    );
    assert!(daemon.has_uint_value("uint").unwrap());
    assert!(daemon.has_int_value("int").unwrap());
    assert!(daemon.has_bool_value("bool").unwrap());
    assert!(daemon.has_string_value("string").unwrap());

    assert_eq!(daemon.get_bool_setting("bool-setting").unwrap(), None);
    assert_eq!(daemon.get_int_setting("int-setting").unwrap(), None);
    assert_eq!(daemon.get_uint_setting("uint-setting").unwrap(), None);
    assert_eq!(daemon.get_string_setting("string-setting").unwrap(), None);

    daemon.set_bool_setting("bool-setting", true).unwrap();
    daemon.set_int_setting("int-setting", -9).unwrap();
    daemon.set_uint_setting("uint-setting", 99).unwrap();
    daemon.set_string_setting("string-setting", "dark").unwrap();
    assert_eq!(daemon.get_bool_setting("bool-setting").unwrap(), Some(true));
    assert_eq!(daemon.get_int_setting("int-setting").unwrap(), Some(-9));
    assert_eq!(daemon.get_uint_setting("uint-setting").unwrap(), Some(99));
    assert_eq!(
        daemon
            .get_string_setting("string-setting")
            .unwrap()
            .as_deref(),
        Some("dark")
    );
    assert!(daemon.reset_setting("string-setting").unwrap());
    assert!(!daemon.reset_setting("string-setting").unwrap());

    daemon.set_string_setting("kind-mismatch", "text").unwrap();
    assert!(matches!(
        daemon.get_bool_setting("kind-mismatch"),
        Err(SdkError::Setting(SettingKvError::Store(_)))
    ));
}

#[test]
fn remote_drive_exercises_metadata_streaming_and_mutations() {
    let (root, mut daemon) = daemon("remote-unauth");
    assert!(matches!(
        daemon.remote().stat("/"),
        Err(RemoteDriveError::Unauthorized(_))
    ));
    assert!(matches!(
        daemon.remote().stat("relative"),
        Err(RemoteDriveError::InvalidRequest(_))
    ));
    drop(daemon);
    drop(root);

    let (root, mut daemon) = authenticated("remote");
    let root_stat = daemon.remote().stat("/").expect("stat root");
    assert!(root_stat.id.is_folder());
    let listing = daemon.remote().list("/").expect("list root");
    assert!(!listing.entries.is_empty());
    assert!(matches!(
        daemon.remote().list("/notes.txt"),
        Err(RemoteDriveError::Conflict(_))
    ));
    assert!(matches!(
        daemon.remote().read_range("/notes.txt", 0, 0),
        Err(RemoteDriveError::InvalidRequest(_))
    ));
    let read = daemon
        .remote()
        .read_range("/notes.txt", 0, 8)
        .expect("bounded read");
    assert!(!read.data.is_empty());

    let source = root.path().join("upload.bin");
    std::fs::write(&source, b"remote upload payload").unwrap();
    let uploaded = daemon
        .remote()
        .upload(&source, "/uploaded.bin")
        .expect("remote upload");
    assert_eq!(uploaded.bytes, 21);

    let destination = root.path().join("download.bin");
    let downloaded = daemon
        .remote()
        .download("/notes.txt", &destination, true)
        .expect("remote download");
    assert_eq!(downloaded.path, destination);
    assert!(downloaded.bytes > 0);

    daemon.remote().mkdir("/NewFolder").expect("remote mkdir");
    assert!(matches!(
        daemon.remote().copy("/notes.txt", "/copy.txt"),
        Err(RemoteDriveError::InvalidRequest(_))
    ));
    assert!(matches!(
        daemon.remote().move_path("/notes.txt", "/moved.txt"),
        Err(RemoteDriveError::Unavailable(_))
    ));
    assert!(daemon.remote().delete("/notes.txt", false).is_err());
    daemon
        .remote()
        .share_folder("/Documents", "reader@example.com", "hello", 7, None)
        .expect("remote share");
}

#[test]
fn tree_links_and_file_mutations_cover_success_and_validation() {
    let (_root, mut daemon) = daemon("tree-and-files");
    assert!(matches!(
        daemon.create_tree_public_link_from_paths("bundle", vec!["/Documents".into()], None),
        Err(SdkError::TreePublicLink(
            TreePublicLinkHelperError::NotAuthenticated
        ))
    ));
    assert!(matches!(
        daemon.delete_file("/notes.txt"),
        Err(SdkError::FileMutation(
            FileMutationHelperError::NotAuthenticated
        ))
    ));

    daemon
        .login_with_token("coverage-auth-token")
        .expect("token login");
    assert!(matches!(
        daemon.create_tree_public_link_from_paths(" ", vec!["/Documents".into()], None),
        Err(SdkError::TreePublicLink(
            TreePublicLinkHelperError::EmptyName
        ))
    ));
    assert!(matches!(
        daemon.create_tree_public_link_from_paths("bundle", Vec::new(), None),
        Err(SdkError::TreePublicLink(
            TreePublicLinkHelperError::EmptyPaths
        ))
    ));
    assert!(matches!(
        daemon.rename_file("relative", "/new"),
        Err(SdkError::FileMutation(
            FileMutationHelperError::RenameFailed(_)
        ))
    ));
    assert!(matches!(
        daemon.rename_file("/notes.txt", "relative"),
        Err(SdkError::FileMutation(
            FileMutationHelperError::RenameFailed(_)
        ))
    ));

    assert!(matches!(
        daemon.create_tree_public_link_from_paths(
            "bundle",
            vec!["/Documents".to_owned(), "/notes.txt".to_owned()],
            Some(1_900_000_000),
        ),
        Err(SdkError::TreePublicLink(
            TreePublicLinkHelperError::PathResolution(_)
        ))
    ));
    assert!(matches!(
        daemon.create_tree_public_link_from_targets(
            "targeted",
            Some("/".to_owned()),
            vec!["/Documents".to_owned()],
            vec!["/notes.txt".to_owned()],
            None,
        ),
        Err(SdkError::TreePublicLink(TreePublicLinkHelperError::Api(_)))
    ));

    let info = daemon.get_file_info("/notes.txt").expect("file metadata");
    assert!(!info.is_folder);
    assert!(matches!(
        daemon.get_file_info("/Documents"),
        Err(SdkError::FileMutation(FileMutationHelperError::StatFailed(
            _
        )))
    ));
    assert!(matches!(
        daemon.stat_path("relative"),
        Err(SdkError::Folder(FolderMetadataError::InvalidPath))
    ));

    assert!(matches!(
        daemon.rename_file("/notes.txt", "/renamed.txt"),
        Err(SdkError::FileMutation(
            FileMutationHelperError::RenameFailed(_)
        ))
    ));
    assert!(matches!(
        daemon.delete_file("/notes.txt"),
        Err(SdkError::FileMutation(
            FileMutationHelperError::DeleteFailed(_)
        ))
    ));
}

#[test]
fn crypto_and_upload_validation_reaches_typed_error_paths() {
    let (_root, mut daemon) = daemon("crypto-errors");
    assert!(matches!(
        daemon.crypto_send_change_user_private(),
        Err(SdkError::Crypto(CryptoHelperError::NotAuthenticated))
    ));
    assert!(matches!(
        daemon.upload_file(0, "missing.bin", Path::new("/definitely/missing")),
        Err(SdkError::Upload(UploadHelperError::ReadLocalFile(_)))
    ));

    let empty = SecretString::new(String::new());
    let password = SecretString::new("new-password");
    assert!(matches!(
        daemon.crypto_change_password(empty, password, "hint", "code", 0),
        Err(SdkError::Crypto(CryptoHelperError::EmptyPassword))
    ));
    assert!(matches!(
        daemon.crypto_change_password(
            SecretString::new("old-password"),
            SecretString::new("new-password"),
            "hint",
            "",
            0
        ),
        Err(SdkError::Crypto(CryptoHelperError::EmptyCode))
    ));
    assert!(matches!(
        daemon.crypto_change_password(
            SecretString::new("old-password"),
            SecretString::new("new-password"),
            "hint",
            "code",
            0
        ),
        Err(SdkError::Crypto(CryptoHelperError::NotAuthenticated))
    ));
    assert!(matches!(
        daemon.crypto_change_password_unlocked(SecretString::new(""), "hint", "code", 0),
        Err(SdkError::Crypto(CryptoHelperError::EmptyPassword))
    ));
    assert!(matches!(
        daemon.crypto_change_password_unlocked(SecretString::new("new-password"), "hint", "", 0),
        Err(SdkError::Crypto(CryptoHelperError::EmptyCode))
    ));
    assert!(matches!(
        daemon.crypto_change_password_unlocked(
            SecretString::new("new-password"),
            "hint",
            "code",
            0
        ),
        Err(SdkError::Crypto(CryptoHelperError::NotAuthenticated))
    ));

    daemon
        .login_with_token("coverage-auth-token")
        .expect("token login");
    daemon
        .crypto_send_change_user_private()
        .expect("development confirmation request");
    assert!(matches!(
        daemon.crypto_change_password(
            SecretString::new("old-password"),
            SecretString::new("new-password"),
            "hint",
            "code",
            0
        ),
        Err(SdkError::Crypto(CryptoHelperError::Shell(_)))
    ));
}

#[test]
fn account_notifications_download_and_convenience_helpers_are_executable() {
    let (root, mut daemon) = daemon("convenience");
    assert!(daemon.get_promo().is_err());
    assert!(daemon.set_language("de").is_err());
    assert!(daemon.verify_email().is_err());
    assert!(daemon.change_password("old", "new").is_err());
    assert!(daemon.get_file_link(1, None).is_err());
    assert!(daemon.download_file(1).is_err());
    assert!(daemon.mount(root.path().join("missing").as_path()).is_err());
    assert!(daemon.unmount().is_err());
    assert!(daemon.submit_recovery_code("recovery", false).is_err());
    assert!(daemon.send_two_factor_sms().is_err());
    assert!(daemon.send_two_factor_notification().is_err());
    assert!(
        daemon
            .register("alice@example.com", SecretString::new(String::new()), true)
            .is_err()
    );
    assert!(daemon.rename_file("/notes.txt", "/renamed.txt").is_err());
    assert!(daemon.get_file_info("/notes.txt").is_err());

    daemon
        .login_with_token("coverage-auth-token")
        .expect("token login");
    daemon
        .set_api_server("binapi.pcloud.com", 1)
        .expect("development API selection");
    assert!(!daemon.get_api_servers().unwrap().is_empty());
    assert!(daemon.get_promo().unwrap().is_some());
    daemon.set_language("de").unwrap();
    daemon.verify_email().unwrap();
    daemon.verify_email_restricted("fixture-token").unwrap();
    daemon.lost_password("alice@example.com").unwrap();

    let notifications = daemon.list_notifications().unwrap();
    assert!(!notifications.is_empty());
    daemon.mark_notifications_read(1).unwrap();
    assert_eq!(daemon.run_localscan(), 1);

    let listing = daemon.list_folder("/").unwrap();
    assert!(!listing.is_empty());
    assert!(daemon.get_folder_id_by_path("/").is_err());
    assert!(daemon.get_folder_flags("/").is_err());
    assert!(daemon.get_folder_id_by_path("/Documents").is_err());
    assert!(daemon.get_folder_flags("/Documents").is_err());
    assert!(daemon.get_folder_owner_id("relative").is_err());
    assert!(daemon.get_folder_owner_id("/Documents").is_err());
    assert_eq!(
        daemon.filesystem_status(root.path().join("outside")),
        pcloud_embedded_sdk::FilesystemPathStatus::Invalid
    );

    let uploaded = daemon
        .upload_data(0, "bytes.bin", b"embedded bytes")
        .expect("upload bytes");
    assert_eq!(uploaded.bytes_uploaded, 14);
    let source = root.path().join("source.bin");
    std::fs::write(&source, b"file bytes").unwrap();
    assert!(
        daemon
            .upload_file_as("/Documents", "source.bin", &source)
            .is_err()
    );
    assert_eq!(
        daemon
            .upload_file(0, "source.bin", &source)
            .unwrap()
            .bytes_uploaded,
        10
    );
    assert!(daemon.upload_write_from_file(0, 1, 2, 0, 0, 4).is_err());

    let link = daemon.get_file_link(1, None).expect("development link");
    assert!(!link.hosts.is_empty());
    assert!(!daemon.download_file(1).unwrap().is_empty());
    assert!(daemon.mount(root.path().join("missing").as_path()).is_err());
    assert!(daemon.unmount().is_err());
    assert!(daemon.delete_file("relative").is_err());
    assert!(
        daemon
            .create_tree_public_link_from_targets(
                " ",
                Some("/".to_owned()),
                Vec::new(),
                Vec::new(),
                None
            )
            .is_err()
    );
}

#[test]
fn builder_rejects_relative_extension_directory() {
    let root = tempfile::tempdir().unwrap();
    let mut policy =
        pcloud_config::extensions::ExtensionPolicy::secure_defaults("relative-plugins".into());
    policy.plugins_enabled = true;
    let error = EmbeddedDaemon::builder(root.path().to_path_buf())
        .environment(Environment::Development)
        .extension_policy(policy)
        .build()
        .expect_err("relative extension directory must fail validation");
    assert!(matches!(
        error,
        SdkError::EmbeddedDaemon(EmbeddedDaemonError::Bootstrap(_))
    ));
}

#[test]
fn unified_error_taxonomy_routes_every_sdk_family_and_branch() {
    fn category(error: impl Into<pcloud_error::Error>) -> String {
        error.into().category().to_owned()
    }

    assert_eq!(
        category(SdkError::Auth(AuthHelperError::Login("x".into()))),
        "auth"
    );
    assert_eq!(
        category(SdkError::Upload(UploadHelperError::NotAuthenticated)),
        "auth"
    );
    assert_eq!(
        category(SdkError::UploadSession(UploadError::Canceled)),
        "busy"
    );
    assert_eq!(
        category(SdkError::Download(DownloadHelperError::NotAuthenticated)),
        "auth"
    );
    assert_eq!(
        category(SdkError::Crypto(CryptoHelperError::EmptyPassword)),
        "invalid_input"
    );
    assert_eq!(
        category(SdkError::Backup(BackupHelperError::Persist("x".into()))),
        "storage"
    );
    assert_eq!(
        category(SdkError::Publink(PublinkHelperError::EmptyCode)),
        "invalid_input"
    );
    assert_eq!(
        category(SdkError::TreePublicLink(
            TreePublicLinkHelperError::EmptyName
        )),
        "invalid_input"
    );
    assert_eq!(
        category(SdkError::Folder(FolderMetadataError::InvalidPath)),
        "invalid_input"
    );
    assert_eq!(
        category(SdkError::CreateFolder(CreateFolderHelperError::InvalidPath)),
        "invalid_input"
    );
    assert_eq!(
        category(SdkError::Account(AccountUtilityError::TermsNotAccepted)),
        "invalid_input"
    );
    assert_eq!(
        category(SdkError::Notifications(
            NotificationsHelperError::InvalidNotificationId
        )),
        "invalid_input"
    );
    assert_eq!(
        category(SdkError::Kv(ValueKvError::Store("x".into()))),
        "storage"
    );
    assert_eq!(
        category(SdkError::Setting(SettingKvError::Store("x".into()))),
        "storage"
    );
    assert_eq!(
        category(SdkError::EmbeddedDaemon(EmbeddedDaemonError::Plugin(
            PluginError::Disabled
        ))),
        "plugin"
    );
    assert_eq!(
        category(SdkError::FileMutation(
            FileMutationHelperError::NotAuthenticated
        )),
        "auth"
    );
    assert_eq!(
        category(SdkError::Mount(MountHelperError::Mount("x".into()))),
        "local_io"
    );
    assert_eq!(
        category(SdkError::Io(std::io::Error::other("x"))),
        "local_io"
    );

    for error in [
        UploadHelperError::ResolveRemoteFolder("x".into()),
        UploadHelperError::Create("x".into()),
        UploadHelperError::Write("x".into()),
    ] {
        assert_eq!(category(error), "api");
    }
    assert_eq!(
        category(UploadHelperError::ReadLocalFile(std::io::Error::from(
            std::io::ErrorKind::NotFound
        ))),
        "local_io"
    );
    for error in [
        BackupHelperError::BackupRootMissing,
        BackupHelperError::DeviceFolderMissing,
        BackupHelperError::EmptyName,
    ] {
        assert_eq!(category(error), "invalid_input");
    }
    for error in [
        BackupHelperError::Create("x".into()),
        BackupHelperError::StopBackup("x".into()),
        BackupHelperError::StopDevice("x".into()),
    ] {
        assert_eq!(category(error), "api");
    }
    for error in [
        AccountUtilityError::VerifyEmail("x".into()),
        AccountUtilityError::VerifyEmailRestricted("x".into()),
        AccountUtilityError::LostPassword("x".into()),
        AccountUtilityError::ApiServers("x".into()),
        AccountUtilityError::Promo("x".into()),
        AccountUtilityError::SetLanguage("x".into()),
        AccountUtilityError::ChangePassword("x".into()),
        AccountUtilityError::SetApiServer("x".into()),
        AccountUtilityError::Register("x".into()),
    ] {
        assert_eq!(category(error), "api");
    }
    assert_eq!(category(AccountUtilityError::NotAuthenticated), "auth");
    assert_eq!(
        category(AccountUtilityError::InvalidRegistrationInput),
        "invalid_input"
    );
    assert_eq!(category(NotificationsHelperError::NotAuthenticated), "auth");
    assert_eq!(category(NotificationsHelperError::List("x".into())), "api");
    assert_eq!(
        category(NotificationsHelperError::MarkRead("x".into())),
        "api"
    );
    assert_eq!(category(FolderMetadataError::NotAuthenticated), "auth");
    assert_eq!(category(FolderMetadataError::Resolve("x".into())), "api");
    assert_eq!(category(PublinkHelperError::NotAuthenticated), "auth");
    assert_eq!(
        category(PublinkHelperError::EmptyRecipients),
        "invalid_input"
    );
    assert_eq!(category(PublinkHelperError::Send("x".into())), "api");
    assert_eq!(
        category(TreePublicLinkHelperError::NotAuthenticated),
        "auth"
    );
    assert_eq!(
        category(TreePublicLinkHelperError::EmptyPaths),
        "invalid_input"
    );
    assert_eq!(
        category(TreePublicLinkHelperError::PathResolution("x".into())),
        "api"
    );
    assert_eq!(category(TreePublicLinkHelperError::Api("x".into())), "api");
    assert_eq!(category(CryptoHelperError::NotAuthenticated), "auth");
    assert_eq!(category(CryptoHelperError::EmptyCode), "invalid_input");
    for error in [
        CryptoHelperError::Shell("x".into()),
        CryptoHelperError::SendChangeUserPrivate("x".into()),
        CryptoHelperError::ChangeUserPrivate("x".into()),
    ] {
        assert_eq!(category(error), "crypto");
    }
    for error in [
        AuthHelperError::NotAuthenticated,
        AuthHelperError::UserInfo("x".into()),
        AuthHelperError::Logout("x".into()),
        AuthHelperError::TwoFactorSms("x".into()),
        AuthHelperError::TwoFactorNotification("x".into()),
        AuthHelperError::TwoFactorCode("x".into()),
    ] {
        assert_eq!(category(error), "auth");
    }
    assert_eq!(
        category(DownloadHelperError::GetFileLink("x".into())),
        "api"
    );
    assert_eq!(
        category(DownloadHelperError::DownloadBytes("x".into())),
        "api"
    );
    assert_eq!(category(CreateFolderHelperError::NotAuthenticated), "auth");
    assert_eq!(
        category(CreateFolderHelperError::EmptyName),
        "invalid_input"
    );
    assert_eq!(category(CreateFolderHelperError::Api("x".into())), "api");
    assert_eq!(category(MountHelperError::NotAuthenticated), "auth");
    assert_eq!(
        category(FileMutationHelperError::DeleteFailed("x".into())),
        "api"
    );
    assert_eq!(
        category(FileMutationHelperError::RenameFailed("x".into())),
        "api"
    );
    assert_eq!(
        category(FileMutationHelperError::StatFailed("x".into())),
        "api"
    );

    assert_eq!(category(UploadError::NotStarted), "invalid_input");
    assert_eq!(
        category(UploadError::Io(std::io::Error::other("x"))),
        "local_io"
    );
    assert_eq!(
        category(UploadError::Helper(UploadHelperError::Create("x".into()))),
        "api"
    );
    assert_eq!(category(UploadError::InvalidState("x")), "invalid_input");
    assert_eq!(category(UploadError::Journal("x".into())), "local_io");
    assert_eq!(
        category(UploadError::HashMismatch {
            expected: "a".into(),
            actual: "b".into(),
        }),
        "api"
    );
    assert_eq!(
        category(EmbeddedDaemonError::Bootstrap(
            pcloud_daemon::BootstrapError::Provision(std::io::Error::other("x"))
        )),
        "config"
    );
}

struct CoveragePlugin;

impl Plugin for CoveragePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "coverage-plugin".into(),
            version: "1.0.0".into(),
            display_name: "Coverage Plugin".into(),
            requested_capabilities: [PluginCapability::SyncControl].into_iter().collect(),
        }
    }

    fn on_load(&mut self, _context: &PluginContext) -> Result<(), PluginError> {
        Ok(())
    }
}

struct RejectedPlugin;

impl Plugin for RejectedPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: String::new(),
            version: "1.0.0".into(),
            display_name: "Rejected Plugin".into(),
            requested_capabilities: Default::default(),
        }
    }

    fn on_load(&mut self, _context: &PluginContext) -> Result<(), PluginError> {
        Ok(())
    }
}

#[test]
fn embedded_plugin_registration_and_authorization_audit_all_outcomes() {
    let root = tempfile::Builder::new()
        .prefix("pcloud-sdk-coverage-plugin-audit-")
        .tempdir()
        .unwrap();
    let mut policy =
        pcloud_config::extensions::ExtensionPolicy::secure_defaults(root.path().join("plugins"));
    policy.plugins_enabled = true;
    policy.allow_sync_control_capability = true;
    let mut daemon = EmbeddedDaemon::builder(root.path().to_path_buf())
        .environment(Environment::Development)
        .extension_policy(policy)
        .build()
        .unwrap();
    let mut plugin = CoveragePlugin;
    assert_eq!(
        daemon.register_plugin(&mut plugin).unwrap().manifest.id,
        "coverage-plugin"
    );
    assert_eq!(daemon.loaded_plugins().len(), 1);
    assert!(
        daemon
            .authorize_plugin_operation(
                "coverage-plugin",
                &PluginOperation::RequestSyncResume { sync_root_id: 1 }
            )
            .is_ok()
    );
    assert!(
        daemon
            .authorize_plugin_operation("coverage-plugin", &PluginOperation::ObserveHealth)
            .is_err()
    );
    assert!(
        daemon
            .authorize_plugin_operation(
                "missing-plugin",
                &PluginOperation::RequestSyncResume { sync_root_id: 1 }
            )
            .is_err()
    );
    let mut rejected = RejectedPlugin;
    assert!(daemon.register_plugin(&mut rejected).is_err());
}
