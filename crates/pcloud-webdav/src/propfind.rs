//! T1.6.a — minimal RFC 4918 `PROPFIND` parser + `multistatus` builder.
//!
//! # Why hand-rolled
//!
//! WebDAV bodies are XML, but the slice we actually care about is
//! tiny: the Depth header, the `<allprop>`/`<propname>`/`<prop>`
//! request shape, and a multistatus response listing one
//! `<response>` per resource. Pulling `quick-xml` would dominate the
//! crate's dep weight for a parser that only has to recognise four
//! production rules. The handful of tags we care about let us walk
//! the body once with `&str::find` / `&str::splitn` and validate the
//! shape with explicit checks.
//!
//! # Coverage
//!
//! - **Parser:** accepts `<D:propfind>` envelopes with one of:
//!   `<D:allprop/>`, `<D:propname/>`, or `<D:prop>...</D:prop>`.
//!   Returns [`PropfindRequest::AllProp`] / [`PropfindRequest::PropName`] /
//!   [`PropfindRequest::NamedProps`]. Unrecognised content yields
//!   [`PropfindError::Malformed`]. Depth comes from the HTTP header
//!   (defaulted to `infinity` when absent, per the RFC).
//! - **Renderer:** emits a `<D:multistatus>` body listing the
//!   resources passed in. Each resource carries a content-length,
//!   last-modified epoch, and a content type when known. Error
//!   responses inside the multistatus are not yet supported in this
//!   fire — callers that need to flag a 404 inline will get full
//!   coverage in T1.6.c.

use std::fmt::Write as _;

/// Parsed `PROPFIND` request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropfindRequest {
    /// `<D:propfind><D:allprop/></D:propfind>` — return every known
    /// property for each resource.
    AllProp,
    /// `<D:propfind><D:propname/></D:propfind>` — return only the
    /// names of properties (not values).
    PropName,
    /// `<D:propfind><D:prop>...</D:prop></D:propfind>` — return the
    /// listed properties for each resource. Values are the local
    /// name of each child element under `<D:prop>` (namespace
    /// prefix stripped).
    NamedProps {
        /// Property names requested.
        names: Vec<String>,
    },
}

/// Errors returned by [`parse_propfind`].
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum PropfindError {
    /// Body was empty. Per RFC 4918 §9.1 an empty body means
    /// `allprop`. The parser surfaces this so the caller can
    /// special-case if needed; [`parse_propfind_or_allprop`] does
    /// it for them.
    #[error("empty PROPFIND body")]
    Empty,
    /// Body was syntactically malformed (no `<propfind>`, unbalanced
    /// elements, etc.).
    #[error("malformed PROPFIND body: {0}")]
    Malformed(&'static str),
}

/// Parse a `PROPFIND` request body. Empty bodies return `Err(Empty)`
/// — call sites that want the RFC default should use
/// [`parse_propfind_or_allprop`].
///
/// # Errors
///
/// See [`PropfindError`].
pub fn parse_propfind(body: &str) -> Result<PropfindRequest, PropfindError> {
    let body = body.trim();
    if body.is_empty() {
        return Err(PropfindError::Empty);
    }
    if !contains_local_name(body, "propfind") {
        return Err(PropfindError::Malformed("missing <propfind>"));
    }
    if contains_local_name(body, "allprop") {
        return Ok(PropfindRequest::AllProp);
    }
    if contains_local_name(body, "propname") {
        return Ok(PropfindRequest::PropName);
    }
    if let Some(prop_inner) = extract_first_element_inner(body, "prop") {
        let names = extract_local_names(&prop_inner);
        return Ok(PropfindRequest::NamedProps { names });
    }
    Err(PropfindError::Malformed(
        "PROPFIND body must contain <allprop/>, <propname/>, or <prop>...</prop>",
    ))
}

/// Parse a `PROPFIND` body, treating an empty body as `allprop`
/// (RFC 4918 §9.1).
///
/// # Errors
///
/// Returns [`PropfindError::Malformed`] for non-empty malformed
/// bodies.
pub fn parse_propfind_or_allprop(body: &str) -> Result<PropfindRequest, PropfindError> {
    if body.trim().is_empty() {
        return Ok(PropfindRequest::AllProp);
    }
    parse_propfind(body)
}

/// One resource for the `multistatus` response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropfindResource {
    /// Absolute href (URL-path), e.g. `/dav/photos/cat.jpg`.
    pub href: String,
    /// `true` when the resource is a directory / collection.
    pub is_collection: bool,
    /// Length in bytes. `None` for collections.
    pub content_length: Option<u64>,
    /// HTTP `Last-Modified` formatted string (RFC 7231 IMF-fixdate).
    /// `None` when the daemon did not surface one.
    pub last_modified: Option<String>,
    /// MIME type for files. `None` for collections (or unknowns).
    pub content_type: Option<String>,
}

/// One per-resource entry returned alongside the parsed request so
/// callers can build the multistatus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropfindResponseEntry {
    /// The resource being described.
    pub resource: PropfindResource,
    /// HTTP-style status line. Today every response is `200 OK`;
    /// 404s for missing nested props land in T1.6.c.
    pub status_line: &'static str,
}

