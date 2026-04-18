//! Notifications API wrapper.
//!
//! Typed helpers for the pCloud notifications surface that backs
//! `psync_get_notifications` (pclsync/psynclib.c:248) and
//! `psync_mark_notificaitons_read` (pclsync/psynclib.c:324).
//!
//! Shape of parsed notifications follows the payload observed by
//! `pnotify_get` in pclsync/pnotify.c:288-351, with the thumbnail resolved
//! to a URL (host + path from the server hash) rather than a local staged
//! file path.
//!
//! ## Role in the request pipeline
//!
//! Wraps the pCloud `listnotifications` / `readnotifications`
//! endpoints. The daemon polls this periodically to surface
//! account-level events (share invitations, quota warnings, etc.)
//! to the UI layer. The parsed [`Notification`] struct is a
//! user-facing type and every field carries a short, stable name.
//!
//! ## Security considerations
//!
//! Notification text and thumbnail URLs originate from the server
//! and are untrusted. Callers that render them in a UI must
//! HTML-escape the text and must only fetch thumbnails from the
//! expected pCloud host. This module does not fetch thumbnails
//! itself.

// **PLATFORM:** all
// **GATING:** none (portable).

use thiserror::Error;

use crate::{
    ProtocolMethod,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    methods::notifications::{ListNotificationsRequest, MarkNotificationsReadRequest},
    response::{HashView, Value},
};

/// `NotificationsApi` — notifications api.
#[derive(Debug)]
pub struct NotificationsApi<T> {
    transport: T,
}

/// `NotificationsApiError` — notifications api error.
#[derive(Debug, Error)]
pub enum NotificationsApiError<E: std::error::Error + Send + Sync + 'static> {
    /// `Encode` variant (encode).
    #[error(transparent)]
    Encode(#[from] crate::FrameParseError),
    /// `Transport` variant (transport).
    #[error("transport failed: {0}")]
    Transport(E),
    /// `Result` variant (result).
    #[error("notifications method returned non-zero result code {result} ({message:?})")]
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

/// Parsed notification record mirroring `psync_notification_t` from
/// pclsync/pnotify.h. Unlike the C struct, `thumbnail_url` is a resolved URL
/// rather than a locally staged filesystem path - the Rust rewrite does not
/// download and cache thumbnails as a side-effect of listing notifications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// Server-side unique id (`notificationid`).
    pub id: u64,
    /// Localized notification text body (`notification`).
    pub text: String,
    /// Resolved thumbnail URL built from the server-provided host + path,
    /// when a `thumb` hash is present in the payload. `None` when the
    /// notification has no thumbnail.
    pub thumbnail_url: Option<String>,
    /// Notification creation time (`mtime`, unix epoch seconds).
    pub created_at: u64,
    /// Whether the notification has already been read. This is the logical
    /// inverse of the wire `isnew` flag, so callers see `read == true` for
    /// already-dismissed notifications.
    pub read: bool,
}

impl<T> NotificationsApi<T> {
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

impl<T> NotificationsApi<T>
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

    /// `list_notifications` — list notifications.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn list_notifications(
        &self,
        auth_token: impl Into<String>,
        thumb_size: Option<String>,
    ) -> Result<Vec<Notification>, NotificationsApiError<T::Error>> {
        let request = ListNotificationsRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            thumb_size,
        };
        let response = self
            .transport
            .execute(&request.encode()?)
            .map_err(NotificationsApiError::Transport)?;
        let hash = response.as_hash().ok_or(NotificationsApiError::Malformed(
            "listnotifications response was not a hash",
        ))?;
        expect_ok_result(hash)?;
        let entries = hash
            .get_array("notifications")
            .ok_or(NotificationsApiError::Malformed(
                "listnotifications missing notifications array",
            ))?;
        entries.iter().map(parse_notification::<T::Error>).collect()
    }

    /// `mark_notifications_read` — mark notifications read.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    pub fn mark_notifications_read(
        &self,
        auth_token: impl Into<String>,
        upto_id: u64,
    ) -> Result<(), NotificationsApiError<T::Error>> {
        let request = MarkNotificationsReadRequest {
            auth_token: crate::redacted::RedactedProtoString::from(auth_token.into()),
            notification_id: upto_id,
        };
        let encoded = request.encode()?;
        let response = self
            .transport
            .execute(&encoded)
            .map_err(NotificationsApiError::Transport)?;
        let hash = response.as_hash().ok_or(NotificationsApiError::Malformed(
            "readnotifications response was not a hash",
        ))?;
        expect_ok_result(hash)
    }
}

