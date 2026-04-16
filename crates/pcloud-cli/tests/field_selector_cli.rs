#![allow(clippy::pedantic)]
//! Integration coverage for the built-in dotted-path field selector.
//!
//! These tests stay CLI-local — they exercise
//! [`field_selector::parse_message_to_json`] and
//! [`field_selector::FieldSelector::apply`] against realistic daemon
//! `message` payloads and confirm that both the happy path and the
//! "unknown field" path behave as documented in the manpage.
//!
//! A full end-to-end daemon test would add a large amount of scaffolding
//! for a one-line renderer; the renderer is covered by the unit tests
//! in `json_output.rs` and by the in-process `globals` tests.

#![cfg(unix)]

#[path = "../src/field_selector.rs"]
mod field_selector;

use field_selector::{
    FieldSelector, FieldSelectorError, parse_message_to_json, render_value_plain,
};

#[test]
fn userinfo_flat_kv_projects_quota() {
    let msg = r#"userinfo: quota=10737418240, usedquota=4294967296, premium=false, email="a@b.c""#;
    let parsed = parse_message_to_json(msg);
    let v = FieldSelector::parse("quota").apply(&parsed).unwrap();
    assert_eq!(render_value_plain(&v), "10737418240");
}

#[test]
fn userinfo_flat_kv_projects_multiple_in_order() {
    let msg = r#"userinfo: quota=10737418240, usedquota=4294967296, premium=false"#;
    let parsed = parse_message_to_json(msg);
    let lines: Vec<String> = ["quota", "usedquota", "premium"]
        .into_iter()
        .map(|s| render_value_plain(&FieldSelector::parse(s).apply(&parsed).unwrap()))
        .collect();
    assert_eq!(lines, vec!["10737418240", "4294967296", "false"]);
}

#[test]
fn typo_yields_not_found_with_available_siblings() {
    let msg = r#"userinfo: quota=10, usedquota=5, premium=false"#;
    let parsed = parse_message_to_json(msg);
    let err = FieldSelector::parse("quotaa").apply(&parsed).unwrap_err();
    match err {
        FieldSelectorError::NotFound { path, available } => {
            assert_eq!(path, "quotaa");
            assert!(available.iter().any(|a| a == "quota"));
            assert!(available.iter().any(|a| a == "usedquota"));
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn nested_array_and_object() {
    let msg = r#"{"links":[{"id":1,"code":"alpha"},{"id":2,"code":"beta"}],"count":2}"#;
    let parsed = parse_message_to_json(msg);
    let id = FieldSelector::parse("links.0.id").apply(&parsed).unwrap();
    assert_eq!(render_value_plain(&id), "1");
    let code = FieldSelector::parse("links.1.code").apply(&parsed).unwrap();
    assert_eq!(render_value_plain(&code), "beta");
    let count = FieldSelector::parse("count").apply(&parsed).unwrap();
    assert_eq!(render_value_plain(&count), "2");
}

#[test]
fn legacy_some_wrapper_and_quoted_commas_work() {
    let msg = r#"status: plan=Some("pro"), note="hello, world", active=true"#;
    let parsed = parse_message_to_json(msg);
    assert_eq!(
        render_value_plain(&FieldSelector::parse("plan").apply(&parsed).unwrap()),
        "pro"
    );
    assert_eq!(
        render_value_plain(&FieldSelector::parse("note").apply(&parsed).unwrap()),
        "hello, world"
    );
    assert_eq!(
        render_value_plain(&FieldSelector::parse("active").apply(&parsed).unwrap()),
        "true"
    );
}

#[test]
fn json_envelope_projection_collects_all_requested_fields() {
    // The envelope side of the projection is handled by
    // `json_output::JsonEnvelope::from_fields`. Here we confirm that
    // every requested selector ends up in the output map with the
    // correct value, regardless of the underlying map ordering.
    let msg = r#"{"a":1,"b":2,"c":3}"#;
    let parsed = parse_message_to_json(msg);
    let mut out = serde_json::Map::new();
    for path in ["b", "c", "a"] {
        let v = FieldSelector::parse(path).apply(&parsed).unwrap();
        out.insert(path.to_owned(), v);
    }
    assert_eq!(out["a"], serde_json::json!(1));
    assert_eq!(out["b"], serde_json::json!(2));
    assert_eq!(out["c"], serde_json::json!(3));
    assert_eq!(out.len(), 3);
}

#[test]
fn type_mismatch_on_scalar_child_reports_kind() {
    let msg = r#"{"n": 42}"#;
    let parsed = parse_message_to_json(msg);
    let err = FieldSelector::parse("n.sub").apply(&parsed).unwrap_err();
    match err {
        FieldSelectorError::TypeMismatch { got, expected, .. } => {
            assert_eq!(expected, "object");
            assert_eq!(got, "number");
        }
        other => panic!("expected TypeMismatch, got {other:?}"),
    }
}

#[test]
fn verbatim_string_message_empty_selector_returns_it() {
    let msg = "daemon listening on /tmp/pcloud.sock";
    let parsed = parse_message_to_json(msg);
    let v = FieldSelector::parse(".").apply(&parsed).unwrap();
    assert_eq!(render_value_plain(&v), msg);
}