impl PropfindResponseEntry {
    /// Convenience constructor for `200 OK` responses.
    #[must_use]
    pub fn ok(resource: PropfindResource) -> Self {
        Self {
            resource,
            status_line: "HTTP/1.1 200 OK",
        }
    }
}

/// Render the multistatus XML body for a list of resources. Output
/// is UTF-8 with the canonical `xmlns:D="DAV:"` prefix used by
/// every common WebDAV client.
#[must_use]
pub fn render_multistatus(entries: &[PropfindResponseEntry]) -> String {
    let mut out = String::with_capacity(256 + entries.len() * 256);
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str("<D:multistatus xmlns:D=\"DAV:\">\n");
    for entry in entries {
        let r = &entry.resource;
        out.push_str("  <D:response>\n");
        out.push_str("    <D:href>");
        push_xml_text(&mut out, &r.href);
        out.push_str("</D:href>\n");
        out.push_str("    <D:propstat>\n");
        out.push_str("      <D:prop>\n");
        if r.is_collection {
            out.push_str("        <D:resourcetype><D:collection/></D:resourcetype>\n");
        } else {
            out.push_str("        <D:resourcetype/>\n");
        }
        if let Some(len) = r.content_length {
            let _ = writeln!(
                out,
                "        <D:getcontentlength>{len}</D:getcontentlength>"
            );
        }
        if let Some(modified) = &r.last_modified {
            out.push_str("        <D:getlastmodified>");
            push_xml_text(&mut out, modified);
            out.push_str("</D:getlastmodified>\n");
        }
        if let Some(ctype) = &r.content_type {
            out.push_str("        <D:getcontenttype>");
            push_xml_text(&mut out, ctype);
            out.push_str("</D:getcontenttype>\n");
        }
        out.push_str("      </D:prop>\n");
        out.push_str("      <D:status>");
        out.push_str(entry.status_line);
        out.push_str("</D:status>\n");
        out.push_str("    </D:propstat>\n");
        out.push_str("  </D:response>\n");
    }
    out.push_str("</D:multistatus>\n");
    out
}

/// Append `text` to `out`, XML-escaping the five standard entities.
fn push_xml_text(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
}

/// Returns `true` if `body` contains an element whose local name is
/// `local` (case-sensitive). Walks the full string once; recognises
/// both `<D:local/>` self-closing and `<D:local>` open forms.
fn contains_local_name(body: &str, local: &str) -> bool {
    let needle_open = format!(":{local}");
    let needle_root = format!("<{local}");
    let needle_root_closed = format!("</{local}");
    body.contains(&needle_open) || body.contains(&needle_root) || body.contains(&needle_root_closed)
}

/// Find the first `<X:local>...</X:local>` element by local name and
/// return its inner contents. Self-closing forms return `None`.
fn extract_first_element_inner(body: &str, local: &str) -> Option<String> {
    // Find a `<...local` open tag. Must be followed by `>` (open
    // tag, possibly with an attribute), not `/>` (self-closing) or
    // a longer name (e.g. `propname` must not match `prop`).
    let mut search_start = 0;
    while search_start < body.len() {
        let idx = body[search_start..].find('<')?;
        let abs = search_start + idx;
        let rest = &body[abs..];
        // Skip element start tag past possible namespace prefix and
        // the local name.
        let after_lt = &rest[1..];
        let local_start = after_lt.find(':').map(|p| p + 1).unwrap_or(0);
        let after_prefix = &after_lt[local_start..];
        if let Some(after_name) = after_prefix.strip_prefix(local) {
            // Discriminate between `<prop>`, `<prop ...>`,
            // `<prop/>`, and `<propname>`. The next char must be
            // whitespace, `>`, or `/`.
            let next = after_name.chars().next().unwrap_or(' ');
            if next == '>' || next.is_whitespace() {
                // Open tag — find matching close.
                let inner_start = abs
                    + 1 // '<'
                    + local_start
                    + local.len();
                // Skip any attributes up to the `>`.
                let inner_start_idx = body[inner_start..].find('>')?;
                let inner_start_abs = inner_start + inner_start_idx + 1;
                // Find `</...local>`.
                let close_needle = format!(":{local}>");
                let close_idx_rel = body[inner_start_abs..]
                    .find(&close_needle)
                    .or_else(|| body[inner_start_abs..].find(&format!("</{local}>")))?;
                let close_idx_abs = inner_start_abs + close_idx_rel;
                // Walk back to the `</`.
                let lt_back = body[..close_idx_abs].rfind("</")?;
                return Some(body[inner_start_abs..lt_back].to_owned());
            }
        }
        search_start = abs + 1;
    }
    None
}

