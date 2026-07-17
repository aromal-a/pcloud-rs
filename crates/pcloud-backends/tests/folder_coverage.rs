//! Public folder-runtime coverage over the deterministic development transport.

use pcloud_backends::folder_backend::FolderRuntime;
use pcloud_config::{ConfigProfile, Environment, api::ApiMode};
use pcloud_proto::folder_api::FolderApiError;
use pcloud_secret::secret_string::SecretString;

fn development_runtime() -> FolderRuntime {
    let root = tempfile::tempdir().expect("temporary config root");
    let config =
        ConfigProfile::secure_defaults(root.path().to_path_buf(), Environment::Development);
    FolderRuntime::from_config(&config)
}

fn token() -> SecretString {
    SecretString::new("coverage-token")
}

#[test]
fn development_folder_runtime_covers_listing_creation_and_validation_edges() {
    let runtime = development_runtime();
    runtime.apply_api_server_hint("bineapi-development.example");

    let root = runtime
        .list_folder_contents(token(), "/")
        .expect("development root should list");
    assert_eq!(root.folder_id, 0);
    assert_eq!(root.entries.len(), 2);
    assert!(runtime.list_folder_contents(token(), "/missing").is_err());

    let existing = runtime
        .create_remote_folder(token(), 11, "ordinary")
        .expect("ordinary folder should be created");
    assert_eq!(existing.folder_id, 123);
    assert_eq!(existing.parent_folder_id, Some(11));

    assert!(matches!(
        runtime
            .create_remote_folder(token(), 11, "")
            .expect_err("an empty leaf name must be rejected"),
        FolderApiError::Result { result: 2003, .. }
    ));
    assert!(matches!(
        runtime
            .create_remote_folder_by_path(token(), "")
            .expect_err("an empty path must be rejected"),
        FolderApiError::Result { result: 2005, .. }
    ));
    assert!(matches!(
        runtime
            .create_remote_folder_by_path(token(), "/conflict")
            .expect_err("development conflict should be deterministic"),
        FolderApiError::Result { result: 2004, .. }
    ));

    let (suffixed, suffix) = runtime
        .check_and_create_folder(token(), 11, "")
        .expect("suffix retry should recover from an empty bare name");
    assert_eq!(suffix, 2);
    assert_eq!(suffixed.name, " 2");
}

#[test]
fn development_folder_runtime_surfaces_unsupported_mutations() {
    let runtime = development_runtime();
    assert!(
        runtime
            .rename_folder_by_id(token(), 1, 2, "renamed")
            .is_err()
    );
    assert!(runtime.delete_folder_by_id(token(), 1, false).is_err());
    assert!(runtime.delete_folder_by_id(token(), 1, true).is_err());
}

#[test]
fn folder_runtime_constructs_every_configured_transport_mode_without_io() {
    for mode in [ApiMode::Plaintext, ApiMode::Tls] {
        let root = tempfile::tempdir().expect("temporary config root");
        let mut config =
            ConfigProfile::secure_defaults(root.path().to_path_buf(), Environment::Development);
        config.api.mode = mode;
        let runtime = FolderRuntime::from_config(&config);
        runtime.apply_api_server_hint("bineapi-configured.example");
    }
}
