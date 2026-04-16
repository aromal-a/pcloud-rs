//! Progress indicator wrapper for long-running CLI subcommands.
//!
//! This module provides a zero-dependency progress reporter used by
//! long-running commands (`mount`, upload / download flows, `sync`).
//!
//! Behavior matrix:
//!
//! | Mode                       | Output                                         |
//! |----------------------------|------------------------------------------------|
//! | text + TTY (stderr)        | in-place spinner + current step label          |
//! | `--json` OR non-TTY stderr | one NDJSON line per event on stderr            |
//! | `--quiet`                  | suppressed entirely                            |
//!
//! Stream discipline:
//! - progress events go to **stderr** only,
//! - final command results keep going to **stdout**,
//! - so a caller piping stdout into `jq` is never polluted by progress
//!   chatter.
//!
//! Security:
//! - messages are caller-provided step labels only; never pass secrets,
//! - we do not buffer user data; we only flush short control strings.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io::{self, IsTerminal, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::globals::{GlobalFlags, OutputFormat};

/// Minimum delay between spinner frame redraws in text/TTY mode.
const SPINNER_TICK: Duration = Duration::from_millis(100);

/// Braille spinner frames. Chosen over the classic `|/-\` rotation because
/// it renders cleanly in modern terminals and survives CR-overwrites
/// without flicker.
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// How progress events should be rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressMode {
    /// Suppress all output (`--quiet`).
    Quiet,
    /// Emit NDJSON lines to stderr (`--json` OR non-TTY stderr).
    Ndjson,
    /// In-place spinner on a TTY.
    Spinner,
}

impl ProgressMode {
    /// Pick a mode from `GlobalFlags` and a TTY probe.
    ///
    /// `is_stderr_tty` is injected so tests can force either branch
    /// without relying on the real terminal state.
    #[must_use]
    pub fn resolve(flags: &GlobalFlags, is_stderr_tty: bool) -> Self {
        if flags.quiet {
            return Self::Quiet;
        }
        if flags.output == OutputFormat::Json || !is_stderr_tty {
            return Self::Ndjson;
        }
        Self::Spinner
    }

    /// Convenience: resolve from the real process state.
    #[must_use]
    pub fn from_env(flags: &GlobalFlags) -> Self {
        Self::resolve(flags, io::stderr().is_terminal())
    }
}

/// One progress event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressEvent<'a> {
    pub step: &'a str,
    pub message: &'a str,
    pub done: Option<u64>,
    pub total: Option<u64>,
}

/// Sink trait so tests can capture output without touching real stderr.
pub trait ProgressSink: Send {
    fn write_line(&mut self, line: &str) -> io::Result<()>;
    fn write_inline(&mut self, frame: &str) -> io::Result<()>;
    fn clear_inline(&mut self) -> io::Result<()>;
}

/// Default sink: writes to stderr.
pub struct StderrSink;

impl ProgressSink for StderrSink {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        let mut s = io::stderr().lock();
        s.write_all(line.as_bytes())?;
        s.write_all(b"\n")?;
        s.flush()
    }
    fn write_inline(&mut self, frame: &str) -> io::Result<()> {
        let mut s = io::stderr().lock();
        s.write_all(b"\r")?;
        s.write_all(frame.as_bytes())?;
        s.flush()
    }
    fn clear_inline(&mut self) -> io::Result<()> {
        let mut s = io::stderr().lock();
        // CR + spaces + CR to fully erase the spinner row.
        s.write_all(b"\r\x1b[2K")?;
        s.flush()
    }
}

/// In-memory sink for tests.
#[derive(Default)]
pub struct BufferSink {
    pub buf: Vec<u8>,
}

impl ProgressSink for BufferSink {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        self.buf.extend_from_slice(line.as_bytes());
        self.buf.push(b'\n');
        Ok(())
    }
    fn write_inline(&mut self, frame: &str) -> io::Result<()> {
        self.buf.push(b'\r');
        self.buf.extend_from_slice(frame.as_bytes());
        Ok(())
    }
    fn clear_inline(&mut self) -> io::Result<()> {
        self.buf.extend_from_slice(b"\r\x1b[2K");
        Ok(())
    }
}

/// Progress reporter wrapping a long-running operation.
///
/// Create once at the top of a command, call [`ProgressReporter::update`]
/// on each significant step, then [`ProgressReporter::finish`] when done.
pub struct ProgressReporter {
    mode: ProgressMode,
    sink: Mutex<Box<dyn ProgressSink>>,
    frame_idx: Mutex<usize>,
    last_tick: Mutex<Option<Instant>>,
    finished: Mutex<bool>,
}

