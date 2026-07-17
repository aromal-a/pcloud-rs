//! Public-contract coverage for the protocol-backed filesystem adapters.
//!
//! The local coverage report excludes `tests/`, so every covered line in this
//! suite belongs to the production backend implementation.

use std::{
    collections::VecDeque,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use pcloud_fs::{
    backend::{
        FileBackend, FileHandle, FolderBackend, ProtoFileBackend, ProtoFolderBackend,
        ProtoUploadBackend, UploadTransport,
    },
    errors::FsError,
    write_path::{FileUploadBackend, UploadStatus, WritePathError},
};
use pcloud_proto::{
    EncodedRequest,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    http_download::HttpDownloadConfig,
    response::Value,
};
use pcloud_secret::secret_string::SecretString;

#[derive(Clone, Debug, Default)]
struct ScriptedTransport {
    state: Arc<ScriptedState>,
}

#[derive(Debug, Default)]
struct ScriptedState {
    responses: Mutex<VecDeque<Result<Value, ScriptError>>>,
    commands: Mutex<Vec<String>>,
    bodies: Mutex<Vec<Vec<u8>>>,
    hints: Mutex<Vec<String>>,
}

#[derive(Debug, Clone)]
struct ScriptError(&'static str);

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ScriptError {}

impl ScriptedTransport {
    fn new(responses: impl IntoIterator<Item = Result<Value, ScriptError>>) -> Self {
        Self {
            state: Arc::new(ScriptedState {
                responses: Mutex::new(responses.into_iter().collect()),
                ..ScriptedState::default()
            }),
        }
    }

    fn values(responses: impl IntoIterator<Item = Value>) -> Self {
        Self::new(responses.into_iter().map(Ok))
    }

    fn next(&self, request: &EncodedRequest) -> Result<Value, ScriptError> {
        self.state
            .commands
            .lock()
            .expect("commands lock")
            .push(request.frame.command.clone());
        self.state
            .responses
            .lock()
            .expect("responses lock")
            .pop_front()
            .unwrap_or(Err(ScriptError("script exhausted")))
    }

    fn commands(&self) -> Vec<String> {
        self.state.commands.lock().expect("commands lock").clone()
    }
}

impl ProtocolTransport for ScriptedTransport {
    type Error = ScriptError;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        self.next(request)
    }
}

impl ApiServerHintConsumer for ScriptedTransport {
    fn apply_api_server_hint(&self, api_server: &str) {
        self.state
            .hints
            .lock()
            .expect("hints lock")
            .push(api_server.to_owned());
    }
}

impl UploadTransport for ScriptedTransport {
    type Error = ScriptError;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        self.next(request)
    }

    fn execute_with_body(
        &self,
        request: &EncodedRequest,
        body: &[u8],
    ) -> Result<Value, Self::Error> {
        self.state
            .bodies
            .lock()
            .expect("bodies lock")
            .push(body.to_vec());
        self.next(request)
    }
}

