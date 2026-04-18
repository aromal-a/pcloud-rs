//! Backup/device method request types.
//!
//! These mirror the C surface declared in `pclsync/psynclib.h` around
//! `psync_create_backup`, `psync_delete_backup`, `psync_stop_device` and
//! `psync_delete_backup_device` and rely on the pCloud `backup/*` endpoints:
//!
//! * `backup/createbackup`
//! * `backup/stopbackup`
//! * `backup/stopdevice`
//!
//! Per the C implementation in `pclsync/psynclib.c`, `psync_delete_backup_device`
//! is a local-only cleanup hook invoked when a stop-device was issued from the
//! web UI and therefore does not have a dedicated backend request. It is
//! represented on the Rust side as a runtime-level operation rather than an API
//! request. The `backup/stopdevice` endpoint is used for both the
//! `psync_stop_device` flow and for the post-web stop-device reconciliation.

// **PLATFORM:** all
// **GATING:** none (portable).

use crate::binary_api::{BinaryParam, BinaryParamValue};
use crate::methods::ProtocolMethod;
use crate::redacted::RedactedProtoString;

/// Parameters for the `backup/createbackup` method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateBackupRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// Leaf folder name. The C client parses the local path and uses the
    /// final segment as this value.
    pub name: String,
    /// Backup root folder id under which the new backup folder is created.
    pub backup_root_folder_id: u64,
    /// Optional parent folder name (set when the local path has additional
    /// intermediate segments, matching the C client behaviour).
    pub parent_folder_name: Option<String>,
}

impl CreateBackupRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "backup/createbackup"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = vec![
            BinaryParam {
                name: "auth".to_owned(),
                value: BinaryParamValue::String(self.auth_token.expose_secret().to_owned()),
            },
            BinaryParam {
                name: "name".to_owned(),
                value: BinaryParamValue::String(self.name.clone()),
            },
            BinaryParam {
                name: "folderid".to_owned(),
                value: BinaryParamValue::Number(self.backup_root_folder_id),
            },
            BinaryParam {
                name: "timeformat".to_owned(),
                value: BinaryParamValue::String("timestamp".to_owned()),
            },
        ];
        if let Some(parent) = &self.parent_folder_name {
            params.push(BinaryParam {
                name: "parentfoldername".to_owned(),
                value: BinaryParamValue::String(parent.clone()),
            });
        }
        params
    }
}

impl ProtocolMethod for CreateBackupRequest {
    fn command_name(&self) -> &'static str {
        CreateBackupRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        CreateBackupRequest::params(self)
    }
}

/// Parameters for the `backup/stopbackup` method used by `psync_delete_backup`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopBackupRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// The `folder_id` field (folder id).
    pub folder_id: u64,
}

impl StopBackupRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "backup/stopbackup"
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
                name: "folderid".to_owned(),
                value: BinaryParamValue::Number(self.folder_id),
            },
        ]
    }
}

impl ProtocolMethod for StopBackupRequest {
    fn command_name(&self) -> &'static str {
        StopBackupRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        StopBackupRequest::params(self)
    }
}

/// Parameters for the `backup/stopdevice` method used by `psync_stop_device`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopDeviceRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// Device (root backup) folder id. In the C implementation this is
    /// resolved from the `BackupRootFoId` setting when the caller passes 0.
    pub device_folder_id: u64,
}

impl StopDeviceRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "backup/stopdevice"
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
                name: "folderid".to_owned(),
                value: BinaryParamValue::Number(self.device_folder_id),
            },
        ]
    }
}

impl ProtocolMethod for StopDeviceRequest {
    fn command_name(&self) -> &'static str {
        StopDeviceRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        StopDeviceRequest::params(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_backup_emits_required_parameters() {
        let request = CreateBackupRequest {
            auth_token: "token".into(),
            name: "Documents".to_owned(),
            backup_root_folder_id: 11,
            parent_folder_name: Some("Work".to_owned()),
        };
        let encoded = request.encode().expect("create backup should encode");
        assert_eq!(encoded.frame.command, "backup/createbackup");
        assert_eq!(encoded.frame.parameter_count, 5);
    }

    #[test]
    fn create_backup_without_parent_skips_parameter() {
        let request = CreateBackupRequest {
            auth_token: "token".into(),
            name: "Documents".to_owned(),
            backup_root_folder_id: 11,
            parent_folder_name: None,
        };
        let encoded = request.encode().expect("create backup should encode");
        assert_eq!(encoded.frame.parameter_count, 4);
    }

    #[test]
    fn stop_backup_and_stop_device_share_shape() {
        let stop_backup = StopBackupRequest {
            auth_token: "token".into(),
            folder_id: 42,
        };
        let stop_device = StopDeviceRequest {
            auth_token: "token".into(),
            device_folder_id: 42,
        };
        let encoded_backup = stop_backup.encode().expect("stop backup should encode");
        let encoded_device = stop_device.encode().expect("stop device should encode");
        assert_eq!(encoded_backup.frame.command, "backup/stopbackup");
        assert_eq!(encoded_device.frame.command, "backup/stopdevice");
        assert_eq!(encoded_backup.frame.parameter_count, 2);
        assert_eq!(encoded_device.frame.parameter_count, 2);
    }
}
