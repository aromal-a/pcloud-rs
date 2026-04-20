//! Shared mock primitives for backend integration testing.
//!
//! # Mutex poisoning policy
//!
//! SAFETY: All `.expect("… mutex poisoned")` call sites in this module
//! protect private mock-recorder fields that are only touched by the
//! test harness. A poison here means another test thread panicked
//! while holding the lock — which is exactly what we want to surface
//! deterministically to the test runner rather than swallow.
//!
//! This module provides three reusable recorders promoted from the
//! `pcloud-fs` mock-backend pattern (R18 wave-01 audit ask) so that each
//! of the ten `pcloud-backends` subsystems can expose a `mock` submodule
//! with identical, deterministic fixtures:
//!
//! * [`MockAudit`] records every audit event surfaced by a backend.
//! * [`MockStore`] records every durable store write a backend would
//!   have performed.
//! * [`MockProto`] records every outbound protocol call a backend would
//!   have emitted on the wire.
//!
//! These recorders are intentionally stringly-typed: every backend in
//! the crate has a distinct audit/store/proto vocabulary, so the
//! shared fixture records `(category, name, payload)` triples rather
//! than one specific typed shape. Higher-level per-backend `mock`
//! submodules build thin wrappers on top.
//!
//! # Properties
//!
//! * `Send + Sync` — internal state lives behind a [`Mutex`].
//! * Cheap to construct — [`Default`] is the canonical constructor.
//! * Deterministic — no thread sleeps, no clocks, no I/O. Events are
//!   appended in call order and read back verbatim.
//!
//! **PLATFORM:** all
//! **GATING:** none (portable).

use std::sync::Mutex;

/// A single recorded event on one of the mock recorders.
///
/// `category` is the recorder-defined bucket (e.g. the audit event
/// name, store table name, or protocol command). `name` is the
/// operation or field within the category, and `payload` is an
/// optional free-form debug representation callers may use to
/// assert on argument shape without pulling in backend-internal
/// types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockEvent {
    /// Recorder-defined bucket (audit event kind, store table, proto command).
    pub category: String,
    /// Operation or field within the category.
    pub name: String,
    /// Optional debug representation of the operation payload.
    pub payload: Option<String>,
}

impl MockEvent {
    /// Build a new event with no payload.
    pub fn bare(category: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            category: category.into(),
            name: name.into(),
            payload: None,
        }
    }

    /// Build a new event with a debug payload.
    pub fn with_payload(
        category: impl Into<String>,
        name: impl Into<String>,
        payload: impl Into<String>,
    ) -> Self {
        Self {
            category: category.into(),
            name: name.into(),
            payload: Some(payload.into()),
        }
    }
}

/// Append-only in-memory recorder shared by the three mock surfaces.
#[derive(Debug, Default)]
struct Recorder {
    events: Mutex<Vec<MockEvent>>,
}

impl Recorder {
    fn push(&self, event: MockEvent) {
        // `expect` here is acceptable: a poisoned mutex means another
        // test thread panicked while holding the lock, and surfacing
        // that panic is the correct behaviour for a deterministic
        // test helper.
        self.events
            .lock()
            .expect("mock recorder mutex poisoned")
            .push(event);
    }

    fn snapshot(&self) -> Vec<MockEvent> {
        self.events
            .lock()
            .expect("mock recorder mutex poisoned")
            .clone()
    }

    fn len(&self) -> usize {
        self.events
            .lock()
            .expect("mock recorder mutex poisoned")
            .len()
    }

    fn clear(&self) {
        self.events
            .lock()
            .expect("mock recorder mutex poisoned")
            .clear();
    }
}

/// Mock audit sink that records every audit event a backend emits.
///
/// Backends typically emit audit events for user-visible side effects
/// (auth success, sync-root add/remove, upload save, etc.). This
/// recorder stores each event verbatim in call order so tests can
/// assert `records()` against an expected sequence.
#[derive(Debug, Default)]
pub struct MockAudit {
    inner: Recorder,
}

impl MockAudit {
    /// Construct an empty audit recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an audit event with optional payload.
    pub fn record(&self, event: MockEvent) {
        self.inner.push(event);
    }

    /// Convenience: record `(kind, detail)` with no payload.
    pub fn emit(&self, kind: impl Into<String>, detail: impl Into<String>) {
        self.inner.push(MockEvent::bare(kind, detail));
    }

    /// Snapshot of all recorded events in call order.
    pub fn records(&self) -> Vec<MockEvent> {
        self.inner.snapshot()
    }

    /// Number of recorded events so far.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether no events have been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.inner.len() == 0
    }

    /// Reset the recorder to its initial empty state.
    pub fn reset(&self) {
        self.inner.clear();
    }
}

/// Mock persistence sink that records every DB write a backend issues.
///
/// The recorder is stringly-typed by design: backends write to heterogeneous
/// store tables (`sync_roots`, `backup_devices`, `auth_vault`, ...) and the
/// shared fixture captures `(table, operation, payload)` without having to
/// reflect each backend's concrete row type.
#[derive(Debug, Default)]
pub struct MockStore {
    inner: Recorder,
}

impl MockStore {
    /// Construct an empty store recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a raw event.
    pub fn record(&self, event: MockEvent) {
        self.inner.push(event);
    }

