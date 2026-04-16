//! Typed dispatch for diff event subtypes (sync row 75 — diff polling).
//!
//! Mirrors the C `event_list[]` table at `pclsync/pdiff.c:2515-2529`,
//! which routes each `event` numeric tag to a typed processor:
//!
//! ```text
//!  1 createfolder      14 cancelledshareout
//!  2 modifyfolder      15 removedsharein
//!  3 deletefolder      16 removedshareout
//!  4 createfile        17 modifiedsharein
//!  5 modifyfile        18 modifiedshareout
//!  6 deletefile        19 establishbsharein
//!  7 modifyuserinfo    20 establishbshareout
//!  8 requestsharein    21 modifybsharein
//!  9 requestshareout   22 modifybshareout
//! 10 acceptedsharein   23 removebsharein
//! 11 acceptedshareout  24 removebshareout
//! 12 declinedsharein   25 cryptopasschange
//! 13 declinedshareout  26 modifyaccountinfo
//! ```
//!
//! The Rust path classifies each diff entry into a [`crate::diff_events::DiffEventKind`]
//! and forwards it to a [`crate::diff_events::DiffEventDispatcher`] implementation. The
//! file/folder CRUD events feed the existing local-store mutation
//! pipeline; share / crypto / account-info events are dispatched as
//! typed hooks so the share, crypto, and account backends can react
//! without leaking C event-numbering into every backend.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_model::ids::SyncId;

use crate::diff_poller::RemoteDiffEntry;

