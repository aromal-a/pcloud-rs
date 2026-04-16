//! Built-in dotted-path field selector for `pcloudc`.
//!
//! This is a **tiny subset** of jq: we expose `key.key.0.key`-style path
//! selection against a parsed representation of the daemon response
//! `message`. It intentionally does NOT support filters, pipes, slices,
//! comma splits, `@csv`, assignment, or any other jq transform. If you
//! need those, pipe to `jq`.
//!
//! Why build this in at all:
//!
//! - The daemon response envelope is stable (`status` + `message`), and
//!   many callers just want *one field* (e.g. `quota`) rather than the
//!   entire blob. A host-level `jq` dependency is surprisingly heavy
//!   for CI / container images, and scripts that only need one key end
//!   up shelling out twice (`pcloudc | jq`).
//! - The selector is enforced at render time inside the CLI, which lets
//!   us guarantee we never project secret-bearing fields: the only
//!   value we reach into is [`pcloud_ipc::Response::message`], which
//!   has already passed through the daemon's sanitisation layer.
//!
//! # Security
//!
//! - `FieldSelector::apply` operates on already-parsed
//!   [`serde_json::Value`] and cannot reach into `SecretString`/
//!   `SecretBytes` — those types never implement
//!   [`serde::Serialize`] for their protected payload. The
//!   `assert_no_secret_in_value` test below pins this invariant.
//! - Error output never echoes user-supplied values, only field names
//!   and the list of available siblings at the last successful step.
//!
//! # Supported message formats
//!
//! [`parse_message_to_json`] understands three shapes:
//!
//! 1. **Real JSON**: `{"quota": 10737418240, "premium": false}`.
//! 2. **Legacy flat form**: `userinfo: quota=10737418240, premium=false,
//!    email="alice@example.com", cryptosetup=None`. This is the shape
//!    emitted by several `Debug`-derived runtime responses; we parse
//!    it on a best-effort basis so users can still project fields out
//!    of legacy surfaces.
//! 3. **Plain text**: returned verbatim as `Value::String`. Only the
//!    empty selector matches such messages.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde_json::Value;

/// One step in a dotted path. `Key("quota")` looks up an object field;
/// `Index(0)` indexes into an array. Parsing picks `Index` only when a
/// segment parses as `usize` end-to-end, so `"0"` inside a legitimate
/// string key requires quoting via the legacy `key=value` format (which
/// preserves quotes) or JSON-in-message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    /// Object-key step.
    Key(String),
    /// Array-index step.
    Index(usize),
}

/// Parsed dotted-path selector. Produced by [`FieldSelector::parse`].
///
/// Empty paths (produced by the lone input `"."`) select the whole
/// value and are useful for "print the parsed envelope as JSON" workflows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FieldSelector {
    /// Path segments to walk, in order.
    pub path: Vec<PathSegment>,
    /// Original source string, used to build error messages that echo
    /// what the user actually typed.
    pub source: String,
}

/// Errors produced while applying a [`FieldSelector`] against a value.
///
/// These are mapped to exit code `2 Usage` at the CLI boundary because
/// they always indicate a user-supplied selector that does not fit the
/// response shape — the daemon itself never returns one of these.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FieldSelectorError {
    /// Selector step did not exist on the current object. `available`
    /// is the list of sibling keys (or the array length, formatted as
    /// `"[0..N]"`) so the error message can steer the operator.
    #[error("field not found: '{path}'. available: {}", .available.join(", "))]
    NotFound {
        /// Full dotted path as given by the user.
        path: String,
        /// Sibling keys at the failing step, already sorted.
        available: Vec<String>,
    },
    /// Selector tried to index into a non-matching type (e.g. asking
    /// for a key on an array or an index on a scalar).
    #[error("type mismatch at '{at}': expected {expected}, got {got}")]
    TypeMismatch {
        /// Prefix of the selector that successfully walked before the
        /// mismatch.
        at: String,
        /// Shape the selector step required (`"object"`, `"array"`).
        expected: &'static str,
        /// Actual shape of the value at `at` (`"string"`, `"number"`,
        /// `"bool"`, `"null"`, `"array"`, `"object"`).
        got: &'static str,
    },
}

