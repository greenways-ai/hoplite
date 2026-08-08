use super::*;
use sha2::Digest as _;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    fn new(label: &str) -> Self {
        let id = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "hoplite-blob-filesystem-{label}-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug)]
struct TestSource {
    bytes: Vec<u8>,
    cursor: usize,
    finished: usize,
}

impl TestSource {
    fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: bytes.into(),
            cursor: 0,
            finished: 0,
        }
    }
}

impl ByteSource for TestSource {
    fn read(&mut self, output: &mut [u8]) -> Result<usize, Error> {
        if self.finished != 0 {
            return Err(Error::SourceClosed);
        }
        if output.is_empty() || self.cursor == self.bytes.len() {
            return Ok(0);
        }
        let amount = output.len().min(self.bytes.len() - self.cursor);
        output[..amount].copy_from_slice(&self.bytes[self.cursor..self.cursor + amount]);
        self.cursor += amount;
        Ok(amount)
    }

    fn finish(&mut self) -> Result<(), Error> {
        if self.finished != 0 {
            return Err(Error::SourceClosed);
        }
        self.finished += 1;
        Ok(())
    }
}

fn limits() -> Limits {
    Limits {
        max_object_bytes: 1024,
        max_append_bytes: 64,
        max_source_chunk_bytes: 4,
        max_staging_key_bytes: 128,
        max_media_type_bytes: 128,
        max_staging_entries: 8,
        max_objects: 8,
    }
}

fn store(root: &TestRoot) -> FilesystemBlobStore {
    FilesystemBlobStore::open(root.path(), limits()).expect("filesystem store must open")
}

fn digest(bytes: &[u8]) -> Digest {
    let value = Sha256::digest(bytes);
    let mut digest = [0_u8; 32];
    digest.copy_from_slice(&value);
    Digest::from_bytes(digest)
}

fn key(value: &str) -> StagingKey {
    StagingKey::new(value.to_owned(), limits()).expect("test staging key must be valid")
}

fn media_type() -> MediaType {
    MediaType::new("application/octet-stream", limits())
        .expect("test media type must be valid")
}

fn open_request(name: &str, bytes: &[u8]) -> StagingOpen {
    StagingOpen::new(
        key(name),
        digest(bytes),
        bytes.len() as u64,
        media_type(),
        limits(),
    )
    .expect("test open request must be valid")
}

fn append_request(name: &str, offset: u64, length: usize) -> StagingAppend {
    StagingAppend::new(key(name), offset, length, limits())
        .expect("test append request must be valid")
}

fn commit_request(name: &str, bytes: &[u8]) -> StagingCommit {
    StagingCommit::new(key(name), digest(bytes), bytes.len() as u64, limits())
        .expect("test commit request must be valid")
}

fn append(store: &FilesystemBlobStore, name: &str, offset: u64, bytes: &[u8]) -> AppendReceipt {
    let mut source = TestSource::new(bytes);
    let receipt = store
        .staging_append_from_source(append_request(name, offset, bytes.len()), &mut source)
        .expect("append must succeed");
    assert_eq!(source.finished, 1);
    receipt
}

fn read_source(source: &mut FilesystemResponseSource, chunk: usize) -> Vec<u8> {
    let mut output = Vec::new();
    let mut buffer = vec![0_u8; chunk];
    loop {
        let read = source.read(&mut buffer).expect("source read must succeed");
        if read == 0 {
            break;
        }
        output.extend_from_slice(&buffer[..read]);
    }
    source.close().expect("source close must succeed");
    output
}

#[test]
fn creates_restart_safe_layout_and_verifies_an_empty_store() {
    let root = TestRoot::new("layout");
    {
        let store = store(&root);
        assert_eq!(store.root(), fs::canonicalize(root.path()).unwrap());
        assert!(store.root().join("staging").is_dir());
        assert!(store.root().join("objects/sha256").is_dir());
        assert!(store.root().join(LOCK_FILE).is_file());
        store.verify().expect("empty store must verify");
    }
    let reopened = store(&root);
    reopened.verify().expect("reopened empty store must verify");
}

