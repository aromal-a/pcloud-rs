//! Inline HTML rendering for the MVP web UI.
//!
//! No templating engine is used yet; we build strings directly and
//! HTML-escape any field that originates from the daemon. When the
//! real Leptos SSR app lands this module is expected to be removed
//! or replaced wholesale.

// **PLATFORM:** all
// **GATING:** none (portable).

use crate::routes::StatusSummary;

/// Escape a string for safe interpolation into HTML text/attribute
/// content. Minimal allow-list for the five XML entities is sufficient
/// for the fields we currently render.
pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Render the plain-HTML status page.
pub(crate) fn render_index(status: &StatusSummary) -> String {
    let state_label = if status.online { "Online" } else { "Offline" };
    let state_class = if status.online { "ok" } else { "err" };
    let roots = status
        .sync_root_count
        .map(|n| n.to_string())
        .unwrap_or_else(|| "unknown".into());
    let mount = status.mount_state.as_deref().unwrap_or("unknown");
    let message = escape(&status.message);
    let raw = status
        .raw
        .as_deref()
        .map(escape)
        .unwrap_or_else(|| "(no response)".to_string());

    format!(
        "<!doctype html>\n\
<html lang=\"en\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<title>pcloud-rs status</title>\n\
<style>\n\
  body {{ font-family: system-ui, sans-serif; max-width: 720px; margin: 2em auto; padding: 0 1em; }}\n\
  h1 {{ font-size: 1.4em; }}\n\
  .status {{ font-weight: bold; }}\n\
  .ok {{ color: #0a7d1f; }}\n\
  .err {{ color: #a01010; }}\n\
  dl {{ display: grid; grid-template-columns: 10em 1fr; gap: 0.3em 1em; }}\n\
  dt {{ font-weight: 600; }}\n\
  pre {{ background: #f4f4f4; padding: 0.6em; overflow-x: auto; }}\n\
  footer {{ margin-top: 2em; color: #666; font-size: 0.85em; }}\n\
</style>\n\
</head>\n\
<body>\n\
<h1>pcloud-rs daemon</h1>\n\
<p class=\"status {class}\">Status: <span>{label}</span></p>\n\
<dl>\n\
  <dt>Message</dt><dd>{msg}</dd>\n\
  <dt>Sync roots</dt><dd>{roots}</dd>\n\
  <dt>Mount state</dt><dd>{mount}</dd>\n\
</dl>\n\
<h2>Raw IPC message</h2>\n\
<pre>{raw}</pre>\n\
<footer>pcloud-web MVP (P4.5 scaffold). Localhost-only. See README.</footer>\n\
</body>\n\
</html>\n",
        class = state_class,
        label = state_label,
        msg = message,
        roots = escape(&roots),
        mount = escape(mount),
        raw = raw,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_handles_entities() {
        assert_eq!(escape("<a&b>"), "&lt;a&amp;b&gt;");
        assert_eq!(escape("\"'"), "&quot;&#39;");
    }

    #[test]
    fn render_offline_page_contains_expected_markers() {
        let s = StatusSummary {
            online: false,
            message: "daemon offline".into(),
            sync_root_count: None,
            mount_state: None,
            raw: None,
        };
        let html = render_index(&s);
        assert!(html.contains("Status:"));
        assert!(html.contains("Offline"));
        assert!(html.contains("daemon offline"));
    }

    #[test]
    fn render_online_page_shows_counts() {
        let s = StatusSummary {
            online: true,
            message: "Online".into(),
            sync_root_count: Some(3),
            mount_state: Some("unmounted".into()),
            raw: Some("{\"sync_root_count\":3}".into()),
        };
        let html = render_index(&s);
        assert!(html.contains("Online"));
        assert!(html.contains("<dd>3</dd>"));
        assert!(html.contains("unmounted"));
    }
}