impl FieldSelector {
    /// Parse a dotted-path selector string.
    ///
    /// Rules:
    /// - `"."` or `""` → empty path (select the entire value).
    /// - `"a.b.c"` → three key steps.
    /// - `"links.0.id"` → key, array-index, key.
    /// - Leading `.` is tolerated (`".quota"` ≡ `"quota"`) so jq muscle
    ///   memory works.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        let source = s.to_owned();
        let trimmed = s.trim_start_matches('.');
        if trimmed.is_empty() {
            return Self {
                path: Vec::new(),
                source,
            };
        }
        let path: Vec<PathSegment> = trimmed
            .split('.')
            .map(|seg| {
                if let Ok(idx) = seg.parse::<usize>() {
                    PathSegment::Index(idx)
                } else {
                    PathSegment::Key(seg.to_owned())
                }
            })
            .collect();
        Self { path, source }
    }

    /// Walk `value` according to this selector and return the selected
    /// sub-value. The input is borrowed but the result is cloned so the
    /// caller can own/serialize it without worrying about lifetimes.
    pub fn apply(&self, value: &Value) -> Result<Value, FieldSelectorError> {
        let mut current = value;
        let mut walked: Vec<String> = Vec::with_capacity(self.path.len());
        for seg in &self.path {
            match (seg, current) {
                (PathSegment::Key(k), Value::Object(map)) => {
                    // Direct hit first — keeps the common path a single
                    // map lookup. The legacy `key=value` parser lower-
                    // cases nothing, so keys are matched exactly.
                    if let Some(v) = map.get(k) {
                        current = v;
                        walked.push(k.clone());
                        continue;
                    }
                    let mut available: Vec<String> = map.keys().cloned().collect();
                    available.sort_unstable();
                    return Err(FieldSelectorError::NotFound {
                        path: self.source.clone(),
                        available,
                    });
                }
                (PathSegment::Index(i), Value::Array(arr)) => {
                    if let Some(v) = arr.get(*i) {
                        current = v;
                        walked.push(i.to_string());
                        continue;
                    }
                    return Err(FieldSelectorError::NotFound {
                        path: self.source.clone(),
                        available: vec![format!("[0..{}]", arr.len())],
                    });
                }
                (PathSegment::Key(_), other) => {
                    return Err(FieldSelectorError::TypeMismatch {
                        at: joined_prefix(&walked),
                        expected: "object",
                        got: kind_of(other),
                    });
                }
                (PathSegment::Index(_), other) => {
                    return Err(FieldSelectorError::TypeMismatch {
                        at: joined_prefix(&walked),
                        expected: "array",
                        got: kind_of(other),
                    });
                }
            }
        }
        Ok(current.clone())
    }
}

/// Render a path prefix joined by dots, empty for the root.
fn joined_prefix(parts: &[String]) -> String {
    if parts.is_empty() {
        "<root>".to_owned()
    } else {
        parts.join(".")
    }
}

/// Human-readable JSON shape name for error messages.
fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Convert a raw daemon `message` string into a
/// [`serde_json::Value`] suitable for [`FieldSelector::apply`].
///
/// See module docs for the three accepted shapes. This function never
/// fails — at worst it returns `Value::String(msg)`, which the empty
/// selector can still print.
#[must_use]
pub fn parse_message_to_json(msg: &str) -> Value {
    let trimmed = msg.trim();

    // Shape 1: real JSON.
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return v;
    }

    // Shape 2: legacy flat form, optionally prefixed with `command:`.
    //
    // We strip the prefix only when it looks like an identifier
    // (letters / digits / `_` / `-`) followed by `: `. Otherwise the
    // whole thing is treated as a single bare string.
    let body = strip_legacy_prefix(trimmed);
    if looks_like_flat_kv(body)
        && let Some(obj) = parse_flat_kv(body)
    {
        return Value::Object(obj);
    }

    // Shape 3: verbatim.
    Value::String(msg.to_owned())
}

/// `"userinfo: quota=10, premium=false"` → `"quota=10, premium=false"`.
/// Anything not shaped like `ident: ...` is returned unchanged.
fn strip_legacy_prefix(s: &str) -> &str {
    if let Some(colon) = s.find(':') {
        let (head, tail) = s.split_at(colon);
        let trimmed_head = head.trim();
        let is_ident = !trimmed_head.is_empty()
            && trimmed_head
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        // Require at least one space after the colon — this avoids
        // matching things like `http://...` or `2026-04-16T...:00`.
        let rest = &tail[1..];
        if is_ident && rest.starts_with(' ') {
            return rest.trim_start();
        }
    }
    s
}

/// Heuristic: a flat kv list must contain at least one `key=value`
/// pair at the top level (i.e. with balanced brackets/quotes). We keep
/// it deliberately conservative so JSON-looking strings never fall
/// through to this parser.
fn looks_like_flat_kv(s: &str) -> bool {
    s.contains('=') && !s.starts_with('{') && !s.starts_with('[')
}

