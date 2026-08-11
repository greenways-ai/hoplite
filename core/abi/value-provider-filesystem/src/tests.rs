use super::*;
use hoplite_blob_store::{
    BlobStore, ByteSource, Error as BlobError, Limits as BlobLimits, MediaType, ObjectRange,
    ResponseSource, StagingAppend, StagingCommit, StagingKey, StagingOpen,
};
use hoplite_blob_store_filesystem::FilesystemBlobStore;
use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicU64, Ordering};

const VECTOR: u8 = 9;
const SYMBOL: u8 = 7;
const NIL: u8 = 0;

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    fn new(label: &str) -> Self {
        let id = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "hoplite-value-provider-{label}-{}-{id}",
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

struct TestSource {
    bytes: Vec<u8>,
    cursor: usize,
    finished: bool,
}

impl TestSource {
    fn new(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            cursor: 0,
            finished: false,
        }
    }
}

impl ByteSource for TestSource {
    fn read(&mut self, output: &mut [u8]) -> Result<usize, BlobError> {
        if self.finished {
            return Err(BlobError::SourceClosed);
        }
        if output.is_empty() || self.cursor == self.bytes.len() {
            return Ok(0);
        }
        let amount = output.len().min(self.bytes.len() - self.cursor);
        output[..amount].copy_from_slice(&self.bytes[self.cursor..self.cursor + amount]);
        self.cursor += amount;
        Ok(amount)
    }

    fn finish(&mut self) -> Result<(), BlobError> {
        if self.finished {
            return Err(BlobError::SourceClosed);
        }
        self.finished = true;
        Ok(())
    }
}

fn blob_limits() -> BlobLimits {
    BlobLimits {
        max_object_bytes: 4096,
        max_append_bytes: 4096,
        max_source_chunk_bytes: 17,
        max_staging_key_bytes: 128,
        max_media_type_bytes: 128,
        max_staging_entries: 32,
        max_objects: 32,
    }
}

fn value_limits() -> Limits {
    Limits {
        max_frame_bytes: 2048,
        max_media_type_bytes: 128,
        io_chunk_bytes: 11,
    }
}

fn digest(bytes: &[u8]) -> Digest {
    let value = Sha256::digest(bytes);
    let mut digest = [0_u8; 32];
    digest.copy_from_slice(&value);
    Digest::from_bytes(digest)
}

fn install(root: &TestRoot, label: &str, bytes: &[u8]) -> Digest {
    let limits = blob_limits();
    let store = FilesystemBlobStore::open(root.path(), limits).expect("blob store must open");
    let object_digest = digest(bytes);
    let staging_key = StagingKey::new(label.to_owned(), limits).expect("valid staging key");
    let media_type = MediaType::new("application/vnd.hara.hta", limits).expect("valid media type");
    store
        .staging_open(
            StagingOpen::new(
                staging_key.clone(),
                object_digest,
                bytes.len() as u64,
                media_type,
                limits,
            )
            .unwrap(),
        )
        .unwrap();
    let mut source = TestSource::new(bytes);
    store
        .staging_append_from_source(
            StagingAppend::new(staging_key.clone(), 0, bytes.len(), limits).unwrap(),
            &mut source,
        )
        .unwrap();
    assert!(source.finished);
    store
        .staging_verify_commit(
            StagingCommit::new(staging_key, object_digest, bytes.len() as u64, limits).unwrap(),
        )
        .unwrap();
    object_digest
}

fn provider(root: &TestRoot) -> FilesystemValueProvider {
    FilesystemValueProvider::open(root.path(), value_limits()).expect("value provider must open")
}

fn portable_value() -> Vec<u8> {
    result_map(vec![
        ("children", bare_vector(vec![vec![NIL], bare_bool(true)])),
        ("name", bare_string("tree")),
        ("version", bare_usize(1).unwrap()),
    ])
    .unwrap()
}

fn bare_vector(values: Vec<Vec<u8>>) -> Vec<u8> {
    let mut output = vec![VECTOR];
    output.extend_from_slice(&(values.len() as u32).to_be_bytes());
    for value in values {
        output.extend_from_slice(&value);
    }
    output
}

fn request_arguments(digest: Digest, maximum: usize) -> Vec<u8> {
    request_arguments_with(vec![
        ("digest", bare_string(&digest.to_string())),
        ("max-bytes", bare_usize(maximum).unwrap()),
        ("operation", bare_string(OPERATION)),
        ("protocol", bare_string(REQUEST_PROTOCOL)),
    ])
}

fn request_arguments_with(entries: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
    let request = result_map(entries).unwrap();
    let mut arguments = MAGIC.to_vec();
    arguments.push(VECTOR);
    arguments.extend_from_slice(&1_u32.to_be_bytes());
    arguments.extend_from_slice(&request[MAGIC.len()..]);
    arguments
}

fn assert_failure(frame: &[u8], expected_digest: Digest, expected_code: &str) {
    let document = Document::parse(frame).unwrap();
    let root = document.root();
    assert_eq!(root.kind(), Kind::Map);
    assert_eq!(root.len().unwrap(), 5);
    assert_eq!(
        root.require("protocol").unwrap().as_text().unwrap(),
        RESULT_PROTOCOL
    );
    assert_eq!(
        root.require("operation").unwrap().as_text().unwrap(),
        OPERATION
    );
    assert!(!root.require("verified").unwrap().as_bool().unwrap());
    assert_eq!(
        root.require("digest").unwrap().as_text().unwrap(),
        expected_digest.to_string()
    );
    assert_eq!(
        root.require("code").unwrap().as_text().unwrap(),
        expected_code
    );
    assert!(root.map_get("path").unwrap().is_none());
    assert!(root.map_get("provider").unwrap().is_none());
}

fn object_paths(root: &TestRoot, digest: Digest) -> (PathBuf, PathBuf) {
    let text = digest.to_string();
    let hex = text.strip_prefix("sha256:").unwrap();
    let directory = root.path().join("objects/sha256").join(&hex[..2]);
    (
        directory.join(format!("{}.meta", &hex[2..])),
        directory.join(format!("{}.blob", &hex[2..])),
    )
}

fn rewrite_metadata_size(path: &Path, size: u64) {
    let mut bytes = fs::read(path).unwrap();
    bytes[36..44].copy_from_slice(&size.to_be_bytes());
    fs::write(path, bytes).unwrap();
}

fn read_blob_source(store: &FilesystemBlobStore, digest: Digest, length: u64) -> Vec<u8> {
    let mut source = store
        .object_open_source(ObjectRange::new(digest, 0, length).unwrap())
        .unwrap();
    let mut output = Vec::new();
    let mut buffer = [0_u8; 7];
    loop {
        let read = source.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        output.extend_from_slice(&buffer[..read]);
    }
    source.close().unwrap();
    output
}

include!("reader_cases.rs");
include!("verification_cases.rs");
include!("integrity_cases.rs");
include!("restart_cases.rs");
