// **PLATFORM:** all
// **GATING:** none (portable).

use rusqlite::Connection;

/// Wraps a closure in a `BEGIN IMMEDIATE` / `COMMIT` pair with automatic `ROLLBACK` on error.
///
/// Every multi-statement mutation in the store goes through this helper so that repository
/// writes, audit hash-chain appends, and schema touch-ups remain atomic.
///
/// # RAII vs. scoped discipline
///
/// This type is intentionally a zero-sized marker, not a drop-guard. The
/// closure-based shape in [`TransactionBoundary::immediate`] gives us two
/// properties a classical RAII guard cannot:
///
/// 1. The commit/rollback decision is tied to a `Result`, not to the
///    presence or absence of a panic. A silent early-return carrying
///    `Err(_)` still triggers `ROLLBACK` — an RAII guard that committed
///    on drop would leak the partial write.
/// 2. No `Drop` impl means no risk of double-panic or of a rollback
///    failure being swallowed by unwinding; if rollback itself fails we
///    still surface the original error to the caller.
///
/// Transactions here are therefore *scoped* rather than *RAII-managed*:
/// the scope is the closure body, and the boundary type documents the
/// discipline the rest of the crate is obliged to follow. Every caller
/// that mutates more than one repository in a single logical operation
/// MUST go through this helper; see [`crate::persist_profile`] for the
/// canonical example.
///
/// # Concurrency
///
/// `BEGIN IMMEDIATE` takes a reserved lock eagerly. Under SQLite's WAL
/// journal this still allows concurrent readers, but a second writer
/// attempting to begin another immediate transaction fails fast with
/// `SQLITE_BUSY` instead of deadlocking at commit time. The store keeps
/// a single long-lived `Connection` behind a `Mutex` (see
/// [`crate::StoreHandle`]) so in practice writer contention is resolved
/// at the Rust mutex layer and `SQLITE_BUSY` is only surfaced by the
/// short-lived-connection facade.
#[derive(Debug, Default)]
pub struct TransactionBoundary;

impl TransactionBoundary {
    /// Returns the stable diagnostic name of this boundary, used in logs and audit records.
    #[must_use]
    pub fn name(&self) -> &'static str {
        "transaction-boundary"
    }

    /// Run `work` inside a `BEGIN IMMEDIATE TRANSACTION`.
    ///
    /// Commits on `Ok(_)`, rolls back on `Err(_)`, and propagates the
    /// original error after a best-effort rollback. A rollback failure is
    /// deliberately discarded (`let _ = …`) so the caller always sees the
    /// root cause rather than a confusing "could not rollback" secondary
    /// error hiding the interesting one.
    ///
    /// `BEGIN IMMEDIATE` acquires a reserved lock eagerly so that concurrent
    /// writers fail fast instead of racing to the commit point.
    ///
    /// # Panics
    ///
    /// If `work` panics, the transaction is not rolled back by this
    /// method — the panic unwinds through the method and SQLite's own
    /// open-transaction cleanup runs when the connection is dropped. In
    /// a long-lived [`crate::StoreHandle`], the connection is not dropped
    /// on panic; the caller must treat a panicking transaction as a
    /// serious fault and either reconnect or shut down the daemon. No
    /// silent state is leaked because the original `BEGIN IMMEDIATE`
    /// took a reserved lock that SQLite releases when the in-flight
    /// transaction is implicitly rolled back on next interaction.
    pub fn immediate<F, T>(&self, conn: &Connection, work: F) -> Result<T, rusqlite::Error>
    where
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error>,
    {
        conn.execute_batch("BEGIN IMMEDIATE TRANSACTION")?;
        match work(conn) {
            Ok(result) => {
                conn.execute_batch("COMMIT")?;
                Ok(result)
            }
            Err(err) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(err)
            }
        }
    }
}