/// The C-event tag carried by each diff entry, classified into the
/// typed family the daemon dispatches against.
///
/// # Emitter / handler semantics
///
/// Each variant documents both (a) **when pCloud emits it** over the
/// `diff` stream and (b) **what the client should do** in response.
/// The handler side is enforced by dispatching through
/// [`DiffEventDispatcher`]: filesystem CRUD events mutate the local
/// store, share/crypto/account events are forwarded to the matching
/// backend, and anything unrecognized lands in
/// [`DiffEventFamily::Unknown`] rather than being silently dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffEventKind {
    /// C event 1 — `createfolder`.
    ///
    /// **Emitted when:** a new folder is created on the server (via web
    /// UI, another client, or API). **Client action:** create the
    /// mirroring directory in the local tree for every sync root whose
    /// scope covers the folder and persist the new folder id in the
    /// local store.
    CreateFolder,
    /// C event 2 — `modifyfolder`.
    ///
    /// **Emitted when:** a folder is renamed, moved, or has its metadata
    /// updated on the server. **Client action:** rename/move the local
    /// directory in place and refresh store metadata; children paths
    /// must be re-keyed.
    ModifyFolder,
    /// C event 3 — `deletefolder`.
    ///
    /// **Emitted when:** a folder (and its subtree) is removed on the
    /// server. **Client action:** recursively delete the local
    /// directory and evict all descendants from the store. Pending
    /// uploads targeting the subtree are cancelled.
    DeleteFolder,
    /// C event 4 — `createfile`.
    ///
    /// **Emitted when:** a new file appears on the server.
    /// **Client action:** schedule a download for each sync root that
    /// covers the file's path.
    CreateFile,
    /// C event 5 — `modifyfile`.
    ///
    /// **Emitted when:** a file's content or metadata is updated on the
    /// server (new `hash`, renamed, moved). **Client action:** schedule
    /// a re-download; if the local file also has pending changes the
    /// planner will emit a
    /// [`pcloud_model::conflict::ConflictKind::LocalModifyVsRemoteModify`].
    ModifyFile,
    /// C event 6 — `deletefile`.
    ///
    /// **Emitted when:** a file is deleted on the server.
    /// **Client action:** delete the local copy if present and cancel
    /// any queued uploads or downloads for the path.
    DeleteFile,
    /// C event 7 — `modifyuserinfo`.
    ///
    /// **Emitted when:** the authenticated user's profile is updated
    /// (email, language, plan). **Client action:** refresh the cached
    /// `userinfo` snapshot; no filesystem state changes.
    ModifyUserInfo,
    /// C event 8 — `requestsharein`.
    ///
    /// **Emitted when:** somebody offers a share to the current user.
    /// **Client action:** surface the request in the share-request
    /// inbox; do not auto-accept.
    ShareRequestIn,
    /// C event 9 — `requestshareout`.
    ///
    /// **Emitted when:** the current user creates an outgoing share
    /// request. **Client action:** persist the outgoing request so the
    /// UI can track its pending state.
    ShareRequestOut,
    /// C event 10 — `acceptedsharein`.
    ///
    /// **Emitted when:** an incoming share request is accepted by the
    /// current user. **Client action:** register the newly shared
    /// folder as an incoming share entry and begin mirroring it.
    ShareAcceptedIn,
    /// C event 11 — `acceptedshareout`.
    ///
    /// **Emitted when:** the recipient accepts a share this user sent.
    /// **Client action:** move the request from "pending" to
    /// "established outgoing" and record the recipient user id.
    ShareAcceptedOut,
    /// C event 12 — `declinedsharein`.
    ///
    /// **Emitted when:** the current user declines an incoming share
    /// request. **Client action:** drop the request from the inbox.
    ShareDeclinedIn,
    /// C event 13 — `declinedshareout`.
    ///
    /// **Emitted when:** the recipient declines a share request this
    /// user sent. **Client action:** drop the outgoing request and
    /// surface the decline in the UI.
    ShareDeclinedOut,
    /// C event 14 — `cancelledsharein`.
    ///
    /// **Emitted when:** an incoming share request is cancelled by the
    /// sender before the user acts on it. **Client action:** remove
    /// the request from the inbox.
    ShareCancelledIn,
    /// C event 15 — `cancelledshareout`.
    ///
    /// **Emitted when:** the current user cancels a previously-sent
    /// share request. **Client action:** remove the outgoing request.
    ShareCancelledOut,
    /// C event 16 — `removedsharein`.
    ///
    /// **Emitted when:** an established incoming share is revoked by
    /// the owner. **Client action:** unmirror the folder, drop its
    /// entries from the local store, and remove the share row.
    ShareRemovedIn,
    /// C event 17 — `removedshareout`.
    ///
    /// **Emitted when:** the current user revokes an outgoing share.
    /// **Client action:** remove the share row and surface the
    /// revocation.
    ShareRemovedOut,
    /// C event 18 — `modifiedsharein`.
    ///
    /// **Emitted when:** permissions on an established incoming share
    /// change. **Client action:** update the cached permission mask
    /// (see [`pcloud_model::shares::SharePermissions`]) and reconcile
    /// any in-flight write operations against the new capabilities.
    ShareModifiedIn,
    /// C event 19 — `modifiedshareout`.
    ///
    /// **Emitted when:** the current user changes permissions on an
    /// outgoing share. **Client action:** update the outgoing share
    /// row's permission mask.
    ShareModifiedOut,
    /// C event 20 — `establishbsharein` (business share in).
    ///
    /// **Emitted when:** the user's team establishes a shared folder
    /// reachable to the current user. **Client action:** register the
    /// business share and begin mirroring.
    BusinessShareEstablishedIn,
    /// C event 21 — `establishbshareout` (business share out).
    ///
    /// **Emitted when:** the current user, acting as team owner,
    /// establishes a business share. **Client action:** record the
    /// outgoing business share for management surfaces.
    BusinessShareEstablishedOut,
    /// C event 22 — `modifybsharein`.
    ///
    /// **Emitted when:** permissions on an established incoming
    /// business share change. **Client action:** refresh permissions
    /// and reconcile in-flight writes.
    BusinessShareModifiedIn,
    /// C event 23 — `modifybshareout`.
    ///
    /// **Emitted when:** permissions on an outgoing business share
    /// change. **Client action:** update the outgoing share row.
    BusinessShareModifiedOut,
    /// C event 24 — `removebsharein`.
    ///
    /// **Emitted when:** an incoming business share is revoked.
    /// **Client action:** unmirror the folder and drop the row.
    BusinessShareRemovedIn,
    /// C event 25 — `removebshareout`.
    ///
    /// **Emitted when:** the current user revokes an outgoing business
    /// share. **Client action:** remove the outgoing row.
    BusinessShareRemovedOut,
    /// C event 26 — `cryptopasschange`.
    ///
    /// **Emitted when:** the user's Crypto (client-side-encrypted
    /// folder) password changes. **Client action:** invalidate every
    /// cached Crypto key and force the crypto subsystem back to the
    /// locked state — decrypts will fail until the user re-enters the
    /// new password.
    CryptoPassChange,
    /// C event 27 — `modifyaccountinfo`.
    ///
    /// **Emitted when:** account-level info (plan, quota, billing)
    /// changes. **Client action:** refresh the cached account info
    /// snapshot; no filesystem impact.
    ModifyAccountInfo,
    /// Unrecognized event tag — surfaced so the dispatcher can record
    /// it rather than silently drop it (CLAUDE.md "do not silently
    /// swallow" rule). **Client action:** log with structured
    /// diagnostics so a future parity pass can promote the numeric tag
    /// to a typed variant.
    Unknown(u64),
}