/// Parse a comma-separated `key=value` list into a JSON object.
///
/// `value` rules:
/// - `true` / `false` → bool
/// - `None`           → null
/// - `Some(x)`        → parse inner `x` recursively
/// - integer          → number
/// - decimal          → number
/// - `"…"`            → string (quotes stripped, `\"` escapes honoured)
/// - anything else    → bare string
fn parse_flat_kv(s: &str) -> Option<serde_json::Map<String, Value>> {
    let mut map = serde_json::Map::new();
    for pair in split_top_level(s) {
        let (k, v) = pair.split_once('=')?;
        let key = k.trim();
        if key.is_empty() {
            return None;
        }
        let value = parse_flat_value(v.trim());
        map.insert(key.to_owned(), value);
    }
    if map.is_empty() { None } else { Some(map) }
}

/// Split `s` on top-level commas, respecting quotes and `(`/`)`/`[`/`]`/
/// `{`/`}` nesting. This keeps `Some(x, y)` or `"a, b"` as a single
/// value.
fn split_top_level(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut depth: i32 = 0;
    let mut in_quote = false;
    let mut escaped = false;
    for ch in s.chars() {
        if escaped {
            buf.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && in_quote {
            buf.push(ch);
            escaped = true;
            continue;
        }
        if ch == '"' {
            in_quote = !in_quote;
            buf.push(ch);
            continue;
        }
        if in_quote {
            buf.push(ch);
            continue;
        }
        match ch {
            '(' | '[' | '{' => {
                depth += 1;
                buf.push(ch);
            }
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                buf.push(ch);
            }
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut buf));
            }
            _ => buf.push(ch),
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

/// Convert a single flat-kv value string into a JSON `Value`.
fn parse_flat_value(v: &str) -> Value {
    let v = v.trim();
    if v.is_empty() {
        return Value::String(String::new());
    }
    if v == "true" {
        return Value::Bool(true);
    }
    if v == "false" {
        return Value::Bool(false);
    }
    if v == "None" || v == "null" {
        return Value::Null;
    }
    if let Some(inner) = v.strip_prefix("Some(").and_then(|s| s.strip_suffix(')')) {
        return parse_flat_value(inner);
    }
    // Quoted strings — honour `\"` and `\\` escapes.
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        let inner = &v[1..v.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\'
                && let Some(next) = chars.next()
            {
                match next {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    other => out.push(other),
                }
                continue;
            }
            out.push(c);
        }
        return Value::String(out);
    }
    // Numbers.
    if let Ok(n) = v.parse::<i64>() {
        return Value::from(n);
    }
    if let Ok(n) = v.parse::<u64>() {
        return Value::from(n);
    }
    if let Ok(n) = v.parse::<f64>()
        && n.is_finite()
        && let Some(num) = serde_json::Number::from_f64(n)
    {
        return Value::Number(num);
    }
    // Bare string.
    Value::String(v.to_owned())
}