fn parse_notification<E>(value: &Value) -> Result<Notification, NotificationsApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let hash = value.as_hash().ok_or(NotificationsApiError::Malformed(
        "notification entry was not a hash",
    ))?;
    let id = hash
        .get_number("notificationid")
        .ok_or(NotificationsApiError::Malformed(
            "notification missing notificationid",
        ))?;
    let text = hash
        .get_string("notification")
        .ok_or(NotificationsApiError::Malformed(
            "notification missing notification text",
        ))?
        .to_owned();
    let created_at = hash.get_number("mtime").unwrap_or(0);
    // `isnew == true` means the notification is unread; invert so the Rust
    // surface exposes a positive `read` flag.
    let read = !hash.get_bool("isnew").unwrap_or(false);
    let thumbnail_url = resolve_thumbnail_url(&hash);
    Ok(Notification {
        id,
        text,
        thumbnail_url,
        created_at,
        read,
    })
}

fn resolve_thumbnail_url(hash: &HashView<'_>) -> Option<String> {
    let thumb = hash.get_hash("thumb")?;
    let path = thumb.get_string("path")?;
    // The C client iterates `hosts` and picks the first. Do the same.
    let host = thumb
        .get_array("hosts")
        .and_then(|hosts| hosts.iter().find_map(Value::as_string))?;
    // `path` is already absolute (`/...`). Compose an https URL, matching
    // the TLS-only transport policy enforced by the daemon runtime.
    Some(format!("https://{host}{path}"))
}

fn expect_ok_result<E>(hash: HashView<'_>) -> Result<(), NotificationsApiError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let result = hash.get_number("result").unwrap_or(0);
    if result == 0 {
        return Ok(());
    }
    Err(NotificationsApiError::Result {
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

    use super::{NotificationsApi, NotificationsApiError};

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

    fn notif(id: u64, text: &str, is_new: bool, thumb: Option<Value>) -> Value {
        let mut entries = vec![
            ("notificationid".to_owned(), Value::Number(id)),
            ("notification".to_owned(), Value::String(text.to_owned())),
            ("mtime".to_owned(), Value::Number(1_700_000_000 + id)),
            ("isnew".to_owned(), Value::Bool(is_new)),
            ("iconid".to_owned(), Value::Number(1)),
            ("action".to_owned(), Value::String("openurl".to_owned())),
            (
                "url".to_owned(),
                Value::String("https://www.pcloud.com/".to_owned()),
            ),
        ];
        if let Some(thumb) = thumb {
            entries.push(("thumb".to_owned(), thumb));
        }
        Value::Hash(entries)
    }

    #[test]
    fn list_notifications_parses_entries_and_thumbnail_url() {
        let thumb = Value::Hash(vec![
            (
                "path".to_owned(),
                Value::String("/cache/thumb-1.jpg".to_owned()),
            ),
            (
                "hosts".to_owned(),
                Value::Array(vec![Value::String("p-cf.pcloud.com".to_owned())]),
            ),
        ]);
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            (
                "notifications".to_owned(),
                Value::Array(vec![
                    notif(10, "Welcome", true, None),
                    notif(11, "Shared with you", false, Some(thumb)),
                ]),
            ),
        ])]);
        let api = NotificationsApi::new(transport);
        let list = api
            .list_notifications("token", None)
            .expect("list should succeed");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, 10);
        assert_eq!(list[0].text, "Welcome");
        assert!(!list[0].read);
        assert!(list[0].thumbnail_url.is_none());
        assert_eq!(list[1].id, 11);
        assert!(list[1].read);
        assert_eq!(
            list[1].thumbnail_url.as_deref(),
            Some("https://p-cf.pcloud.com/cache/thumb-1.jpg")
        );
    }

    #[test]
    fn list_notifications_rejects_error_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2000)),
            (
                "error".to_owned(),
                Value::String("log in failed".to_owned()),
            ),
        ])]);
        let api = NotificationsApi::new(transport);
        let err = api
            .list_notifications("token", None)
            .expect_err("non-zero result should fail");
        assert!(matches!(
            err,
            NotificationsApiError::Result { result: 2000, .. }
        ));
    }

    #[test]
    fn mark_notifications_read_accepts_ok_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![(
            "result".to_owned(),
            Value::Number(0),
        )])]);
        let api = NotificationsApi::new(transport);
        api.mark_notifications_read("token", 42)
            .expect("mark read should succeed");
    }

    #[test]
    fn mark_notifications_read_rejects_error_result() {
        let transport = MockTransport::with_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2000)),
            (
                "error".to_owned(),
                Value::String("log in failed".to_owned()),
            ),
        ])]);
        let api = NotificationsApi::new(transport);
        let err = api
            .mark_notifications_read("token", 42)
            .expect_err("non-zero result should fail");
        assert!(matches!(
            err,
            NotificationsApiError::Result { result: 2000, .. }
        ));
    }
}