impl DiffEventKind {
    /// Map a numeric `event` tag from the binary protocol to the typed
    /// kind. `None` indicates the diff entry carried no event tag at all
    /// (initial-sync entries from `diff` without `event`).
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::diff_events::DiffEventKind;
    /// assert_eq!(DiffEventKind::from_event_id(None), None);
    /// assert_eq!(DiffEventKind::from_event_id(Some(1)), Some(DiffEventKind::CreateFolder));
    /// // Unknown tags are preserved as `Unknown(tag)` rather than dropped.
    /// assert!(matches!(DiffEventKind::from_event_id(Some(9999)), Some(DiffEventKind::Unknown(9999))));
    /// ```
    #[must_use]
    pub fn from_event_id(tag: Option<u64>) -> Option<Self> {
        let tag = tag?;
        Some(match tag {
            1 => Self::CreateFolder,
            2 => Self::ModifyFolder,
            3 => Self::DeleteFolder,
            4 => Self::CreateFile,
            5 => Self::ModifyFile,
            6 => Self::DeleteFile,
            7 => Self::ModifyUserInfo,
            8 => Self::ShareRequestIn,
            9 => Self::ShareRequestOut,
            10 => Self::ShareAcceptedIn,
            11 => Self::ShareAcceptedOut,
            12 => Self::ShareDeclinedIn,
            13 => Self::ShareDeclinedOut,
            14 => Self::ShareCancelledIn,
            15 => Self::ShareCancelledOut,
            16 => Self::ShareRemovedIn,
            17 => Self::ShareRemovedOut,
            18 => Self::ShareModifiedIn,
            19 => Self::ShareModifiedOut,
            20 => Self::BusinessShareEstablishedIn,
            21 => Self::BusinessShareEstablishedOut,
            22 => Self::BusinessShareModifiedIn,
            23 => Self::BusinessShareModifiedOut,
            24 => Self::BusinessShareRemovedIn,
            25 => Self::BusinessShareRemovedOut,
            26 => Self::CryptoPassChange,
            27 => Self::ModifyAccountInfo,
            other => Self::Unknown(other),
        })
    }