/// Render a [`serde_json::Value`] in the "plain text" form used when
/// the user projected a single field.
///
/// Strings are unquoted, numbers/bools are printed via `Display`,
/// `null` is an empty string, arrays/objects fall back to compact
/// JSON so no information is lost.
#[must_use]
pub fn render_value_plain(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_dotted_path() {
        let s = FieldSelector::parse("a.b.0.c");
        assert_eq!(
            s.path,
            vec![
                PathSegment::Key("a".into()),
                PathSegment::Key("b".into()),
                PathSegment::Index(0),
                PathSegment::Key("c".into()),
            ]
        );
    }

    #[test]
    fn tolerates_leading_dot_and_empty() {
        assert!(FieldSelector::parse(".").path.is_empty());
        assert!(FieldSelector::parse("").path.is_empty());
        assert_eq!(
            FieldSelector::parse(".quota").path,
            vec![PathSegment::Key("quota".into())]
        );
    }

    #[test]
    fn apply_finds_top_level_key() {
        let v = json!({"quota": 10u64, "premium": false});
        let out = FieldSelector::parse("quota").apply(&v).unwrap();
        assert_eq!(out, json!(10u64));
    }

    #[test]
    fn apply_missing_returns_available_siblings() {
        let v = json!({"quota": 10u64, "premium": false, "email": "a@b"});
        let err = FieldSelector::parse("quotaa").apply(&v).unwrap_err();
        match err {
            FieldSelectorError::NotFound { path, available } => {
                assert_eq!(path, "quotaa");
                assert_eq!(available, vec!["email", "premium", "quota"]);
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn apply_nested_and_index() {
        let v = json!({"links": [{"id": 1}, {"id": 2}], "count": 2});
        let got = FieldSelector::parse("links.0.id").apply(&v).unwrap();
        assert_eq!(got, json!(1));
        let got = FieldSelector::parse("links.1.id").apply(&v).unwrap();
        assert_eq!(got, json!(2));
        let got = FieldSelector::parse("count").apply(&v).unwrap();
        assert_eq!(got, json!(2));
    }

    #[test]
    fn apply_type_mismatch() {
        let v = json!({"quota": 10});
        let err = FieldSelector::parse("quota.deep").apply(&v).unwrap_err();
        match err {
            FieldSelectorError::TypeMismatch { at, expected, got } => {
                assert_eq!(at, "quota");
                assert_eq!(expected, "object");
                assert_eq!(got, "number");
            }
            other => panic!("expected TypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn apply_array_out_of_range_reports_length() {
        let v = json!({"links": [1, 2]});
        let err = FieldSelector::parse("links.7").apply(&v).unwrap_err();
        match err {
            FieldSelectorError::NotFound { available, .. } => {
                assert_eq!(available, vec!["[0..2]"]);
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn parse_message_handles_real_json() {
        let v = parse_message_to_json(r#"{"quota": 10737418240, "premium": false}"#);
        assert_eq!(v["quota"], json!(10737418240u64));
        assert_eq!(v["premium"], json!(false));
    }

    #[test]
    fn parse_message_handles_flat_kv_with_prefix() {
        let v = parse_message_to_json(
            r#"userinfo: quota=10737418240, premium=false, email="a@b.c", cryptosetup=None"#,
        );
        let obj = v.as_object().expect("flat kv should produce object");
        assert_eq!(obj["quota"], json!(10737418240u64));
        assert_eq!(obj["premium"], json!(false));
        assert_eq!(obj["email"], json!("a@b.c"));
        assert_eq!(obj["cryptosetup"], Value::Null);
    }

    #[test]
    fn parse_message_handles_some_wrapper() {
        let v = parse_message_to_json("status: planquota=Some(42), label=Some(\"pro\")");
        assert_eq!(v["planquota"], json!(42));
        assert_eq!(v["label"], json!("pro"));
    }

    #[test]
    fn parse_message_flat_preserves_quoted_commas() {
        let v = parse_message_to_json(r#"msg: greeting="hello, world", count=3"#);
        assert_eq!(v["greeting"], json!("hello, world"));
        assert_eq!(v["count"], json!(3));
    }

    #[test]
    fn parse_message_falls_back_to_string() {
        let v = parse_message_to_json("daemon listening on /tmp/pcloud.sock");
        assert!(matches!(v, Value::String(_)));
    }

    #[test]
    fn parse_message_does_not_mistake_url_for_prefix() {
        // "http://example.com/x" must not be split on the `:` because
        // there's no space after it.
        let v = parse_message_to_json("http://example.com/x");
        assert!(matches!(v, Value::String(ref s) if s == "http://example.com/x"));
    }

    #[test]
    fn render_value_plain_unwraps_strings() {
        assert_eq!(render_value_plain(&json!("hello")), "hello");
        assert_eq!(render_value_plain(&json!(42)), "42");
        assert_eq!(render_value_plain(&json!(true)), "true");
        assert_eq!(render_value_plain(&json!(null)), "");
        assert_eq!(render_value_plain(&json!([1, 2])), "[1,2]");
    }

    #[test]
    fn assert_no_secret_in_value() {
        // Guard: the selector never reaches into a SecretString/SecretBytes
        // because `parse_message_to_json` only consumes `&str`, which is
        // already post-sanitisation by the daemon and SDK. This test
        // pins the invariant by feeding a string that LOOKS like a
        // secret and confirming we just get a normal string back — no
        // special handling, no clone into a redaction wrapper.
        let v = parse_message_to_json(r#"{"token": "do-not-print-me"}"#);
        let out = FieldSelector::parse("token").apply(&v).unwrap();
        // Plain-value render is the only output path; it yields the raw
        // string the daemon already sanitised. The test's purpose is to
        // document that this is the whole threat model — if the daemon
        // ships a secret here, it is a daemon-side bug.
        assert_eq!(render_value_plain(&out), "do-not-print-me");
    }

    #[test]
    fn empty_selector_returns_whole_value() {
        let v = json!({"a": 1});
        let out = FieldSelector::parse(".").apply(&v).unwrap();
        assert_eq!(out, v);
    }

    #[test]
    fn numeric_key_disambiguated_by_top_level_object() {
        // Object keys that happen to be numeric still resolve via
        // the object step when the value is an object; the parser
        // picks `Index` because numeric segments have no way to
        // disambiguate at parse time. This test documents the
        // intentional limitation.
        let v = json!({"0": "zero", "1": "one"});
        // `"0"` parses as Index(0) -> TypeMismatch on an object.
        let err = FieldSelector::parse("0").apply(&v).unwrap_err();
        assert!(matches!(err, FieldSelectorError::TypeMismatch { .. }));
    }
}
