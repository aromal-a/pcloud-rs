use std::sync::{Arc, Mutex};

use pcloud_backends::transfer_backend::{
    ChunkedConflictMode, ChunkedUploadError, ChunkedUploadRequest, TransferBackendError,
    TransferRuntime,
};
use pcloud_backends::upload_journal::JournalEntry;
use pcloud_backends::upload_state::{SessionRefresher, UploadStateMachine};
use pcloud_config::{ConfigProfile, Environment};
use pcloud_proto::DownloadLink;
use pcloud_proto::methods::upload::ConflictParam;
use pcloud_resilience::BandwidthPacer;
use pcloud_secret::secret_string::SecretString;

fn token() -> SecretString {
    SecretString::new("coverage-token")
}

struct Refresher;

impl SessionRefresher for Refresher {
    fn refresh(&mut self) -> Result<SecretString, String> {
        Ok(token())
    }
}

#[test]
fn development_transfer_runtime_exposes_complete_streaming_surface() {
    let root = tempfile::tempdir().unwrap();
    let config =
        ConfigProfile::secure_defaults(root.path().to_path_buf(), Environment::Development);
    let mut runtime = TransferRuntime::from_config(&config);
    assert!(runtime.is_development());
    assert!(runtime.network_transport().is_none());
    assert!(!runtime.http_download_config().use_tls);
    runtime.apply_api_server_hint("api.example.test");

    let pacer = Arc::new(BandwidthPacer::new(None));
    runtime.set_bandwidth_pacer(Some(pacer.clone()));
    assert!(Arc::ptr_eq(&runtime.bandwidth_pacer().unwrap(), &pacer));
    runtime.set_bandwidth_pacer(None);
    assert!(runtime.bandwidth_pacer().is_none());
    let runtime = runtime.with_bandwidth_pacer(Some(pacer));

    let link = runtime.get_file_link(token(), 1, None).unwrap();
    assert_eq!(link.hosts.len(), 2);
    assert!(runtime.get_file_link(token(), 999, None).is_err());
    let (signed, bytes) = runtime.download_bytes(&link).unwrap();
    assert_eq!(signed.host, "c1.pcloud.com");
    assert!(!bytes.is_empty());
    let hostless = DownloadLink {
        path: "/hostless".into(),
        hosts: Vec::new(),
        download_tag: None,
        api_server: None,
    };
    assert_eq!(
        runtime.download_bytes(&hostless).unwrap().0.host,
        "c1.pcloud.com"
    );
    assert_eq!(runtime.download_for_range(&link, 2, 7).range, Some((2, 7)));
    assert_eq!(
        runtime.download_for_range(&hostless, 0, 1).host,
        "c1.pcloud.com"
    );
    assert_eq!(runtime.read_range(&link, 0, 4).unwrap().len(), 4);
    assert!(matches!(
        runtime.read_range(&link, 4, 3),
        Err(TransferBackendError::Malformed(_))
    ));
    assert!(
        runtime
            .read_range(&link, u64::MAX, u64::MAX)
            .unwrap()
            .is_empty()
    );

    let observed = Arc::new(Mutex::new(Vec::new()));
    let observer = {
        let observed = observed.clone();
        Arc::new(move |delta| observed.lock().unwrap().push(delta))
    };
    let destination = root.path().join("nested/download.bin");
    let (_, written) = runtime
        .download_to_path_with_observer(&link, &destination, Some(observer))
        .unwrap();
    assert_eq!(std::fs::metadata(&destination).unwrap().len(), written);
    assert_eq!(observed.lock().unwrap().as_slice(), &[written]);
    assert!(
        runtime
            .download_to_path(&link, root.path().join("nested/plain.bin").as_path())
            .is_ok()
    );
    let blocked_parent = root.path().join("parent-file");
    std::fs::write(&blocked_parent, b"x").unwrap();
    assert!(
        runtime
            .download_to_path(&link, &blocked_parent.join("child"))
            .is_err()
    );

    let session = runtime.upload_create(token(), 0, "file.bin", 4).unwrap();
    assert!(
        runtime
            .upload_create_idempotent(token(), 0, "file.bin", 4, "stable-key".into())
            .is_ok()
    );
    assert!(
        runtime
            .upload_create(token(), 0, "fail-upload.txt", 4)
            .is_err()
    );
    assert!(runtime.upload_info(token(), session.upload_id, 0).is_err());

    let upload_observed = Arc::new(Mutex::new(Vec::new()));
    let upload_observer = {
        let upload_observed = upload_observed.clone();
        Arc::new(move |delta| upload_observed.lock().unwrap().push(delta))
    };
    assert_eq!(
        runtime
            .upload_bytes_with_observer_and_conflict(
                token(),
                &session,
                b"data",
                Some(upload_observer),
                Some(ConflictParam::New),
            )
            .unwrap()
            .payload_len,
        4
    );
    assert_eq!(upload_observed.lock().unwrap().as_slice(), &[4]);
    assert_eq!(
        runtime
            .upload_bytes(token(), &session, b"x")
            .unwrap()
            .payload_len,
        1
    );
    assert_eq!(
        runtime
            .upload_bytes_with_observer(token(), &session, b"xy", None)
            .unwrap()
            .payload_len,
        2
    );
    assert_eq!(
        runtime
            .upload_write_chunk(token(), session.upload_id, 5, 0, b"abc")
            .unwrap(),
        8
    );
    assert_eq!(
        runtime
            .upload_write_chunk_idempotent(
                token(),
                session.upload_id,
                u64::MAX,
                0,
                b"abc",
                Some("key".into()),
            )
            .unwrap(),
        u64::MAX
    );
    runtime
        .upload_save_session(token(), &session, Some(ConflictParam::IfHash(7)), 1)
        .unwrap();
    runtime
        .upload_save_session_idempotent(token(), &session, None, 1, Some("key".into()))
        .unwrap();
    assert!(matches!(
        runtime.upload_write_from_file(token(), 1, 0, 0, 2, 3, 0, 4),
        Err(TransferBackendError::NetworkExecutionUnavailable)
    ));
    assert!(runtime.upload_delete(token(), session.upload_id).is_err());
    assert!(runtime.delete_file_by_id(token(), 1).is_err());
    assert!(runtime.rename_file_by_id(token(), 1, 0, "new").is_err());

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let clock = Arc::new(pcloud_resilience::clock::ManualClock::new());
    let mut machine = UploadStateMachine::with_defaults(clock);
    let request = ChunkedUploadRequest {
        local_path: "/tmp/file".into(),
        parent_folder_id: 0,
        file_name: "file".into(),
        total_size: 1,
        modified_at_unix: 1,
        ctime: None,
        conflict: ChunkedConflictMode::CreateIfNew,
    };
    assert!(matches!(
        runtime.upload_bytes_chunked(
            &conn,
            &mut machine,
            token(),
            request,
            b"x",
            |_| {},
            &mut Refresher,
        ),
        Err(ChunkedUploadError::NoNetworkTransport)
    ));
}

#[test]
fn upload_journal_helpers_partition_known_and_unknown_entries() {
    let root = tempfile::tempdir().unwrap();
    let journal = TransferRuntime::open_upload_journal(root.path().join("journal")).unwrap();
    for upload_id in [1, 2] {
        journal
            .append(&JournalEntry {
                upload_id,
                chunks_done: 1,
                bytes: 4,
                sha_partial: Some("abcd".into()),
                descriptor: None,
                committed: false,
            })
            .unwrap();
    }
    let (known, unknown, report) = TransferRuntime::replay_upload_journal(&journal, &[1]).unwrap();
    assert_eq!(known.len(), 1);
    assert_eq!(unknown.len(), 1);
    assert_eq!(report.entries.len(), 2);
}