    /// Coarse family — file/folder CRUD vs share vs crypto vs account.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::diff_events::{DiffEventFamily, DiffEventKind};
    /// assert_eq!(DiffEventKind::CreateFile.family(), DiffEventFamily::FilesystemCrud);
    /// assert_eq!(DiffEventKind::CryptoPassChange.family(), DiffEventFamily::Crypto);
    /// ```
    #[must_use]
    pub fn family(&self) -> DiffEventFamily {
        match self {
            Self::CreateFolder
            | Self::ModifyFolder
            | Self::DeleteFolder
            | Self::CreateFile
            | Self::ModifyFile
            | Self::DeleteFile => DiffEventFamily::FilesystemCrud,
            Self::ShareRequestIn
            | Self::ShareRequestOut
            | Self::ShareAcceptedIn
            | Self::ShareAcceptedOut
            | Self::ShareDeclinedIn
            | Self::ShareDeclinedOut
            | Self::ShareCancelledIn
            | Self::ShareCancelledOut
            | Self::ShareRemovedIn
            | Self::ShareRemovedOut
            | Self::ShareModifiedIn
            | Self::ShareModifiedOut
            | Self::BusinessShareEstablishedIn
            | Self::BusinessShareEstablishedOut
            | Self::BusinessShareModifiedIn
            | Self::BusinessShareModifiedOut
            | Self::BusinessShareRemovedIn
            | Self::BusinessShareRemovedOut => DiffEventFamily::Share,
            Self::CryptoPassChange => DiffEventFamily::Crypto,
            Self::ModifyUserInfo | Self::ModifyAccountInfo => DiffEventFamily::Account,
            Self::Unknown(_) => DiffEventFamily::Unknown,
        }
    }
}

/// Coarse family used by the daemon to route to the correct backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffEventFamily {
    /// File/folder create/modify/delete against the local store.
    FilesystemCrud,
    /// Share-request, share-accept, share-folder, or business-share event.
    Share,
    /// Crypto password change — invalidates cached crypto keys.
    Crypto,
    /// Account or user-info update.
    Account,
    /// Tag not recognized by this Rust build; recorded for observability.
    Unknown,
}

/// One classified diff event, ready for dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedDiffEvent {
    /// Sync root the event applies to.
    pub sync_id: SyncId,
    /// Typed event kind derived from the C event tag.
    pub kind: DiffEventKind,
    /// Raw remote diff entry that produced this event.
    pub entry: RemoteDiffEntry,
}

/// Dispatcher trait implemented by the daemon. Each method receives a
/// classified event; stub implementations are acceptable for share /
/// crypto / account hooks (those backends own the actual mutation
/// path and are out of scope for the diff worker).
pub trait DiffEventDispatcher {
    /// CRUD against local store (files/folders).
    fn handle_filesystem(&mut self, ev: &ClassifiedDiffEvent);
    /// Share-request / share-accept / share-folder events.
    fn handle_share(&mut self, ev: &ClassifiedDiffEvent);
    /// `cryptopasschange`: invalidate any cached crypto keys.
    fn handle_crypto(&mut self, ev: &ClassifiedDiffEvent);
    /// `modifyuserinfo` / `modifyaccountinfo`.
    fn handle_account(&mut self, ev: &ClassifiedDiffEvent);
    /// Unknown event tag — recorded so it never disappears silently.
    fn handle_unknown(&mut self, ev: &ClassifiedDiffEvent);
}