impl ProgressReporter {
    /// Build a reporter with an explicit sink. Used by tests.
    #[must_use]
    pub fn with_sink(mode: ProgressMode, sink: Box<dyn ProgressSink>) -> Self {
        Self {
            mode,
            sink: Mutex::new(sink),
            frame_idx: Mutex::new(0),
            last_tick: Mutex::new(None),
            finished: Mutex::new(false),
        }
    }

    /// Build a reporter that writes to real stderr.
    #[must_use]
    pub fn new(mode: ProgressMode) -> Self {
        Self::with_sink(mode, Box::new(StderrSink))
    }

    /// Report a progress event. No-op in `Quiet`.
    ///
    /// In `Ndjson`, emits one line of `{"type":"progress", ...}` to the
    /// sink. In `Spinner`, rate-limits to `SPINNER_TICK` and redraws
    /// the current frame with the step label.
    pub fn update(&self, event: &ProgressEvent<'_>) {
        match self.mode {
            ProgressMode::Quiet => {}
            ProgressMode::Ndjson => {
                let line = encode_ndjson(event);
                // Best-effort: progress must never crash the command.
                if let Ok(mut sink) = self.sink.lock() {
                    let _ = sink.write_line(&line);
                }
            }
            ProgressMode::Spinner => {
                let now = Instant::now();
                let Ok(mut last) = self.last_tick.lock() else {
                    return;
                };
                if let Some(t) = *last
                    && now.duration_since(t) < SPINNER_TICK
                {
                    return;
                }
                *last = Some(now);
                let Ok(mut idx) = self.frame_idx.lock() else {
                    return;
                };
                let frame = SPINNER_FRAMES[*idx % SPINNER_FRAMES.len()];
                *idx = idx.wrapping_add(1);
                let label = format_spinner_label(frame, event);
                if let Ok(mut sink) = self.sink.lock() {
                    let _ = sink.write_inline(&label);
                }
            }
        }
    }

    /// Finish the progress stream. Clears the spinner line (TTY) or
    /// emits a terminal `{"type":"progress","done":...}` (NDJSON).
    pub fn finish(&self, final_message: &str) {
        if let Ok(mut done) = self.finished.lock() {
            if *done {
                return;
            }
            *done = true;
        }
        match self.mode {
            ProgressMode::Quiet => {}
            ProgressMode::Ndjson => {
                let ev = ProgressEvent {
                    step: "done",
                    message: final_message,
                    done: None,
                    total: None,
                };
                let line = encode_ndjson(&ev);
                if let Ok(mut sink) = self.sink.lock() {
                    let _ = sink.write_line(&line);
                }
            }
            ProgressMode::Spinner => {
                if let Ok(mut sink) = self.sink.lock() {
                    let _ = sink.clear_inline();
                }
            }
        }
    }

    #[must_use]
    pub fn mode(&self) -> ProgressMode {
        self.mode
    }
}