    /// Convenience: record a write against `table` as `operation`
    /// (`insert`, `update`, `delete`, ...) with a debug payload.
    pub fn write(
        &self,
        table: impl Into<String>,
        operation: impl Into<String>,
        payload: impl Into<String>,
    ) {
        self.inner
            .push(MockEvent::with_payload(table, operation, payload));
    }

    /// Snapshot of all recorded events in call order.
    pub fn records(&self) -> Vec<MockEvent> {
        self.inner.snapshot()
    }

    /// Number of recorded events so far.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether no events have been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.inner.len() == 0
    }

    /// Reset the recorder to its initial empty state.
    pub fn reset(&self) {
        self.inner.clear();
    }
}

/// Mock protocol transport that records every outbound API call.
///
/// Backends issue protocol calls via their typed API clients
/// (`auth_api::login`, `folder_api::listfolder`, ...). This recorder
/// stores the command name and a debug representation of the encoded
/// arguments, without actually dispatching to a live or canned
/// transport.
///
/// Tests can additionally seed canned responses via [`MockProto::seed`]
/// so a representative operation can round-trip through a backend
/// helper that expects a protocol-shaped reply.
#[derive(Debug, Default)]
pub struct MockProto {
    inner: Recorder,
    canned: Mutex<Vec<(String, String)>>,
}

impl MockProto {
    /// Construct an empty protocol recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an outbound call with optional serialised arguments.
    pub fn record(&self, event: MockEvent) {
        self.inner.push(event);
    }

    /// Convenience: record `command` with a debug payload describing
    /// the encoded arguments.
    pub fn call(&self, command: impl Into<String>, args: impl Into<String>) {
        self.inner
            .push(MockEvent::with_payload("proto", command, args));
    }

    /// Seed a canned `(command, response)` pair consumed in order by
    /// [`MockProto::take_canned`]. Tests that do not need responses
    /// may skip this entirely.
    pub fn seed(&self, command: impl Into<String>, response: impl Into<String>) {
        self.canned
            .lock()
            .expect("mock proto canned mutex poisoned")
            .push((command.into(), response.into()));
    }

    /// Pop the next seeded `(command, response)` pair, if any.
    pub fn take_canned(&self) -> Option<(String, String)> {
        let mut guard = self
            .canned
            .lock()
            .expect("mock proto canned mutex poisoned");
        if guard.is_empty() {
            None
        } else {
            Some(guard.remove(0))
        }
    }

    /// Snapshot of all recorded calls in call order.
    pub fn records(&self) -> Vec<MockEvent> {
        self.inner.snapshot()
    }

    /// Number of recorded calls so far.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether no calls have been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.inner.len() == 0
    }

    /// Reset the recorder to its initial empty state.
    pub fn reset(&self) {
        self.inner.clear();
        self.canned
            .lock()
            .expect("mock proto canned mutex poisoned")
            .clear();
    }
}

/// Convenience bundle holding one of each shared recorder. Backends'
/// per-module `mock` submodules typically wrap this struct so tests
/// only have to thread a single fixture handle.
#[derive(Debug, Default)]
pub struct MockFixture {
    /// Audit event recorder.
    pub audit: MockAudit,
    /// Store write recorder.
    pub store: MockStore,
    /// Protocol call recorder.
    pub proto: MockProto,
}

impl MockFixture {
    /// Construct a fixture with three empty recorders.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset all three recorders back to empty.
    pub fn reset(&self) {
        self.audit.reset();
        self.store.reset();
        self.proto.reset();
    }
}

// Compile-time assertions: the shared recorders must be `Send + Sync`
// so backend fixtures can be shared across integration-test threads.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<MockAudit>();
    assert_send_sync::<MockStore>();
    assert_send_sync::<MockProto>();
    assert_send_sync::<MockFixture>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_records_in_order() {
        let audit = MockAudit::new();
        audit.emit("auth", "login_ok");
        audit.emit("auth", "logout");
        let records = audit.records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].category, "auth");
        assert_eq!(records[0].name, "login_ok");
        assert_eq!(records[1].name, "logout");
    }

    #[test]
    fn store_records_writes_with_payload() {
        let store = MockStore::new();
        store.write("sync_roots", "insert", "folder_id=1");
        let records = store.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].category, "sync_roots");
        assert_eq!(records[0].name, "insert");
        assert_eq!(records[0].payload.as_deref(), Some("folder_id=1"));
    }

    #[test]
    fn proto_records_calls_and_canned_responses() {
        let proto = MockProto::new();
        proto.call("userinfo", "token=redacted");
        proto.seed("userinfo", "{result:0}");
        assert_eq!(proto.len(), 1);
        let (cmd, resp) = proto.take_canned().expect("seeded response");
        assert_eq!(cmd, "userinfo");
        assert_eq!(resp, "{result:0}");
        assert!(proto.take_canned().is_none());
    }

    #[test]
    fn fixture_reset_clears_all_recorders() {
        let fx = MockFixture::new();
        fx.audit.emit("x", "y");
        fx.store.write("t", "op", "p");
        fx.proto.call("cmd", "args");
        fx.reset();
        assert!(fx.audit.is_empty());
        assert!(fx.store.is_empty());
        assert!(fx.proto.is_empty());
    }
}
