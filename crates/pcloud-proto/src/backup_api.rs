//! Backup / device API wrapper.
//!
//! This module exposes typed helpers for the pCloud `backup/*` endpoints that
//! back `psync_create_backup`, `psync_delete_backup`, `psync_stop_device` and
//! the post-web `psync_delete_backup_device` reconciliation in
//! `pclsync/psynclib.c`.
//!
//! ## Role in the request pipeline
//!
//! Wraps the pCloud `backup/create`, `backup/delete`, and device
//! lifecycle endpoints. Each method encodes a typed request,
//! dispatches it through the supplied transport, and projects the
//! response into [`CreatedBackup`] or a success / error envelope.
//! Unlike the C client, the Rust path intentionally does *not*
//! auto-register or auto-remove local sync roots as a side-effect
//! of these operations — the caller drives that explicitly.
//!
//! ## Security considerations
//!
//! Device identifiers and backup ids are server-issued; callers
//! must not trust ids supplied by untrusted sources. `stop_device`
//! is destructive (it revokes the session's refresh capability) and
//! must only be invoked after user confirmation.

// **PLATFORM:** all
// **GATING:** none (portable).

use thiserror::Error;

use crate::{
    ProtocolMethod,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    methods::backup::{CreateBackupRequest, StopBackupRequest, StopDeviceRequest},
    response::HashView,
};

/// `BackupApi` — backup api.
#[derive(Debug)]
pub struct BackupApi<T> {
    transport: T,
}

/// `BackupApiError` — backup api error.
#[derive(Debug, Error)]
pub enum BackupApiError<E: std::error::Error + Send + Sync + 'static> {
    /// `Encode` variant (encode).
    #[error(transparent)]
    Encode(#[from] crate::FrameParseError),
    /// `Transport` variant (transport).
    #[error("transport failed: {0}")]
    Transport(E),
    /// `Result` variant (result).
    #[error("backup method returned non-zero result code {result} ({message:?})")]
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

/// Describes the backup that has just been created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedBackup {
    /// Remote folder id of the newly-created backup folder.
    pub folder_id: u64,
    /// Parent folder id reported by the backend (typically the device root).
    pub parent_folder_id: Option<u64>,
    /// Remote name reported by the backend; useful for audit trails.
    pub name: Option<String>,
}

impl<T> BackupApi<T> {
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

impl<T> BackupApi<T>
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

    /// `create_backup` — create backup.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn create_backup(
        &self,
        auth_token: impl Into<String>,
        name: impl Into<String>,
        backup_root_folder_id: u64,
        parent_folder_name: Option<String>,
    ) -> Result<CreatedBackup, BackupApiError<T::Error>> {
        let request = CreateBackupRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            name: name.into(),
            backup_root_folder_id,
            parent_folder_name,
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(BackupApiError::Transport)?;
        let hash = response.as_hash().ok_or(BackupApiError::Malformed(
            "createbackup response was not a hash",
        ))?;
        expect_ok_result(hash)?;

        let metadata = hash
            .get_hash("metadata")
            .ok_or(BackupApiError::Malformed("createbackup missing metadata"))?;
        let folder_id = metadata
            .get_number("folderid")
            .ok_or(BackupApiError::Malformed(
                "createbackup metadata missing folderid",
            ))?;
        let parent_folder_id = metadata.get_number("parentfolderid");
        let name = metadata.get_string("name").map(ToOwned::to_owned);
        Ok(CreatedBackup {
            folder_id,
            parent_folder_id,
            name,
        })
    }

    /// `stop_backup` — stop backup.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn stop_backup(
        &self,
        auth_token: impl Into<String>,
        folder_id: u64,
    ) -> Result<(), BackupApiError<T::Error>> {
        let request = StopBackupRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            folder_id,
        };
        execute_unit(
            &self.transport,
            &request,
            "stopbackup response was not a hash",
        )
    }

    /// `stop_device` — stop device.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn stop_device(
        &self,
        auth_token: impl Into<String>,
        device_folder_id: u64,
    ) -> Result<(), BackupApiError<T::Error>> {
        let request = StopDeviceRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            device_folder_id,
        };
        execute_unit(
            &self.transport,
            &request,
            "stopdevice response was not a hash",
        )
    }
}

fn execute_unit<T, M>(
    transport: &T,
    request: &M,
    malformed_message: &'static str,
) -> Result<(), BackupApiError<T::Error>>
where
    T: ProtocolTransport + ApiServerHintConsumer,
    M: ProtocolMethod,
{
    let encoded = request.encode()?;
    let response = transport
        .execute(&encoded)
        .map_err(BackupApiError::Transport)?;
    let hash = response
        .as_hash()
        .ok_or(BackupApiError::Malformed(malformed_message))?;
    expect_ok_result(hash)?;
    Ok(())
}

fn expect_ok_result<E>(hash: HashView<'_>) -> Result<(), BackupApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let result = hash.get_number("result").unwrap_or(0);
    if result == 0 {
        return Ok(());
    }
    Err(BackupApiError::Result {
        result,
        message: hash.get_string("error").map(ToOwned::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use std::{io, sync::Mutex};

    use crate::{
        auth_api::{ApiServerHintConsumer, ProtocolTransport},
        response::Value,
    };

    use super::{BackupApi, BackupApiError};

    #[derive(Debug)]
    struct MockTransport {
        responses: Mutex<Vec<Value>>,
    }

    impl MockTransport {
        fn with_responses(responses: Vec<Value>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().rev().collect()),
            }
        }
    }

    impl ProtocolTransport for MockTransport {
        type Error = io::Error;

        fn execute(&self, _request: &crate::EncodedRequest) -> Result<Value, Self::Error> {
            self.responses
                .lock()
                .expect("responses lock should not be poisoned")
                .pop()
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "missing response"))
        }
    }

    impl ApiServerHintConsumer for MockTransport {
        fn apply_api_server_hint(&self, _api_server: &str) {}
    }

    #[test]
    fn create_backup_parses_folder_metadata() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            (
                "metadata".to_owned(),
                Value::Hash(vec![
                    ("folderid".to_owned(), Value::Number(111)),
                    ("parentfolderid".to_owned(), Value::Number(9)),
                    ("name".to_owned(), Value::String("Documents".to_owned())),
                ]),
            ),
        ])]);
        let api = BackupApi::new(transport);
        let created = api
            .create_backup("token", "Documents", 9, None)
            .expect("create backup should succeed");
        assert_eq!(created.folder_id, 111);
        assert_eq!(created.parent_folder_id, Some(9));
        assert_eq!(created.name.as_deref(), Some("Documents"));
    }

    #[test]
    fn create_backup_rejects_error_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2002)),
            (
                "error".to_owned(),
                Value::String("invalid backup root".to_owned()),
            ),
        ])]);
        let api = BackupApi::new(transport);
        let err = api
            .create_backup("token", "Documents", 0, None)
            .expect_err("result != 0 should fail");
        assert!(matches!(err, BackupApiError::Result { result: 2002, .. }));
    }

    #[test]
    fn stop_backup_handles_success_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![(
            "result".to_owned(),
            Value::Number(0),
        )])]);
        let api = BackupApi::new(transport);
        api.stop_backup("token", 5)
            .expect("stop backup should succeed");
    }

    #[test]
    fn stop_device_rejects_error_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2005)),
            (
                "error".to_owned(),
                Value::String("unknown device".to_owned()),
            ),
        ])]);
        let api = BackupApi::new(transport);
        let err = api
            .stop_device("token", 123)
            .expect_err("stop device should fail on non-zero result");
        assert!(matches!(err, BackupApiError::Result { result: 2005, .. }));
    }
}