/// Render a `ProgressEvent` as one NDJSON line.
///
/// Hand-rolled to avoid serde overhead for a 4-field shape; the escape
/// routine handles the JSON control character set so step labels
/// containing quotes or backslashes are safe.
fn encode_ndjson(ev: &ProgressEvent<'_>) -> String {
    let mut s = String::with_capacity(96);
    s.push_str(r#"{"type":"progress","step":""#);
    json_escape_into(ev.step, &mut s);
    s.push_str(r#"","message":""#);
    json_escape_into(ev.message, &mut s);
    s.push('"');
    if let Some(d) = ev.done {
        s.push_str(r#","done":"#);
        s.push_str(&d.to_string());
    }
    if let Some(t) = ev.total {
        s.push_str(r#","total":"#);
        s.push_str(&t.to_string());
    }
    s.push('}');
    s
}

fn json_escape_into(input: &str, out: &mut String) {
    for c in input.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
}

fn format_spinner_label(frame: &str, ev: &ProgressEvent<'_>) -> String {
    match (ev.done, ev.total) {
        (Some(d), Some(t)) => format!("{frame} {} [{d}/{t}] {}", ev.step, ev.message),
        (Some(d), None) => format!("{frame} {} [{d}] {}", ev.step, ev.message),
        _ => format!("{frame} {} {}", ev.step, ev.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

    /// Sink that shares a Vec<u8> via Arc so tests can inspect it after
    /// the reporter has taken ownership.
    struct SharedSink {
        inner: Arc<StdMutex<Vec<u8>>>,
    }
    impl ProgressSink for SharedSink {
        fn write_line(&mut self, line: &str) -> io::Result<()> {
            let mut g = self.inner.lock().unwrap();
            g.extend_from_slice(line.as_bytes());
            g.push(b'\n');
            Ok(())
        }
        fn write_inline(&mut self, frame: &str) -> io::Result<()> {
            let mut g = self.inner.lock().unwrap();
            g.push(b'\r');
            g.extend_from_slice(frame.as_bytes());
            Ok(())
        }
        fn clear_inline(&mut self) -> io::Result<()> {
            let mut g = self.inner.lock().unwrap();
            g.extend_from_slice(b"\r\x1b[2K");
            Ok(())
        }
    }

    fn shared() -> (Arc<StdMutex<Vec<u8>>>, Box<dyn ProgressSink>) {
        let inner = Arc::new(StdMutex::new(Vec::new()));
        let sink = Box::new(SharedSink {
            inner: inner.clone(),
        });
        (inner, sink)
    }

    #[test]
    fn resolve_modes_from_flags() {
        let mut flags = GlobalFlags::default();
        assert_eq!(ProgressMode::resolve(&flags, true), ProgressMode::Spinner);
        assert_eq!(ProgressMode::resolve(&flags, false), ProgressMode::Ndjson);
        flags.output = OutputFormat::Json;
        assert_eq!(ProgressMode::resolve(&flags, true), ProgressMode::Ndjson);
        flags.quiet = true;
        assert_eq!(ProgressMode::resolve(&flags, true), ProgressMode::Quiet);
        assert_eq!(ProgressMode::resolve(&flags, false), ProgressMode::Quiet);
    }

    #[test]
    fn progress_json_emits_ndjson_to_stderr() {
        // Simulates `--json` mode (or non-TTY): reporter must emit one
        // NDJSON object per update, all on the stderr-equivalent sink,
        // never on stdout.
        let (captured, sink) = shared();
        let r = ProgressReporter::with_sink(ProgressMode::Ndjson, sink);
        r.update(&ProgressEvent {
            step: "upload",
            message: "chunk 1",
            done: Some(1),
            total: Some(4),
        });
        r.update(&ProgressEvent {
            step: "upload",
            message: "chunk \"2\"",
            done: Some(2),
            total: Some(4),
        });
        r.finish("uploaded");

        let out = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "got output: {out:?}");

        // Each line parses as JSON and carries the required shape.
        for line in &lines {
            assert!(line.starts_with('{') && line.ends_with('}'), "line={line}");
            assert!(line.contains(r#""type":"progress""#), "line={line}");
            assert!(line.contains(r#""step":"#), "line={line}");
            assert!(line.contains(r#""message":"#), "line={line}");
        }
        // Numeric fields unquoted.
        assert!(lines[0].contains(r#""done":1"#));
        assert!(lines[0].contains(r#""total":4"#));
        // Embedded quote in message is escaped, not passed raw.
        assert!(lines[1].contains(r#"chunk \"2\""#));
        // Final line is the "done" terminator.
        assert!(lines[2].contains(r#""step":"done""#));
    }

    #[test]
    fn progress_text_respects_quiet() {
        // `--quiet` must produce zero bytes regardless of mode/TTY.
        let (captured, sink) = shared();
        let r = ProgressReporter::with_sink(ProgressMode::Quiet, sink);
        for i in 0..5 {
            r.update(&ProgressEvent {
                step: "sync",
                message: "walking",
                done: Some(i),
                total: Some(5),
            });
        }
        r.finish("done");
        assert!(
            captured.lock().unwrap().is_empty(),
            "quiet mode must not write anything"
        );
    }

    #[test]
    fn spinner_mode_writes_inline_frames() {
        let (captured, sink) = shared();
        let r = ProgressReporter::with_sink(ProgressMode::Spinner, sink);
        // First update always fires (no prior tick).
        r.update(&ProgressEvent {
            step: "mount",
            message: "attaching",
            done: None,
            total: None,
        });
        r.finish("mounted");
        let out = captured.lock().unwrap().clone();
        assert!(out.starts_with(b"\r"), "spinner must use CR prefix");
        // finish() clears the line.
        assert!(
            out.windows(2).any(|w| w == b"\r\x1b"),
            "expected clear-line sequence, got {out:?}"
        );
    }

    #[test]
    fn ndjson_escapes_control_chars() {
        let ev = ProgressEvent {
            step: "s",
            message: "line1\nline2\tend",
            done: None,
            total: None,
        };
        let s = encode_ndjson(&ev);
        assert!(s.contains(r"\n"), "got {s}");
        assert!(s.contains(r"\t"), "got {s}");
        // No raw newline smuggled into NDJSON (would break one-event-per-line).
        assert!(!s.contains('\n'));
    }

    #[test]
    fn finish_is_idempotent() {
        let (captured, sink) = shared();
        let r = ProgressReporter::with_sink(ProgressMode::Ndjson, sink);
        r.finish("a");
        r.finish("b");
        let n = captured
            .lock()
            .unwrap()
            .iter()
            .filter(|b| **b == b'\n')
            .count();
        assert_eq!(n, 1, "finish must only emit once");
    }
}
