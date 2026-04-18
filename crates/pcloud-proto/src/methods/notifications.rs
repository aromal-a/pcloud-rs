//! Notifications method request types.
//!
//! Mirrors the C surface declared in `pclsync/psynclib.h`:
//!
//! * `psync_notification_list_t *psync_get_notifications()`
//!   (pclsync/psynclib.c:248, implementation via `pnotify_get` in
//!   pclsync/pnotify.c:288 - the C client consumes notifications pushed via
//!   the long-poll diff stream; the Rust rewrite fetches them on-demand via
//!   the dedicated `listnotifications` endpoint, matching the pCloud public
//!   API shape used by all other first-party clients.)
//! * `int psync_mark_notificaitons_read(uint32_t notificationid)`
//!   (pclsync/psynclib.c:324 - issues a `readnotifications` command with the
//!   `notificationid` parameter.)
//!
//! Wire-level notes:
//!
//! * The C implementation in `psync_mark_notificaitons_read` uses the
//!   parameter name `notificationid` verbatim - we preserve that exact name.
//! * The C entry point keeps the historical typo `notificaitons_read`
//!   (sic); the Rust identifier uses the correct spelling
//!   `mark_notifications_read` but the wire command is `readnotifications`
//!   as declared by the C source (pclsync/psynclib.c:327).

// **PLATFORM:** all
// **GATING:** none (portable).

use crate::binary_api::BinaryParam;
use crate::methods::ProtocolMethod;
use crate::redacted::RedactedProtoString;

/// Parameters for the `listnotifications` method used by
/// `psync_get_notifications`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListNotificationsRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// Optional thumbnail size hint (matches the C
    /// `pnotify_set_callback(... thumbsize)` contract). Passed verbatim to
    /// the backend as the `thumbsize` parameter when present.
    pub thumb_size: Option<String>,
}

impl ListNotificationsRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "listnotifications"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let cap = 2 + usize::from(self.thumb_size.is_some());
        let mut params = Vec::with_capacity(cap);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::string("timeformat", "timestamp"));
        if let Some(size) = self.thumb_size.as_deref() {
            params.push(BinaryParam::string("thumbsize", size));
        }
        params
    }
}

impl ProtocolMethod for ListNotificationsRequest {
    fn command_name(&self) -> &'static str {
        ListNotificationsRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        ListNotificationsRequest::params(self)
    }
}

/// Parameters for the `readnotifications` method used by
/// `psync_mark_notificaitons_read` (pclsync/psynclib.c:324).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkNotificationsReadRequest {
    /// The `auth_token` field (auth token).
    pub auth_token: RedactedProtoString,
    /// Highest notification id that should be marked read. Matches the C
    /// parameter name `notificationid`.
    pub notification_id: u64,
}

impl MarkNotificationsReadRequest {
    /// `command_name` — command name.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        "readnotifications"
    }

    /// `params` — params.
    ///
    /// # Errors
    ///
    /// Returns a typed error on transport failure or malformed response.
    #[must_use]
    pub fn params(&self) -> Vec<BinaryParam> {
        let mut params = Vec::with_capacity(2);
        params.push(BinaryParam::string("auth", self.auth_token.expose_secret()));
        params.push(BinaryParam::number("notificationid", self.notification_id));
        params
    }
}

impl ProtocolMethod for MarkNotificationsReadRequest {
    fn command_name(&self) -> &'static str {
        MarkNotificationsReadRequest::command_name(self)
    }

    fn params(&self) -> Vec<BinaryParam> {
        MarkNotificationsReadRequest::params(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_notifications_encodes_with_auth_and_timeformat() {
        let request = ListNotificationsRequest {
            auth_token: "token".into(),
            thumb_size: None,
        };
        let encoded = request.encode().expect("listnotifications should encode");
        assert_eq!(encoded.frame.command, "listnotifications");
        assert_eq!(encoded.frame.parameter_count, 2);
    }

    #[test]
    fn list_notifications_includes_thumb_size_when_present() {
        let request = ListNotificationsRequest {
            auth_token: "token".into(),
            thumb_size: Some("128x128".to_owned()),
        };
        let encoded = request
            .encode()
            .expect("listnotifications with thumbsize should encode");
        assert_eq!(encoded.frame.command, "listnotifications");
        assert_eq!(encoded.frame.parameter_count, 3);
    }

    #[test]
    fn mark_notifications_read_encodes_notificationid() {
        let request = MarkNotificationsReadRequest {
            auth_token: "token".into(),
            notification_id: 42,
        };
        let encoded = request.encode().expect("readnotifications should encode");
        assert_eq!(encoded.frame.command, "readnotifications");
        assert_eq!(encoded.frame.parameter_count, 2);
    }
}