#[cfg(unix)]
#[test]
fn rejects_symlink_roots_and_provider_owned_directories() {
    use std::os::unix::fs::symlink;

    let target = TestRoot::new("symlink-target");
    fs::create_dir_all(target.path()).unwrap();
    let link = TestRoot::new("symlink-root");
    symlink(target.path(), link.path()).unwrap();
    let error = FilesystemBlobStore::open(link.path(), limits()).unwrap_err();
    assert_eq!(error.code(), "blob-filesystem-root-invalid");
    fs::remove_file(link.path()).unwrap();

    let root = TestRoot::new("symlink-child");
    let store = store(&root);
    drop(store);
    fs::remove_dir(root.path().join("staging")).unwrap();
    symlink(target.path(), root.path().join("staging")).unwrap();
    let error = FilesystemBlobStore::open(root.path(), limits()).unwrap_err();
    assert_eq!(error.code(), "blob-filesystem-directory-invalid");
}

#[test]
fn resumes_appends_commits_and_reads_ranges_after_restart() {
    let root = TestRoot::new("restart");
    let bytes = b"hello-world";
    let object_digest = digest(bytes);

    {
        let store = store(&root);
        let opened = store
            .staging_open(open_request("upload-a", bytes))
            .expect("staging must open");
        assert_eq!(opened.offset, 0);
        let receipt = append(&store, "upload-a", 0, &bytes[..5]);
        assert_eq!(receipt.offset, 5);
    }

    {
        let store = store(&root);
        let resumed = store
            .staging_open(open_request("upload-a", bytes))
            .expect("staging must resume");
        assert_eq!(resumed.offset, 5);
        append(&store, "upload-a", 5, &bytes[5..]);
        let descriptor = store
            .staging_verify_commit(commit_request("upload-a", bytes))
            .expect("complete staging must commit");
        assert_eq!(descriptor.digest, object_digest);
        assert_eq!(descriptor.size, bytes.len() as u64);
    }

    let store = store(&root);
    let mut range = store
        .object_open_source(ObjectRange::new(object_digest, 1, 4).unwrap())
        .expect("range source must open");
    assert_eq!(range.declared_length(), 4);
    assert_eq!(read_source(&mut range, 2), b"ello");

    let mut complete = store
        .object_open_source(ObjectRange::new(object_digest, 0, bytes.len() as u64).unwrap())
        .expect("complete source must survive restart");
    assert_eq!(read_source(&mut complete, 3), bytes);
    store.verify().expect("committed store must verify");
}

#[test]
fn short_and_long_sources_finish_once_without_advancing_staging() {
    let root = TestRoot::new("source-bounds");
    let store = store(&root);
    let expected = b"four";
    store
        .staging_open(open_request("upload-a", expected))
        .expect("staging must open");

    let mut short = TestSource::new(b"abc".to_vec());
    let error = store
        .staging_append_from_source(append_request("upload-a", 0, 4), &mut short)
        .unwrap_err();
    assert_eq!(error.code(), "blob-source-short");
    assert_eq!(short.finished, 1);
    assert_eq!(
        store
            .staging_open(open_request("upload-a", expected))
            .unwrap()
            .offset,
        0
    );

    let mut long = TestSource::new(b"abcde".to_vec());
    let error = store
        .staging_append_from_source(append_request("upload-a", 0, 4), &mut long)
        .unwrap_err();
    assert_eq!(error.code(), "blob-source-long");
    assert_eq!(long.finished, 1);
    assert_eq!(
        store
            .staging_open(open_request("upload-a", expected))
            .unwrap()
            .offset,
        0
    );
}

#[test]
fn two_instances_serialize_writers_and_reject_stale_offsets() {
    let root = TestRoot::new("writers");
    let first = store(&root);
    first
        .staging_open(open_request("upload-a", b"abcd"))
        .expect("staging must open");
    let second = store(&root);

    append(&first, "upload-a", 0, b"ab");
    let mut source = TestSource::new(b"cd".to_vec());
    let error = second
        .staging_append_from_source(append_request("upload-a", 0, 2), &mut source)
        .unwrap_err();
    assert_eq!(error.code(), "blob-offset-mismatch");
    assert_eq!(source.finished, 0);
    assert_eq!(
        second
            .staging_open(open_request("upload-a", b"abcd"))
            .unwrap()
            .offset,
        2
    );
}

