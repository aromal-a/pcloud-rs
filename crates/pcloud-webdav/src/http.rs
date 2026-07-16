//! T1.6.b.1 — minimal HTTP/1.1 request/response codec for the WebDAV gateway.
//!
//! # Why hand-rolled
//!
//! The same rationale as `propfind.rs` applies: pulling `httparse`, `http`,
//! and a server crate would dominate the gateway's dependency tree for a
//! protocol slice used only to transport our own DAV verbs.
//!
//! What we actually need is:
//!
//! - parse an HTTP/1.1 request line + headers + body of bounded
//!   size,
//! - emit an HTTP/1.1 response status line + headers + body,
//! - reject malformed input long before it reaches the IPC layer.
//!
//! Roughly 200 LOC of careful `&[u8]` walking does that. The parser
//! deliberately accepts only what RFC 7230 calls a `recipient`
//! MUST accept and rejects everything else, so a misbehaving client
//! cannot fingerprint quirks here.
//!
//! # Wire shape
//!
//! ```ignore
//! use pcloud_webdav::http::{parse_request, HttpResponse};
//!
//! let raw = b"GET /dav/photos HTTP/1.1\r\nHost: localhost\r\n\r\n";
//! let req = parse_request(raw).unwrap();
//! assert_eq!(req.method, "GET");
//! assert_eq!(req.path, "/dav/photos");
//!
//! let resp = HttpResponse::ok_text("hello");
//! let bytes = resp.serialize();
//! assert!(bytes.starts_with(b"HTTP/1.1 200 OK\r\n"));
//! ```

// **PLATFORM:** all
// **GATING:** none (portable; pure stdlib).

use std::fmt::Write as _;

/// Maximum number of headers accepted on a request. Bounded so a
/// malicious client cannot grow the parser's allocation. RFC 7230
/// makes no formal cap; 64 is comfortably above what real clients
/// (browsers, `cadaver`, `curl`) ever send.
pub const MAX_REQUEST_HEADERS: usize = 64;

/// Maximum length (bytes) of the request line plus headers. Bodies
/// are not counted here — they are bounded by the server's
/// `max_put_body_bytes`. RFC 7230 §3.1.1 suggests 8 KiB minimum;
/// we go to 16 KiB to accommodate the long `Destination` and
/// `If` headers some WebDAV clients send.
pub const MAX_HEADER_BYTES: usize = 16 * 1024;

/// Parsed HTTP/1.1 request envelope.
///
/// `headers` retains insertion order so case-insensitive lookups
/// can find duplicates if any (the WebDAV spec sometimes asks for
/// repeated headers like `If`). Header names are lower-cased on
/// parse so callers can match without re-folding case at every
/// site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// HTTP method (`GET`, `PROPFIND`, `PUT`, `DELETE`, `OPTIONS`,
    /// `MKCOL`, …). Stored verbatim — handlers compare against
    /// upper-case literals.
    pub method: String,
    /// Request-target. Always the absolute-path form for the
    /// WebDAV gateway (origin-form per RFC 7230 §5.3.1) — the
    /// parser rejects authority-form / asterisk-form URIs.
    pub path: String,
    /// `(name, value)` pairs in arrival order. Names are
    /// lowercase ASCII (`content-length`, `depth`, …).
    pub headers: Vec<(String, String)>,
    /// Request body bytes (length matches `Content-Length`). Empty
    /// when the request had no body.
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// Find the first header value matching `name` (case-insensitive).
    /// Returns `None` if absent. The lower-cased form is what
    /// `parse_request` stored, so this is a fast linear scan.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        let needle = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k == &needle)
            .map(|(_, v)| v.as_str())
    }
}