fn hash(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Hash(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn ok() -> Value {
    hash([("result", Value::Number(0))])
}

fn folder(id: u64, name: &str) -> Value {
    hash([(
        "metadata",
        hash([
            ("folderid", Value::Number(id)),
            ("name", Value::String(name.to_owned())),
        ]),
    )])
}

fn listing(id: u64, name: &str, entries: Vec<Value>) -> Value {
    hash([
        ("result", Value::Number(0)),
        (
            "metadata",
            hash([
                ("folderid", Value::Number(id)),
                ("name", Value::String(name.to_owned())),
                ("contents", Value::Array(entries)),
            ]),
        ),
    ])
}

fn file_entry(name: &str, id: Option<u64>) -> Value {
    let mut fields = vec![
        ("name".to_owned(), Value::String(name.to_owned())),
        ("isfolder".to_owned(), Value::Bool(false)),
    ];
    if let Some(id) = id {
        fields.push(("fileid".to_owned(), Value::Number(id)));
    }
    Value::Hash(fields)
}

fn result(code: u64) -> Value {
    hash([
        ("result", Value::Number(code)),
        ("error", Value::String("fixture error".to_owned())),
    ])
}

fn token() -> SecretString {
    SecretString::new("fixture-token")
}

#[derive(Debug)]
struct ReadOnlyFolder;

impl FolderBackend for ReadOnlyFolder {
    fn list_contents(
        &self,
        _path: &str,
    ) -> Result<pcloud_proto::folder_api::RemoteFolderListing, FsError> {
        Err(FsError::NotFound)
    }
}

#[derive(Debug)]
struct MinimalFile;

impl FileBackend for MinimalFile {
    fn open(&self, file_id: u64) -> Result<FileHandle, FsError> {
        Ok(FileHandle {
            file_id,
            size: 1,
            host: "fixture.invalid".to_owned(),
            path: "/file".to_owned(),
            dwltag: None,
        })
    }

    fn read(&self, _handle: &FileHandle, _offset: u64, _len: usize) -> Result<Vec<u8>, FsError> {
        Ok(Vec::new())
    }
}

#[derive(Debug)]
struct LegacyUpload;

impl FileUploadBackend for LegacyUpload {
    fn upload_file(
        &self,
        _parent_path: &str,
        _name: &str,
        _staging_file: &Path,
    ) -> Result<(), WritePathError> {
        Ok(())
    }

    fn unlink_remote(&self, _path: &str) -> Result<(), WritePathError> {
        Ok(())
    }

    fn rename_remote(&self, _from: &str, _to: &str) -> Result<(), WritePathError> {
        Ok(())
    }
}

#[test]
fn default_backend_contracts_are_explicit_and_safe() {
    let folder = ReadOnlyFolder;
    assert_eq!(folder.create_folder("/", "x"), Err(FsError::Invalid));
    assert_eq!(folder.delete_folder("/x"), Err(FsError::Invalid));
    assert_eq!(folder.statfs(), Err(FsError::Io));

    let file = MinimalFile;
    let handle = file.open_with_size(7, 99).expect("default open_with_size");
    assert_eq!((handle.file_id, handle.size), (7, 99));
    file.release(&handle).expect("default release");

    let upload = LegacyUpload;
    for error in [
        upload.upload_create("/", "x").expect_err("unsupported"),
        upload.upload_write(1, 0, b"x").expect_err("unsupported"),
        upload.upload_save(1, "/", "x", 1).expect_err("unsupported"),
        upload.upload_status(1).expect_err("unsupported"),
    ] {
        assert!(matches!(error, WritePathError::Upload(_)));
    }
}

#[test]
fn folder_backend_lists_creates_and_deletes() {
    let transport = ScriptedTransport::values([
        listing(1, "Root", vec![file_entry("a.txt", Some(2))]),
        folder(41, "new"),
        folder(42, "nested"),
        folder(41, "new"),
        ok(),
    ]);
    let probe = transport.clone();
    let backend = ProtoFolderBackend::new(transport, token());

    let contents = backend.list_contents("/").expect("listing");
    assert_eq!(contents.entries[0].file_id, Some(2));
    assert_eq!(backend.create_folder("/", "new"), Ok(41));
    assert_eq!(backend.create_folder("/parent", "nested"), Ok(42));
    backend.delete_folder("/new").expect("delete");
    assert_eq!(
        probe.commands(),
        [
            "listfolder",
            "createfolder",
            "createfolder",
            "listfolder",
            "deletefolder"
        ]
    );
    assert!(!format!("{backend:?}").contains("fixture-token"));
}

#[test]
fn folder_backend_maps_result_transport_and_malformed_failures() {
    let not_found = ProtoFolderBackend::new(
        ScriptedTransport::values([result(2005), result(2005)]),
        token(),
    );
    assert_eq!(not_found.list_contents("/missing"), Err(FsError::NotFound));
    assert_eq!(
        not_found.create_folder("/", "missing"),
        Err(FsError::NotFound)
    );

    let transport = ProtoFolderBackend::new(
        ScriptedTransport::new([Err(ScriptError("offline"))]),
        token(),
    );
    assert!(matches!(
        transport.list_contents("/"),
        Err(FsError::Transport(message)) if message == "offline"
    ));

    let malformed =
        ProtoFolderBackend::new(ScriptedTransport::values([Value::Bool(true)]), token());
    assert_eq!(malformed.list_contents("/"), Err(FsError::Io));
}

fn download_link(hosts: Vec<Value>) -> Value {
    hash([
        ("result", Value::Number(0)),
        ("path", Value::String("/signed/file".to_owned())),
        ("hosts", Value::Array(hosts)),
        ("dwltag", Value::String("tag".to_owned())),
    ])
}

#[test]
fn file_backend_opens_sizes_and_handles_empty_reads() {
    let transport = ScriptedTransport::values([
        download_link(vec![Value::String("127.0.0.1".to_owned())]),
        download_link(vec![Value::String("127.0.0.1".to_owned())]),
    ]);
    let backend = ProtoFileBackend::new(transport, token());
    let handle = backend.open(4).expect("open");
    assert_eq!(
        (
            handle.file_id,
            handle.size,
            handle.host.as_str(),
            handle.path.as_str(),
            handle.dwltag.as_deref()
        ),
        (4, 0, "127.0.0.1", "/signed/file", Some("tag"))
    );
    assert!(backend.read(&handle, 0, 0).expect("empty read").is_empty());
    backend.release(&handle).expect("release");
    assert_eq!(
        backend.open_with_size(5, 123).expect("sized open").size,
        123
    );
    assert!(!format!("{backend:?}").contains("fixture-token"));
}

#[test]
fn file_backend_maps_link_and_download_failures() {
    let no_hosts =
        ProtoFileBackend::new(ScriptedTransport::values([download_link(vec![])]), token());
    assert!(matches!(
        no_hosts.open(1),
        Err(FsError::Transport(message)) if message.contains("no hosts")
    ));

    let denied = ProtoFileBackend::new(ScriptedTransport::values([result(2003)]), token());
    assert!(matches!(denied.open(1), Err(FsError::PermissionDenied)));

    let offline = ProtoFileBackend::new(
        ScriptedTransport::new([Err(ScriptError("offline"))]),
        token(),
    );
    assert!(matches!(offline.open(1), Err(FsError::Transport(_))));

    let malformed = ProtoFileBackend::new(ScriptedTransport::values([Value::Bool(false)]), token());
    assert!(matches!(malformed.open(1), Err(FsError::Io)));

    let config = HttpDownloadConfig {
        use_tls: false,
        connect_timeout: Duration::from_millis(20),
        read_timeout: Duration::from_millis(20),
        write_timeout: Duration::from_millis(20),
        total_request_timeout: Duration::from_millis(20),
        ..HttpDownloadConfig::default()
    };
    let backend = ProtoFileBackend::with_http_config(ScriptedTransport::default(), token(), config);
    let handle = FileHandle {
        file_id: 1,
        size: 1,
        host: "127.0.0.1".to_owned(),
        path: "/unreachable".to_owned(),
        dwltag: None,
    };
    assert!(matches!(
        backend.read(&handle, u64::MAX, 4),
        Err(FsError::Transport(_))
    ));
}

#[test]
fn whole_file_upload_streams_and_commits() {
    let transport = ScriptedTransport::values([
        folder(9, "docs"),
        hash([("uploadid", Value::Number(77))]),
        ok(),
        ok(),
    ]);
    let probe = transport.clone();
    let backend = ProtoUploadBackend::new(transport, token());
    let dir = tempfile::tempdir().expect("tempdir");
    let staged = dir.path().join("report.txt");
    std::fs::write(&staged, b"payload").expect("write fixture");

    backend
        .upload_file("/docs", "report.txt", &staged)
        .expect("whole-file upload");
    assert_eq!(
        probe.commands(),
        ["listfolder", "upload_create", "upload_write", "upload_save"]
    );
    assert_eq!(
        probe.state.bodies.lock().expect("bodies lock").as_slice(),
        [b"payload".as_slice()]
    );
    assert!(!format!("{backend:?}").contains("fixture-token"));
}

#[test]
fn whole_file_upload_rejects_local_and_remote_failures() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing");
    let stat_backend =
        ProtoUploadBackend::new(ScriptedTransport::values([folder(1, "root")]), token());
    assert!(
        stat_backend
            .upload_file("/", "x", &missing)
            .expect_err("missing staging file")
            .to_string()
            .contains("staging stat")
    );

    let large = dir.path().join("large");
    let file = std::fs::File::create(&large).expect("create sparse fixture");
    file.set_len(4 * 1024 * 1024 + 1)
        .expect("set sparse length");
    let large_backend =
        ProtoUploadBackend::new(ScriptedTransport::values([folder(1, "root")]), token());
    assert!(
        large_backend
            .upload_file("/", "x", &large)
            .expect_err("large staging file")
            .to_string()
            .contains("too large")
    );

    let staged = dir.path().join("small");
    std::fs::write(&staged, b"x").expect("write fixture");
    let malformed_create = ProtoUploadBackend::new(
        ScriptedTransport::values([folder(1, "root"), Value::Bool(true)]),
        token(),
    );
    assert!(
        malformed_create
            .upload_file("/", "x", &staged)
            .expect_err("malformed create")
            .to_string()
            .contains("not a hash")
    );

    let missing_id = ProtoUploadBackend::new(
        ScriptedTransport::values([folder(1, "root"), ok()]),
        token(),
    );
    assert!(
        missing_id
            .upload_file("/", "x", &staged)
            .expect_err("missing upload id")
            .to_string()
            .contains("missing uploadid")
    );

    for (code, permanent) in [(2008, true), (5001, false)] {
        let backend = ProtoUploadBackend::new(
            ScriptedTransport::values([
                folder(1, "root"),
                hash([("uploadid", Value::Number(7))]),
                result(code),
            ]),
            token(),
        );
        let error = backend
            .upload_file("/", "x", &staged)
            .expect_err("write result failure");
        assert_eq!(
            matches!(error, WritePathError::UploadPermanent(_)),
            permanent
        );
        assert_eq!(
            matches!(error, WritePathError::UploadTransient(_)),
            !permanent
        );
    }
}

#[test]
fn unlink_and_rename_resolve_remote_identifiers() {
    let transport = ScriptedTransport::values([
        listing(2, "docs", vec![file_entry("old.txt", Some(12))]),
        ok(),
        listing(2, "docs", vec![file_entry("old.txt", Some(12))]),
        ok(),
        listing(2, "docs", vec![file_entry("old.txt", Some(12))]),
        folder(3, "archive"),
        ok(),
    ]);
    let probe = transport.clone();
    let backend = ProtoUploadBackend::new(transport, token());

    backend.unlink_remote("/docs/old.txt").expect("unlink");
    backend
        .rename_remote("/docs/old.txt", "/docs/new.txt")
        .expect("same-folder rename");
    backend
        .rename_remote("/docs/old.txt", "/archive/new.txt")
        .expect("move and rename");
    assert_eq!(
        probe.commands(),
        [
            "listfolder",
            "deletefile",
            "listfolder",
            "renamefile",
            "listfolder",
            "listfolder",
            "renamefile"
        ]
    );
}

#[test]
fn unlink_and_rename_validate_paths_and_entries() {
    let backend = ProtoUploadBackend::new(ScriptedTransport::default(), token());
    assert!(backend.unlink_remote("/").is_err());
    assert!(backend.rename_remote("/", "/ok").is_err());
    assert!(backend.rename_remote("/ok", "/").is_err());

    let missing = ProtoUploadBackend::new(
        ScriptedTransport::values([listing(2, "docs", vec![])]),
        token(),
    );
    assert!(missing.unlink_remote("/docs/missing").is_err());

    let no_id = ProtoUploadBackend::new(
        ScriptedTransport::values([listing(2, "docs", vec![file_entry("missing-id", None)])]),
        token(),
    );
    assert!(no_id.unlink_remote("/docs/missing-id").is_err());
}

#[test]
fn chunked_upload_lifecycle_tracks_progress_and_status() {
    let transport = ScriptedTransport::values([
        folder(9, "docs"),
        hash([("uploadid", Value::Number(77))]),
        ok(),
        hash([
            ("result", Value::Number(0)),
            ("uploadoffset", Value::Number(3)),
        ]),
        ok(),
        hash([
            ("result", Value::Number(0)),
            ("uploadoffset", Value::Number(6)),
        ]),
    ]);
    let probe = transport.clone();
    let backend = ProtoUploadBackend::new(transport, token());

    let upload_id = backend.upload_create("/docs", "x").expect("create");
    backend.upload_write(upload_id, 0, b"abc").expect("write");
    assert_eq!(
        backend.upload_status(upload_id).expect("status"),
        UploadStatus::Bytes(3)
    );
    backend
        .upload_save(upload_id, "/docs", "x", 3)
        .expect("save");
    assert_eq!(
        backend.upload_status(upload_id).expect("status after save"),
        UploadStatus::Bytes(6)
    );
    assert_eq!(
        probe.commands(),
        [
            "listfolder",
            "upload_create",
            "upload_write",
            "upload_info",
            "upload_save",
            "upload_info"
        ]
    );
}

#[test]
fn chunked_upload_handles_missing_sessions_and_server_failures() {
    let fallback_save = ProtoUploadBackend::new(
        ScriptedTransport::values([folder(3, "archive"), ok()]),
        token(),
    );
    fallback_save
        .upload_save(99, "/archive", "x", 0)
        .expect("fallback parent resolution");

    let no_session = ProtoUploadBackend::new(
        ScriptedTransport::values([ok(), result(2069), result(5000)]),
        token(),
    );
    no_session
        .upload_write(44, 0, b"x")
        .expect("best-effort missing session");
    assert_eq!(
        no_session.upload_status(44).expect("not found status"),
        UploadStatus::NotFound
    );
    assert!(matches!(
        no_session.upload_write(44, 1, b"x"),
        Err(WritePathError::UploadTransient(_))
    ));

    let malformed =
        ProtoUploadBackend::new(ScriptedTransport::values([Value::Bool(false)]), token());
    assert!(malformed.upload_status(1).is_err());

    let offline = ProtoUploadBackend::new(
        ScriptedTransport::new([Err(ScriptError("offline"))]),
        token(),
    );
    assert!(offline.upload_status(1).is_err());
}

#[test]
fn scripted_transport_reports_exhaustion_as_io_compatible_error() {
    let transport = ScriptedTransport::default();
    let backend = ProtoFolderBackend::new(transport, token());
    let error = backend.list_contents("/").expect_err("script exhausted");
    assert_eq!(error, FsError::Transport("script exhausted".to_owned()));
}