#[test]
fn restart_truncates_uncommitted_tail_to_the_last_metadata_offset() {
    let root = TestRoot::new("append-recovery");
    let store = store(&root);
    store
        .staging_open(open_request("upload-a", b"abcdef"))
        .unwrap();
    append(&store, "upload-a", 0, b"ab");
    let (_, data_path) = store.staging_paths(&key("upload-a"));
    {
        let mut data = OpenOptions::new().append(true).open(&data_path).unwrap();
        data.write_all(b"uncommitted").unwrap();
        data.sync_all().unwrap();
    }
    drop(store);

    let reopened = store(&root);
    assert_eq!(
        reopened
            .staging_open(open_request("upload-a", b"abcdef"))
            .unwrap()
            .offset,
        2
    );
    assert_eq!(fs::metadata(data_path).unwrap().len(), 2);
}

#[test]
fn commit_recovers_an_orphaned_digest_derived_object_link() {
    let root = TestRoot::new("commit-recovery");
    let bytes = b"recoverable";
    let store = store(&root);
    store
        .staging_open(open_request("upload-a", bytes))
        .unwrap();
    append(&store, "upload-a", 0, bytes);

    let (_, staging_data) = store.staging_paths(&key("upload-a"));
    let (object_metadata, object_data) = store.object_paths(digest(bytes));
    ensure_directory(object_data.parent().unwrap()).unwrap();
    fs::hard_link(&staging_data, &object_data).unwrap();
    sync_directory(object_data.parent().unwrap()).unwrap();
    assert!(!object_metadata.exists());

    let descriptor = store
        .staging_verify_commit(commit_request("upload-a", bytes))
        .expect("commit retry must recover object metadata");
    assert_eq!(descriptor.digest, digest(bytes));
    assert!(object_metadata.is_file());
    assert!(!staging_data.exists());

    let mut source = store
        .object_open_source(ObjectRange::new(digest(bytes), 0, bytes.len() as u64).unwrap())
        .unwrap();
    assert_eq!(read_source(&mut source, 4), bytes);
}

#[test]
fn tampered_object_bytes_fail_before_a_source_is_returned() {
    let root = TestRoot::new("tamper");
    let bytes = b"original";
    let store = store(&root);
    store
        .staging_open(open_request("upload-a", bytes))
        .unwrap();
    append(&store, "upload-a", 0, bytes);
    store
        .staging_verify_commit(commit_request("upload-a", bytes))
        .unwrap();

    let (_, object_data) = store.object_paths(digest(bytes));
    {
        let mut data = OpenOptions::new().write(true).open(object_data).unwrap();
        data.seek(SeekFrom::Start(0)).unwrap();
        data.write_all(b"X").unwrap();
        data.sync_all().unwrap();
    }

    let error = store
        .object_open_source(ObjectRange::new(digest(bytes), 0, bytes.len() as u64).unwrap())
        .unwrap_err();
    assert_eq!(error.code(), "blob-digest-mismatch");
}

#[test]
fn abort_is_idempotent_and_commit_replay_needs_no_staging_state() {
    let root = TestRoot::new("idempotency");
    let bytes = b"committed";
    let store = store(&root);
    store
        .staging_open(open_request("upload-a", bytes))
        .unwrap();
    append(&store, "upload-a", 0, bytes);
    let first = store
        .staging_verify_commit(commit_request("upload-a", bytes))
        .unwrap();
    let replay = store
        .staging_verify_commit(commit_request("upload-a", bytes))
        .expect("exact commit replay must use the installed object");
    assert_eq!(replay, first);

    store
        .staging_open(open_request("upload-b", b"discard"))
        .unwrap();
    append(&store, "upload-b", 0, b"dis");
    store.staging_abort(&key("upload-b")).unwrap();
    store.staging_abort(&key("upload-b")).unwrap();
    assert_eq!(
        store
            .staging_open(open_request("upload-b", b"discard"))
            .unwrap()
            .offset,
        0
    );
}

#[test]
fn orphaned_staging_data_is_removed_before_a_new_open() {
    let root = TestRoot::new("orphan-staging");
    let store = store(&root);
    let (metadata_path, data_path) = store.staging_paths(&key("upload-a"));
    fs::write(&data_path, b"orphan").unwrap();
    assert!(!metadata_path.exists());

    let opened = store
        .staging_open(open_request("upload-a", b"fresh"))
        .expect("new staging must replace orphan data");
    assert_eq!(opened.offset, 0);
    assert_eq!(fs::metadata(data_path).unwrap().len(), 0);
}