/// Extract every element local-name in `inner` (one occurrence per
/// distinct element). De-duplicates while preserving first-seen
/// order so a request like `<displayname/><displayname/>` yields
/// `["displayname"]` exactly once.
fn extract_local_names(inner: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut idx = 0;
    while idx < inner.len() {
        let Some(lt_rel) = inner[idx..].find('<') else {
            break;
        };
        let lt = idx + lt_rel;
        let after = &inner[lt + 1..];
        if after.starts_with('/') {
            // Closing tag — skip past `>`.
            idx = inner[lt..]
                .find('>')
                .map(|p| lt + p + 1)
                .unwrap_or(inner.len());
            continue;
        }
        // Strip namespace prefix.
        let local_start = after.find(':').map(|p| p + 1).unwrap_or(0);
        let after_prefix = &after[local_start..];
        // Read until whitespace, `>`, `/`, or end.
        let end = after_prefix
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(after_prefix.len());
        let name = &after_prefix[..end];
        if !name.is_empty() && !out.iter().any(|n| n == name) {
            out.push(name.to_owned());
        }
        idx = inner[lt..]
            .find('>')
            .map(|p| lt + p + 1)
            .unwrap_or(inner.len());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_allprop_recognised() {
        let body = r#"<?xml version="1.0"?>
<D:propfind xmlns:D="DAV:"><D:allprop/></D:propfind>"#;
        assert_eq!(parse_propfind(body).unwrap(), PropfindRequest::AllProp);
    }

    #[test]
    fn parse_propname_recognised() {
        let body = r#"<D:propfind xmlns:D="DAV:"><D:propname/></D:propfind>"#;
        assert_eq!(parse_propfind(body).unwrap(), PropfindRequest::PropName);
    }

    #[test]
    fn parse_named_props_lists_local_names() {
        let body = r#"<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:displayname/>
    <D:getcontentlength/>
    <D:getcontenttype/>
  </D:prop>
</D:propfind>"#;
        let req = parse_propfind(body).unwrap();
        match req {
            PropfindRequest::NamedProps { names } => {
                assert_eq!(
                    names,
                    vec!["displayname", "getcontentlength", "getcontenttype"]
                );
            }
            other => panic!("expected NamedProps, got {other:?}"),
        }
    }

    #[test]
    fn parse_empty_body_returns_err() {
        assert_eq!(parse_propfind("").unwrap_err(), PropfindError::Empty);
    }

    #[test]
    fn parse_or_allprop_treats_empty_as_allprop() {
        assert_eq!(
            parse_propfind_or_allprop("").unwrap(),
            PropfindRequest::AllProp
        );
    }

    #[test]
    fn parse_garbage_returns_malformed() {
        let body = "<not-a-propfind/>";
        assert!(matches!(
            parse_propfind(body).unwrap_err(),
            PropfindError::Malformed(_)
        ));
    }

    #[test]
    fn render_multistatus_collection() {
        let entry = PropfindResponseEntry::ok(PropfindResource {
            href: "/dav/photos".to_owned(),
            is_collection: true,
            content_length: None,
            last_modified: Some("Wed, 30 Apr 2026 12:00:00 GMT".to_owned()),
            content_type: None,
        });
        let body = render_multistatus(&[entry]);
        assert!(body.contains("<D:multistatus xmlns:D=\"DAV:\">"));
        assert!(body.contains("<D:href>/dav/photos</D:href>"));
        assert!(body.contains("<D:resourcetype><D:collection/></D:resourcetype>"));
        assert!(!body.contains("<D:getcontentlength>"));
        assert!(body.contains("<D:status>HTTP/1.1 200 OK</D:status>"));
    }

    #[test]
    fn render_multistatus_file_includes_size_and_type() {
        let entry = PropfindResponseEntry::ok(PropfindResource {
            href: "/dav/photos/cat.jpg".to_owned(),
            is_collection: false,
            content_length: Some(102_400),
            last_modified: None,
            content_type: Some("image/jpeg".to_owned()),
        });
        let body = render_multistatus(&[entry]);
        assert!(body.contains("<D:getcontentlength>102400</D:getcontentlength>"));
        assert!(body.contains("<D:getcontenttype>image/jpeg</D:getcontenttype>"));
        assert!(body.contains("<D:resourcetype/>"));
    }

    #[test]
    fn render_multistatus_escapes_xml_entities_in_href() {
        let entry = PropfindResponseEntry::ok(PropfindResource {
            href: "/dav/odd & odd <name>.txt".to_owned(),
            is_collection: false,
            content_length: Some(10),
            last_modified: None,
            content_type: None,
        });
        let body = render_multistatus(&[entry]);
        assert!(body.contains("/dav/odd &amp; odd &lt;name&gt;.txt"));
        assert!(!body.contains("/dav/odd & odd"));
    }

    #[test]
    fn parse_named_props_dedupes_repeats() {
        let body = r#"<D:propfind xmlns:D="DAV:">
  <D:prop><D:displayname/><D:displayname/></D:prop>
</D:propfind>"#;
        match parse_propfind(body).unwrap() {
            PropfindRequest::NamedProps { names } => {
                assert_eq!(names, vec!["displayname"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_does_not_confuse_propname_for_prop() {
        // `<propname/>` must not match a `<prop>...</prop>` extractor.
        let body = r#"<D:propfind xmlns:D="DAV:"><D:propname/></D:propfind>"#;
        assert_eq!(parse_propfind(body).unwrap(), PropfindRequest::PropName);
    }
}
