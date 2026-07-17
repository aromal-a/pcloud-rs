//! Public fleet-agent error and value-surface coverage.

use std::{fs, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use pcloud_fleet::{
    FleetAgent, FleetCommand, FleetError, FleetIdentity, FleetResponse, MtlsFleetAgent,
    MtlsFleetConfig, NullFleetAgent,
};

fn config(root: &std::path::Path) -> MtlsFleetConfig {
    MtlsFleetConfig {
        server_url: "https://fleet.coverage.example".to_owned(),
        device_group: "coverage".to_owned(),
        identity_path: root.join("identity.json"),
        ca_bundle_path: root.join("ca.pem"),
        trusted_server_keys: Vec::new(),
        request_timeout: Some(Duration::from_millis(50)),
    }
}

#[test]
fn malformed_identity_files_return_typed_errors_without_panicking() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("identity.json");

    fs::write(&path, "{not-json").unwrap();
    assert!(matches!(
        FleetIdentity::new_or_load(&path),
        Err(FleetError::Encode(_))
    ));

    fs::write(
        &path,
        serde_json::json!({
            "private_key": "*not-base64*",
            "public_key": "",
            "device_id": "device"
        })
        .to_string(),
    )
    .unwrap();
    assert!(matches!(
        FleetIdentity::new_or_load(&path),
        Err(FleetError::Encode(_))
    ));

    fs::write(
        &path,
        serde_json::json!({
            "private_key": B64.encode([1_u8, 2, 3]),
            "public_key": "",
            "device_id": "device"
        })
        .to_string(),
    )
    .unwrap();
    assert!(matches!(
        FleetIdentity::new_or_load(&path),
        Err(FleetError::Encode(_))
    ));

    assert!(matches!(
        FleetIdentity::new_or_load(root.path()),
        Err(FleetError::Io(_))
    ));

    let blocked_parent = root.path().join("ordinary-file");
    fs::write(&blocked_parent, b"x").unwrap();
    assert!(matches!(
        FleetIdentity::new_or_load(blocked_parent.join("identity.json")),
        Err(FleetError::Io(_))
    ));
}

#[test]
fn fleet_config_and_null_agent_cover_all_command_shapes() {
    let root = tempfile::tempdir().unwrap();
    let mut invalid = config(root.path());
    invalid.server_url.clear();
    assert!(matches!(
        MtlsFleetAgent::new(invalid),
        Err(FleetError::Config(_))
    ));

    let null = NullFleetAgent::new();
    null.heartbeat().unwrap();
    assert!(matches!(
        null.handle_command(FleetCommand::RunDoctor).unwrap(),
        FleetResponse::DoctorReport { .. }
    ));
    for command in [
        FleetCommand::Reconfigure(serde_json::json!({"enabled": true})),
        FleetCommand::Upgrade {
            target_version: "2.0.0".to_owned(),
            signature: vec![1, 2, 3],
        },
        FleetCommand::Quarantine,
        FleetCommand::Unregister,
    ] {
        assert!(matches!(
            null.handle_command(command).unwrap(),
            FleetResponse::Applied
        ));
    }
}