/// Errors raised by [`parse_request`].
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum HttpParseError {
    /// Request line / headers exceeded [`MAX_HEADER_BYTES`].
    #[error("HTTP header section larger than {limit} bytes")]
    HeadersTooLarge {
        /// The cap that was hit.
        limit: usize,
    },
    /// Header count exceeded [`MAX_REQUEST_HEADERS`].
    #[error("too many HTTP headers (cap {limit})")]
    TooManyHeaders {
        /// The cap that was hit.
        limit: usize,
    },
    /// Request line did not match `METHOD SP path SP HTTP/1.1`.
    #[error("malformed HTTP request line")]
    BadRequestLine,
    /// `Content-Length` header was malformed.
    #[error("invalid Content-Length header")]
    BadContentLength,
    /// Body was shorter than the declared `Content-Length` (the
    /// caller passed an incomplete buffer).
    #[error("body shorter than Content-Length")]
    ShortBody,
    /// A header line was missing the `:` separator.
    #[error("malformed HTTP header line")]
    BadHeaderLine,
    /// HTTP version was not `HTTP/1.1`. We deliberately do not
    /// support 1.0 (no chunked TE there) or 2/3 (binary framing).
    #[error("unsupported HTTP version (only HTTP/1.1 accepted)")]
    UnsupportedVersion,
    /// Request-target was not in absolute-path origin form.
    #[error("request target must be an absolute path")]
    BadTarget,
}

/// Parse a complete HTTP/1.1 request from `raw`. The caller is
/// responsible for buffering bytes until both the header section
/// (terminator `\r\n\r\n`) **and** any declared body are present.
///
/// # Errors
///
/// Every variant of [`HttpParseError`] is recoverable at the
/// transport layer (close the connection, return 400). The parser
/// is total over `&[u8]` — no panics on any input.
pub fn parse_request(raw: &[u8]) -> Result<HttpRequest, HttpParseError> {
    // Locate the header / body boundary.
    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or(HttpParseError::BadRequestLine)?;
    if header_end > MAX_HEADER_BYTES {
        return Err(HttpParseError::HeadersTooLarge {
            limit: MAX_HEADER_BYTES,
        });
    }
    let header_section = &raw[..header_end];
    let body_start = header_end + 4;

    // Split request line from header lines.
    let line_end = header_section
        .windows(2)
        .position(|w| w == b"\r\n")
        .unwrap_or(header_section.len());
    let request_line = std::str::from_utf8(&header_section[..line_end])
        .map_err(|_| HttpParseError::BadRequestLine)?;
    let mut parts = request_line.splitn(3, ' ');
    let method = parts.next().ok_or(HttpParseError::BadRequestLine)?;
    let target = parts.next().ok_or(HttpParseError::BadRequestLine)?;
    let version = parts.next().ok_or(HttpParseError::BadRequestLine)?;
    if !method.chars().all(|c| c.is_ascii_uppercase() || c == '_') || method.is_empty() {
        return Err(HttpParseError::BadRequestLine);
    }
    if version != "HTTP/1.1" {
        return Err(HttpParseError::UnsupportedVersion);
    }
    if !target.starts_with('/') {
        return Err(HttpParseError::BadTarget);
    }

    // Parse headers.
    let mut headers: Vec<(String, String)> = Vec::new();
    let after_request_line = if line_end + 2 < header_section.len() {
        &header_section[line_end + 2..]
    } else {
        &header_section[header_section.len()..]
    };
    for line in split_crlf(after_request_line) {
        if line.is_empty() {
            continue;
        }
        if headers.len() >= MAX_REQUEST_HEADERS {
            return Err(HttpParseError::TooManyHeaders {
                limit: MAX_REQUEST_HEADERS,
            });
        }
        let line_str = std::str::from_utf8(line).map_err(|_| HttpParseError::BadHeaderLine)?;
        let colon = line_str.find(':').ok_or(HttpParseError::BadHeaderLine)?;
        let (name, value) = line_str.split_at(colon);
        if name.is_empty() {
            return Err(HttpParseError::BadHeaderLine);
        }
        let value = value[1..].trim();
        headers.push((name.to_ascii_lowercase(), value.to_owned()));
    }

    // Body length.
    let content_length = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .map(|(_, v)| {
            v.parse::<usize>()
                .map_err(|_| HttpParseError::BadContentLength)
        })
        .transpose()?
        .unwrap_or(0);
    if raw.len() < body_start + content_length {
        return Err(HttpParseError::ShortBody);
    }
    let body = raw[body_start..body_start + content_length].to_vec();

    Ok(HttpRequest {
        method: method.to_owned(),
        path: target.to_owned(),
        headers,
        body,
    })
}

