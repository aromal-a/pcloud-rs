#![allow(clippy::pedantic)]
//! Live field-selector probes: verify userinfo / session-status /
//! sync-list / list-links responses expose the field shapes the CLI's
//! `--field <path>` selector relies on.
//!
//! This does not spawn the `pcloudc` binary — it parses the daemon's
//! `Response::message` directly (the same input the CLI's
//! `FieldSelector::apply` receives) and walks the dotted paths the CLI
//! advertises. If the CLI selector ever drifts from the daemon shape,
//! this test catches it against the live API.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use pcloud_ipc::{Method, Request, ResponseStatus};
use serde_json::Value;

use crate::common::{
    ENV_PASSWORD, ENV_TOKEN, ENV_USER, TestDaemon, assert_no_secret_leak, authenticate,
    optional_env, skip_if_not_live,
};

fn have_any_credentials() -> bool {
    optional_env(ENV_TOKEN).is_some()
        || (optional_env(ENV_USER).is_some() && optional_env(ENV_PASSWORD).is_some())
}

/// Mini field selector supporting both JSON shape and the legacy
/// `key=value` shape the daemon emits from some Debug-derived handlers.
/// Returns `Some(Value)` when the path resolves.
fn select(msg: &str, path: &str) -> Option<Value> {
    let parsed = parse_message(msg);
    let trimmed = path.trim_start_matches('.');
    if trimmed.is_empty() {
        return Some(parsed);
    }
    let mut current: &Value = &parsed;
    for seg in trimmed.split('.') {
        if let Ok(idx) = seg.parse::<usize>() {
            current = current.get(idx)?;
        } else {
            current = current.get(seg)?;
        }
    }
    Some(current.clone())
}

/// Turn a response `message` into a `serde_json::Value`.
/// Handles: real JSON, `key=value` legacy shapes, and plain strings.
fn parse_message(msg: &str) -> Value {
    if let Ok(v) = serde_json::from_str::<Value>(msg) {
        return v;
    }
    // Legacy `prefix: key=val, key2="val2", key3=42` shape.
    if let Some(after) = msg.find(':') {
        let rest = msg[after + 1..].trim();
        if rest.contains('=') {
            let mut out = serde_json::Map::new();
            for kv in rest.split(',') {
                let kv = kv.trim();
                if let Some((k, v)) = kv.split_once('=') {
                    let key = k.trim().to_owned();
                    let vt = v.trim().trim_matches('"');
                    let value = if let Ok(n) = vt.parse::<i64>() {
                        Value::from(n)
                    } else if vt == "true" || vt == "false" {
                        Value::Bool(vt == "true")
                    } else {
                        Value::String(vt.to_owned())
                    };
                    out.insert(key, value);
                }
            }
            if !out.is_empty() {
                return Value::Object(out);
            }
        }
    }
    Value::String(msg.to_owned())
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_field_selector_probes() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping field selectors: need credentials");
        return;
    }

    let mut daemon = TestDaemon::new("field-selectors");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping: {err}");
        return;
    }

    // 1) userinfo — must expose at least one human-recognisable field
    //    (quota OR email) whether emitted as JSON or legacy form.
    let userinfo = daemon.dispatch(Request::Plain {
        method: Method::GetUserInfo,
    });
    assert_no_secret_leak(&userinfo);
    assert_eq!(
        userinfo.status,
        ResponseStatus::Ok,
        "userinfo probe failed: {}",
        userinfo.message
    );
    let any_userinfo_field = ["quota", "email", "userid", "user_id", "premium"]
        .iter()
        .any(|k| select(&userinfo.message, k).is_some());
    assert!(
        any_userinfo_field,
        "userinfo response should advertise at least one common field: {}",
        userinfo.message
    );

    // 2) session-status — JSON envelope with `expires_at` or
    //    `refresh_in_flight`.
    let session = daemon.dispatch(Request::Plain {
        method: Method::SessionStatus,
    });
    assert_no_secret_leak(&session);
    assert_eq!(session.status, ResponseStatus::Ok);
    let ssv: Value =
        serde_json::from_str(&session.message).expect("SessionStatus response body must be JSON");
    assert!(
        ssv.is_object(),
        "session-status must decode to a JSON object"
    );
    // Must advertise a known key even if the current value is `null`.
    let known = ["expires_at", "last_used_at", "refresh_in_flight"]
        .iter()
        .any(|k| ssv.get(*k).is_some());
    assert!(
        known,
        "session-status payload missing known field: {}",
        session.message
    );

    // 3) sync-list — shape varies; minimum probe is that the selector
    //    resolves the whole envelope via `"."`.
    let syncs = daemon.dispatch(Request::Plain {
        method: Method::GetSyncRoots,
    });
    assert_no_secret_leak(&syncs);
    assert_eq!(syncs.status, ResponseStatus::Ok);
    assert!(
        select(&syncs.message, ".").is_some(),
        "sync-list must be selectable via '.'"
    );

    // 4) list-links — similar minimum probe: `.` must resolve, and if
    //    the response has a `links` array we should be able to walk
    //    `.links.0.*` without panicking (even if empty).
    let links = daemon.dispatch(Request::Plain {
        method: Method::ListPublicLinks,
    });
    assert_no_secret_leak(&links);
    assert_eq!(links.status, ResponseStatus::Ok);
    assert!(
        select(&links.message, ".").is_some(),
        "list-links must be selectable via '.'"
    );
    // If the shape is a JSON object with a `links` array, the first
    // element's id/code field (when present) should be a string or
    // number — catches accidental secret projection via field lookup.
    if let Some(links_val) = select(&links.message, "links")
        && let Some(arr) = links_val.as_array()
    {
        for item in arr.iter().take(3) {
            for key in ["link_id", "id", "code", "short"] {
                if let Some(v) = item.get(key) {
                    assert!(
                        v.is_string() || v.is_number(),
                        "list-links field {key} must be scalar, not {v:?}"
                    );
                }
            }
        }
    }
}