/// Classify and dispatch a batch of diff entries. Returns the number of
/// events that were dispatched (not counting entries with no event tag,
/// which are surfaced as `FilesystemCrud` upserts/deletes by the engine
/// elsewhere).
pub fn dispatch_diff_batch<D: DiffEventDispatcher>(
    sync_id: SyncId,
    entries: &[RemoteDiffEntry],
    event_tags: &[Option<u64>],
    dispatcher: &mut D,
) -> usize {
    debug_assert_eq!(entries.len(), event_tags.len());
    let mut dispatched = 0;
    for (entry, tag) in entries.iter().zip(event_tags.iter()) {
        let Some(kind) = DiffEventKind::from_event_id(*tag) else {
            continue;
        };
        let ev = ClassifiedDiffEvent {
            sync_id,
            kind,
            entry: entry.clone(),
        };
        match kind.family() {
            DiffEventFamily::FilesystemCrud => dispatcher.handle_filesystem(&ev),
            DiffEventFamily::Share => dispatcher.handle_share(&ev),
            DiffEventFamily::Crypto => dispatcher.handle_crypto(&ev),
            DiffEventFamily::Account => dispatcher.handle_account(&ev),
            DiffEventFamily::Unknown => dispatcher.handle_unknown(&ev),
        }
        dispatched += 1;
    }
    dispatched
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcloud_model::sync::{ChangeKind, EntryKind};

    #[derive(Default, Debug)]
    struct CountingDispatcher {
        fs: Vec<DiffEventKind>,
        share: Vec<DiffEventKind>,
        crypto: Vec<DiffEventKind>,
        account: Vec<DiffEventKind>,
        unknown: Vec<DiffEventKind>,
    }

    impl DiffEventDispatcher for CountingDispatcher {
        fn handle_filesystem(&mut self, ev: &ClassifiedDiffEvent) {
            self.fs.push(ev.kind);
        }
        fn handle_share(&mut self, ev: &ClassifiedDiffEvent) {
            self.share.push(ev.kind);
        }
        fn handle_crypto(&mut self, ev: &ClassifiedDiffEvent) {
            self.crypto.push(ev.kind);
        }
        fn handle_account(&mut self, ev: &ClassifiedDiffEvent) {
            self.account.push(ev.kind);
        }
        fn handle_unknown(&mut self, ev: &ClassifiedDiffEvent) {
            self.unknown.push(ev.kind);
        }
    }

    fn entry(name: &str) -> RemoteDiffEntry {
        RemoteDiffEntry {
            path: name.to_owned(),
            entry_kind: EntryKind::File,
            change_kind: ChangeKind::Upsert,
            remote_file_id: None,
            remote_folder_id: None,
            event: None,
        }
    }

    #[test]
    fn classifies_all_event_families() {
        let entries = vec![entry("a"), entry("b"), entry("c"), entry("d"), entry("e")];
        let tags = vec![Some(4), Some(8), Some(10), Some(26), Some(7)];
        let mut d = CountingDispatcher::default();
        let n = dispatch_diff_batch(SyncId::new(1), &entries, &tags, &mut d);
        assert_eq!(n, 5);
        assert_eq!(d.fs, vec![DiffEventKind::CreateFile]);
        assert_eq!(
            d.share,
            vec![
                DiffEventKind::ShareRequestIn,
                DiffEventKind::ShareAcceptedIn
            ]
        );
        assert_eq!(d.crypto, vec![DiffEventKind::CryptoPassChange]);
        assert_eq!(d.account, vec![DiffEventKind::ModifyUserInfo]);
        assert!(d.unknown.is_empty());
    }

    #[test]
    fn unknown_event_tags_are_recorded_not_dropped() {
        let entries = vec![entry("x")];
        let tags = vec![Some(999)];
        let mut d = CountingDispatcher::default();
        let n = dispatch_diff_batch(SyncId::new(1), &entries, &tags, &mut d);
        assert_eq!(n, 1);
        assert!(matches!(d.unknown[0], DiffEventKind::Unknown(999)));
    }

    #[test]
    fn entries_without_event_tag_are_skipped() {
        let entries = vec![entry("x"), entry("y")];
        let tags = vec![None, Some(4)];
        let mut d = CountingDispatcher::default();
        let n = dispatch_diff_batch(SyncId::new(1), &entries, &tags, &mut d);
        assert_eq!(n, 1);
        assert_eq!(d.fs, vec![DiffEventKind::CreateFile]);
    }

    #[test]
    fn share_folder_subtypes_route_to_share_handler() {
        // ShareAcceptedIn (tag 10) carries the new shared-folder
        // metadata in the C path; the dispatcher must route it as a
        // share event (so the share backend can persist it) rather than
        // as a filesystem CRUD.
        let entries = vec![entry("docs/")];
        let tags = vec![Some(10)];
        let mut d = CountingDispatcher::default();
        dispatch_diff_batch(SyncId::new(1), &entries, &tags, &mut d);
        assert_eq!(d.share, vec![DiffEventKind::ShareAcceptedIn]);
        assert!(d.fs.is_empty());
    }
}