/// Split `bytes` on each `\r\n`, yielding empty slices for blank
/// lines so the caller can skip them.
fn split_crlf(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut idx = 0;
    std::iter::from_fn(move || {
        if idx > bytes.len() {
            return None;
        }
        let rest = &bytes[idx..];
        if let Some(pos) = rest.windows(2).position(|w| w == b"\r\n") {
            let chunk = &rest[..pos];
            idx += pos + 2;
            Some(chunk)
        } else if rest.is_empty() {
            idx = bytes.len() + 1;
            None
        } else {
            idx = bytes.len() + 1;
            Some(rest)
        }
    })
}

/// HTTP/1.1 response envelope.
///
/// Construct via [`HttpResponse::ok_text`] /
/// [`HttpResponse::ok_xml_multistatus`] / [`HttpResponse::status`] /
/// [`HttpResponse::with_header`].
/// Serialize via [`HttpResponse::serialize`] (returns owned bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// Status code (e.g. `200`, `207`, `404`).
    pub status: u16,
    /// Reason phrase (e.g. `"OK"`, `"Multi-Status"`).
    pub reason: &'static str,
    /// `(name, value)` pairs. Order is preserved on the wire.
    pub headers: Vec<(String, String)>,
    /// Body bytes. Length is auto-emitted as `Content-Length`.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// Build a response from a status code + reason phrase.
    #[must_use]
    pub fn status(code: u16, reason: &'static str) -> Self {
        Self {
            status: code,
            reason,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// `200 OK` with `text/plain` body.
    #[must_use]
    pub fn ok_text(body: impl Into<String>) -> Self {
        Self::status(200, "OK")
            .with_header("Content-Type", "text/plain; charset=utf-8")
            .with_body(body.into().into_bytes())
    }

    /// `207 Multi-Status` with `application/xml` body — the
    /// canonical PROPFIND reply.
    #[must_use]
    pub fn ok_xml_multistatus(body: impl Into<String>) -> Self {
        Self::status(207, "Multi-Status")
            .with_header("Content-Type", "application/xml; charset=utf-8")
            .with_body(body.into().into_bytes())
    }

    /// Append a header. Existing headers with the same name are
    /// **not** removed — WebDAV occasionally requires repeated
    /// headers (e.g. multiple `WWW-Authenticate` challenges).
    #[must_use]
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Replace the body with `bytes`.
    #[must_use]
    pub fn with_body(mut self, bytes: Vec<u8>) -> Self {
        self.body = bytes;
        self
    }

    /// Encode the response as bytes ready to write to the socket.
    /// Always emits a `Content-Length` header derived from `body`,
    /// overriding any caller-supplied value to keep framing
    /// authoritative.
    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let mut head = String::with_capacity(64 + self.headers.len() * 32);
        let _ = write!(head, "HTTP/1.1 {} {}\r\n", self.status, self.reason);
        for (name, value) in &self.headers {
            if name.eq_ignore_ascii_case("content-length") {
                continue;
            }
            let _ = write!(head, "{name}: {value}\r\n");
        }
        let _ = write!(head, "Content-Length: {}\r\n", self.body.len());
        head.push_str("\r\n");
        let mut out = head.into_bytes();
        out.extend_from_slice(&self.body);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_get() {
        let raw = b"GET /dav HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let req = parse_request(raw).expect("parses");
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/dav");
        assert_eq!(req.header("Host"), Some("localhost"));
        assert!(req.body.is_empty());
    }

    #[test]
    fn parse_with_body() {
        let raw = b"PUT /dav/cat.jpg HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello";
        let req = parse_request(raw).expect("parses");
        assert_eq!(req.method, "PUT");
        assert_eq!(req.body, b"hello");
    }

    #[test]
    fn parse_propfind_with_xml_body() {
        let body = "<D:propfind xmlns:D=\"DAV:\"><D:allprop/></D:propfind>";
        let mut raw = format!(
            "PROPFIND /dav HTTP/1.1\r\nDepth: 1\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        raw.extend_from_slice(body.as_bytes());
        let req = parse_request(&raw).expect("parses");
        assert_eq!(req.method, "PROPFIND");
        assert_eq!(req.header("depth"), Some("1"));
        assert_eq!(req.body.as_slice(), body.as_bytes());
    }

    #[test]
    fn parse_lowercases_header_names() {
        let raw = b"GET / HTTP/1.1\r\nHost: localhost\r\nDepth: 0\r\n\r\n";
        let req = parse_request(raw).unwrap();
        // Internal storage is lowercase; header() does case-insensitive lookup.
        assert!(
            req.headers
                .iter()
                .all(|(k, _)| k == &k.to_ascii_lowercase())
        );
        assert_eq!(req.header("DEPTH"), Some("0"));
    }

    #[test]
    fn parse_rejects_http_1_0() {
        let raw = b"GET / HTTP/1.0\r\n\r\n";
        assert_eq!(
            parse_request(raw).unwrap_err(),
            HttpParseError::UnsupportedVersion
        );
    }

    #[test]
    fn parse_rejects_authority_form_target() {
        // `OPTIONS *` is sometimes accepted by HTTP/1.1 servers but
        // is not what the WebDAV gateway needs — reject so paths
        // are always `/...`.
        let raw = b"OPTIONS * HTTP/1.1\r\n\r\n";
        assert_eq!(parse_request(raw).unwrap_err(), HttpParseError::BadTarget);
    }

    #[test]
    fn parse_rejects_bad_content_length() {
        let raw = b"PUT / HTTP/1.1\r\nContent-Length: not-a-number\r\n\r\n";
        assert_eq!(
            parse_request(raw).unwrap_err(),
            HttpParseError::BadContentLength
        );
    }

    #[test]
    fn parse_rejects_short_body() {
        let raw = b"PUT / HTTP/1.1\r\nContent-Length: 100\r\n\r\nshort";
        assert_eq!(parse_request(raw).unwrap_err(), HttpParseError::ShortBody);
    }

    #[test]
    fn parse_rejects_bad_request_line() {
        let raw = b"NOTHTTP\r\n\r\n";
        assert!(matches!(
            parse_request(raw).unwrap_err(),
            HttpParseError::BadRequestLine
        ));
    }

    #[test]
    fn parse_rejects_bad_header_line() {
        let raw = b"GET / HTTP/1.1\r\nNoColonHere\r\n\r\n";
        assert_eq!(
            parse_request(raw).unwrap_err(),
            HttpParseError::BadHeaderLine
        );
    }

    #[test]
    fn parse_rejects_huge_header_section() {
        let mut raw = b"GET / HTTP/1.1\r\n".to_vec();
        for i in 0..(MAX_REQUEST_HEADERS + 5) {
            // Pad each header to ensure we breach the byte cap too.
            let h = format!("X-Pad-{i}: {}\r\n", "A".repeat(512));
            raw.extend_from_slice(h.as_bytes());
        }
        raw.extend_from_slice(b"\r\n");
        assert!(matches!(
            parse_request(&raw).unwrap_err(),
            HttpParseError::HeadersTooLarge { .. } | HttpParseError::TooManyHeaders { .. }
        ));
    }

    #[test]
    fn serialize_includes_content_length_and_body() {
        let resp = HttpResponse::ok_text("hello world");
        let bytes = resp.serialize();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Content-Type: text/plain; charset=utf-8\r\n"));
        assert!(text.contains("Content-Length: 11\r\n"));
        assert!(text.ends_with("\r\n\r\nhello world"));
    }

    #[test]
    fn serialize_strips_caller_supplied_content_length() {
        // Defensively remove a stray `Content-Length` header so
        // body-derived length stays authoritative even if a future
        // helper accidentally adds one.
        let resp = HttpResponse::status(200, "OK")
            .with_header("Content-Length", "9999")
            .with_body(b"hi".to_vec());
        let bytes = resp.serialize();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(!text.contains("Content-Length: 9999"));
        assert!(text.contains("Content-Length: 2\r\n"));
    }

    #[test]
    fn serialize_multistatus() {
        let resp = HttpResponse::ok_xml_multistatus("<D:multistatus xmlns:D=\"DAV:\"/>");
        let bytes = resp.serialize();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 207 Multi-Status\r\n"));
        assert!(text.contains("Content-Type: application/xml; charset=utf-8\r\n"));
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let req = HttpRequest {
            method: "GET".into(),
            path: "/".into(),
            headers: vec![("authorization".into(), "Bearer x".into())],
            body: Vec::new(),
        };
        assert_eq!(req.header("Authorization"), Some("Bearer x"));
        assert_eq!(req.header("AUTHORIZATION"), Some("Bearer x"));
        assert_eq!(req.header("missing"), None);
    }
}
