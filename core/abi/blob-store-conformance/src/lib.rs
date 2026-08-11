#![forbid(unsafe_code)]

//! Shared behavioural conformance for implementations of `hoplite.blob`.
//!
//! The harness owns only portable `BlobStore` expectations. Driver-specific
//! crash recovery, trusted-root layout and tamper evidence remain in each
//! driver's own test suite.

use hoplite_blob_store::{
    AppendReceipt, BlobStore, ByteSource, Digest, Error, Limits, MediaType, ObjectRange,
    ResponseSource, StagingAppend, StagingCommit, StagingKey, StagingOpen,
};

/// Constructs fresh or reopened sessions for one driver-owned backing store.
pub trait Driver {
    type Store: BlobStore;

    fn name(&self) -> &'static str;
    fn limits(&self) -> Limits;
    fn open(&self) -> Result<Self::Store, Error>;
    fn digest(&self, bytes: &[u8]) -> Digest;
}

#[derive(Debug)]
struct TestSource {
    bytes: Vec<u8>,
    cursor: usize,
    finishes: usize,
    fail_after: Option<usize>,
}

impl TestSource {
    fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: bytes.into(),
            cursor: 0,
            finishes: 0,
            fail_after: None,
        }
    }

    fn failing(bytes: impl Into<Vec<u8>>, fail_after: usize) -> Self {
        Self {
            fail_after: Some(fail_after),
            ..Self::new(bytes)
        }
    }
}

impl ByteSource for TestSource {
    fn read(&mut self, output: &mut [u8]) -> Result<usize, Error> {
        if self.finishes != 0 {
            return Err(Error::SourceClosed);
        }
        if self.fail_after.is_some_and(|limit| self.cursor >= limit) {
            return Err(Error::source(
                "blob-conformance-source-failure",
                "injected source failure",
            ));
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
        if self.finishes != 0 {
            return Err(Error::SourceClosed);
        }
        self.finishes += 1;
        Ok(())
    }
}

fn staging_key<D: Driver>(driver: &D, value: &str) -> StagingKey {
    StagingKey::new(value, driver.limits()).expect("conformance staging key must be valid")
}

fn media_type<D: Driver>(driver: &D) -> MediaType {
    MediaType::new("application/octet-stream", driver.limits())
        .expect("conformance media type must be valid")
}

fn open_request<D: Driver>(driver: &D, name: &str, bytes: &[u8]) -> StagingOpen {
    StagingOpen::new(
        staging_key(driver, name),
        driver.digest(bytes),
        bytes.len() as u64,
        media_type(driver),
        driver.limits(),
    )
    .expect("conformance open request must be valid")
}

fn open_request_with_digest<D: Driver>(
    driver: &D,
    name: &str,
    expected_digest: Digest,
    expected_size: u64,
) -> StagingOpen {
    StagingOpen::new(
        staging_key(driver, name),
        expected_digest,
        expected_size,
        media_type(driver),
        driver.limits(),
    )
    .expect("conformance open request must be valid")
}

fn append_request<D: Driver>(driver: &D, name: &str, offset: u64, length: usize) -> StagingAppend {
    StagingAppend::new(staging_key(driver, name), offset, length, driver.limits())
        .expect("conformance append request must be valid")
}

fn commit_request<D: Driver>(driver: &D, name: &str, bytes: &[u8]) -> StagingCommit {
    StagingCommit::new(
        staging_key(driver, name),
        driver.digest(bytes),
        bytes.len() as u64,
        driver.limits(),
    )
    .expect("conformance commit request must be valid")
}

fn commit_request_with<D: Driver>(
    driver: &D,
    name: &str,
    expected_digest: Digest,
    expected_size: u64,
) -> StagingCommit {
    StagingCommit::new(
        staging_key(driver, name),
        expected_digest,
        expected_size,
        driver.limits(),
    )
    .expect("conformance commit request must be valid")
}

fn append_exact<D: Driver>(
    driver: &D,
    store: &D::Store,
    name: &str,
    offset: u64,
    bytes: &[u8],
) -> AppendReceipt {
    let mut source = TestSource::new(bytes.to_vec());
    let receipt = store
        .staging_append_from_source(
            append_request(driver, name, offset, bytes.len()),
            &mut source,
        )
        .unwrap_or_else(|error| {
            panic!(
                "{} exact conformance append failed with {}",
                driver.name(),
                error.code()
            )
        });
    assert_eq!(source.finishes, 1, "source must finish exactly once");
    receipt
}

fn staging_offset<D: Driver>(driver: &D, store: &D::Store, name: &str, bytes: &[u8]) -> u64 {
    store
        .staging_open(open_request(driver, name, bytes))
        .unwrap_or_else(|error| {
            panic!(
                "{} conformance staging resume failed with {}",
                driver.name(),
                error.code()
            )
        })
        .offset
}

fn error_code<T>(result: Result<T, Error>) -> &'static str {
    match result {
        Ok(_) => panic!("conformance operation unexpectedly succeeded"),
        Err(error) => error.code(),
    }
}

fn read_all<S: ResponseSource>(mut source: S, chunk_size: usize) -> Vec<u8> {
    assert_ne!(chunk_size, 0);
    let declared = source.declared_length() as usize;
    let mut output = Vec::with_capacity(declared);
    let mut chunk = vec![0_u8; chunk_size];
    loop {
        let read = source.read(&mut chunk).expect("conformance source read");
        if read == 0 {
            break;
        }
        output.extend_from_slice(&chunk[..read]);
    }
    assert_eq!(
        output.len(),
        declared,
        "source length must match declaration"
    );
    source.close().expect("conformance source close");
    assert_eq!(
        error_code(source.read(&mut [0_u8; 1])),
        "blob-source-closed"
    );
    output
}

/// Covers both single-append and resumed multi-append installation, exact
/// commit replay and immutable ranged response sources.
pub fn staged_round_trip<D: Driver>(driver: &D) {
    let store = driver.open().expect("conformance driver must open");
    store
        .probe()
        .expect("conformance driver probe must succeed");

    let bytes = b"abcdefgh";
    let opened = store
        .staging_open(open_request(driver, "multi", bytes))
        .expect("multi staging must open");
    assert_eq!(opened.offset, 0);
    assert_eq!(staging_offset(driver, &store, "multi", bytes), 0);

    let first = append_exact(driver, &store, "multi", 0, &bytes[..3]);
    assert_eq!(first.offset, 3);
    assert_eq!(first.length, 3);
    assert_eq!(staging_offset(driver, &store, "multi", bytes), 3);

    let second = append_exact(driver, &store, "multi", 3, &bytes[3..]);
    assert_eq!(second.offset, bytes.len() as u64);
    assert_eq!(second.length, bytes.len() - 3);

    let commit = commit_request(driver, "multi", bytes);
    let first_commit = store
        .staging_verify_commit(commit.clone())
        .expect("multi staging must commit");
    let replay = store
        .staging_verify_commit(commit)
        .expect("exact commit replay must succeed without staging state");
    assert_eq!(replay, first_commit);
    assert_eq!(first_commit.digest, driver.digest(bytes));
    assert_eq!(first_commit.size, bytes.len() as u64);

    let source = store
        .object_open_source(ObjectRange::new(driver.digest(bytes), 2, 4).unwrap())
        .expect("installed range must open");
    assert_eq!(read_all(source, 2), b"cdef");

    let single = b"single";
    store
        .staging_open(open_request(driver, "single", single))
        .expect("single staging must open");
    let receipt = append_exact(driver, &store, "single", 0, single);
    assert_eq!(receipt.offset, single.len() as u64);
    let descriptor = store
        .staging_verify_commit(commit_request(driver, "single", single))
        .expect("single staging must commit");
    assert_eq!(descriptor.digest, driver.digest(single));
}

/// Ensures short, long, cancelled/failed and stale-offset sources never
/// advance staging and that accepted sources are finished exactly once.
pub fn source_failures_are_atomic<D: Driver>(driver: &D) {
    let store = driver.open().expect("conformance driver must open");
    let bytes = b"abcd";
    store
        .staging_open(open_request(driver, "source-errors", bytes))
        .expect("source-error staging must open");

    let mut short = TestSource::new(b"a".to_vec());
    assert_eq!(
        error_code(
            store.staging_append_from_source(
                append_request(driver, "source-errors", 0, 2),
                &mut short,
            )
        ),
        "blob-source-short"
    );
    assert_eq!(short.finishes, 1);
    assert_eq!(staging_offset(driver, &store, "source-errors", bytes), 0);

    let mut long = TestSource::new(b"abc".to_vec());
    assert_eq!(
        error_code(
            store.staging_append_from_source(
                append_request(driver, "source-errors", 0, 2),
                &mut long,
            )
        ),
        "blob-source-long"
    );
    assert_eq!(long.finishes, 1);
    assert_eq!(staging_offset(driver, &store, "source-errors", bytes), 0);

    let mut stale = TestSource::new(b"ab".to_vec());
    assert_eq!(
        error_code(
            store.staging_append_from_source(
                append_request(driver, "source-errors", 1, 2),
                &mut stale,
            )
        ),
        "blob-offset-mismatch"
    );
    assert_eq!(stale.finishes, 0, "rejected source must not be claimed");

    let mut failed = TestSource::failing(b"ab".to_vec(), 0);
    assert_eq!(
        error_code(store.staging_append_from_source(
            append_request(driver, "source-errors", 0, 2),
            &mut failed,
        )),
        "blob-conformance-source-failure"
    );
    assert_eq!(failed.finishes, 1);
    assert_eq!(staging_offset(driver, &store, "source-errors", bytes), 0);
}

/// Covers incomplete/size and digest failures, staging identity collisions and
/// idempotent abort without leaking partially accepted bytes into an object.
pub fn commit_failures_and_abort<D: Driver>(driver: &D) {
    let store = driver.open().expect("conformance driver must open");

    let bytes = b"abcd";
    store
        .staging_open(open_request(driver, "incomplete", bytes))
        .expect("incomplete staging must open");
    append_exact(driver, &store, "incomplete", 0, &bytes[..2]);
    assert_eq!(
        error_code(store.staging_verify_commit(commit_request(driver, "incomplete", bytes,))),
        "blob-staging-incomplete"
    );
    assert_eq!(
        error_code(store.staging_verify_commit(commit_request_with(
            driver,
            "incomplete",
            driver.digest(bytes),
            3,
        ))),
        "blob-staging-conflict"
    );

    let wrong_digest = driver.digest(b"wxyz");
    store
        .staging_open(open_request_with_digest(
            driver,
            "digest-mismatch",
            wrong_digest,
            bytes.len() as u64,
        ))
        .expect("digest-mismatch staging must open");
    append_exact(driver, &store, "digest-mismatch", 0, bytes);
    assert_eq!(
        error_code(store.staging_verify_commit(commit_request_with(
            driver,
            "digest-mismatch",
            wrong_digest,
            bytes.len() as u64,
        ))),
        "blob-digest-mismatch"
    );

    store
        .staging_open(open_request(driver, "collision", b"abc"))
        .expect("collision staging must open");
    assert_eq!(
        error_code(store.staging_open(open_request(driver, "collision", b"xyz"))),
        "blob-staging-conflict"
    );
    let key = staging_key(driver, "collision");
    store.staging_abort(&key).expect("first abort must succeed");
    store
        .staging_abort(&key)
        .expect("replayed abort must succeed");
    assert_eq!(staging_offset(driver, &store, "collision", b"abc"), 0);
}

/// Exercises object absence, range bounds and the configured staging capacity
/// through the portable store contract.
pub fn range_and_capacity_guards<D: Driver>(driver: &D) {
    let store = driver.open().expect("conformance driver must open");
    let missing = driver.digest(b"missing");
    assert_eq!(
        error_code(store.object_open_source(ObjectRange::new(missing, 0, 1).unwrap())),
        "blob-object-missing"
    );

    let bytes = b"range";
    store
        .staging_open(open_request(driver, "range", bytes))
        .expect("range staging must open");
    append_exact(driver, &store, "range", 0, bytes);
    store
        .staging_verify_commit(commit_request(driver, "range", bytes))
        .expect("range staging must commit");
    assert_eq!(
        error_code(store.object_open_source(
            ObjectRange::new(driver.digest(bytes), bytes.len() as u64 - 1, 2).unwrap(),
        )),
        "blob-range-invalid"
    );

    for index in 0..driver.limits().max_staging_entries {
        let name = format!("capacity-{index}");
        let content = [index as u8];
        store
            .staging_open(open_request(driver, &name, &content))
            .expect("staging capacity fixture must open");
    }
    assert_eq!(
        error_code(store.staging_open(open_request(driver, "capacity-overflow", b"x",))),
        "blob-staging-capacity"
    );
}

/// Opens independent driver sessions around a partially staged upload and an
/// installed object. Memory fixtures retain a shared in-process backing store;
/// filesystem fixtures additionally exercise their restart-safe trusted root.
pub fn reopen_preserves_staging_and_objects<D: Driver>(driver: &D) {
    let bytes = b"restartable";
    {
        let store = driver.open().expect("first conformance session must open");
        store
            .staging_open(open_request(driver, "restart", bytes))
            .expect("restart staging must open");
        append_exact(driver, &store, "restart", 0, &bytes[..4]);
    }

    {
        let store = driver.open().expect("second conformance session must open");
        assert_eq!(staging_offset(driver, &store, "restart", bytes), 4);
        append_exact(driver, &store, "restart", 4, &bytes[4..]);
        store
            .staging_verify_commit(commit_request(driver, "restart", bytes))
            .expect("reopened staging must commit");
    }

    let store = driver.open().expect("third conformance session must open");
    let source = store
        .object_open_source(ObjectRange::new(driver.digest(bytes), 0, bytes.len() as u64).unwrap())
        .expect("reopened object source must open");
    assert_eq!(read_all(source, 3), bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoplite_blob_store::{
        DigestVerifier, InMemoryBlobStore, MemoryResponseSource, StagingStatus,
    };
    use hoplite_blob_store_filesystem::FilesystemBlobStore;
    use sha2::{Digest as ShaDigest, Sha256};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    #[derive(Clone, Copy, Debug)]
    struct Sha256Verifier;

    impl DigestVerifier for Sha256Verifier {
        fn sha256(&self, bytes: &[u8]) -> [u8; 32] {
            let value = Sha256::digest(bytes);
            let mut output = [0_u8; 32];
            output.copy_from_slice(&value);
            output
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

    fn sha256(bytes: &[u8]) -> Digest {
        Digest::from_bytes(Sha256Verifier.sha256(bytes))
    }

    #[derive(Clone)]
    struct SharedMemoryStore(Arc<InMemoryBlobStore<Sha256Verifier>>);

    impl BlobStore for SharedMemoryStore {
        type Source = MemoryResponseSource;

        fn probe(&self) -> Result<(), Error> {
            self.0.probe()
        }

        fn staging_open(&self, request: StagingOpen) -> Result<StagingStatus, Error> {
            self.0.staging_open(request)
        }

        fn staging_append_from_source(
            &self,
            request: StagingAppend,
            source: &mut dyn ByteSource,
        ) -> Result<AppendReceipt, Error> {
            self.0.staging_append_from_source(request, source)
        }

        fn staging_abort(&self, staging_key: &StagingKey) -> Result<(), Error> {
            self.0.staging_abort(staging_key)
        }

        fn staging_verify_commit(
            &self,
            request: StagingCommit,
        ) -> Result<hoplite_blob_store::ObjectDescriptor, Error> {
            self.0.staging_verify_commit(request)
        }

        fn object_open_source(&self, request: ObjectRange) -> Result<Self::Source, Error> {
            self.0.object_open_source(request)
        }
    }

    struct MemoryDriver {
        store: Arc<InMemoryBlobStore<Sha256Verifier>>,
    }

    impl MemoryDriver {
        fn new() -> Self {
            Self {
                store: Arc::new(InMemoryBlobStore::new(Sha256Verifier, limits()).unwrap()),
            }
        }
    }

    impl Driver for MemoryDriver {
        type Store = SharedMemoryStore;

        fn name(&self) -> &'static str {
            "memory"
        }

        fn limits(&self) -> Limits {
            limits()
        }

        fn open(&self) -> Result<Self::Store, Error> {
            Ok(SharedMemoryStore(self.store.clone()))
        }

        fn digest(&self, bytes: &[u8]) -> Digest {
            sha256(bytes)
        }
    }

    static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new(label: &str) -> Self {
            let id = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "hoplite-blob-conformance-{label}-{}-{id}",
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

    struct FilesystemDriver {
        root: TestRoot,
    }

    impl FilesystemDriver {
        fn new(label: &str) -> Self {
            Self {
                root: TestRoot::new(label),
            }
        }
    }

    impl Driver for FilesystemDriver {
        type Store = FilesystemBlobStore;

        fn name(&self) -> &'static str {
            "filesystem"
        }

        fn limits(&self) -> Limits {
            limits()
        }

        fn open(&self) -> Result<Self::Store, Error> {
            FilesystemBlobStore::open(self.root.path(), limits())
        }

        fn digest(&self, bytes: &[u8]) -> Digest {
            sha256(bytes)
        }
    }

    #[test]
    fn memory_staged_round_trip() {
        staged_round_trip(&MemoryDriver::new());
    }

    #[test]
    fn filesystem_staged_round_trip() {
        staged_round_trip(&FilesystemDriver::new("round-trip"));
    }

    #[test]
    fn memory_source_failures_are_atomic() {
        source_failures_are_atomic(&MemoryDriver::new());
    }

    #[test]
    fn filesystem_source_failures_are_atomic() {
        source_failures_are_atomic(&FilesystemDriver::new("source-failures"));
    }

    #[test]
    fn memory_commit_failures_and_abort() {
        commit_failures_and_abort(&MemoryDriver::new());
    }

    #[test]
    fn filesystem_commit_failures_and_abort() {
        commit_failures_and_abort(&FilesystemDriver::new("commit-failures"));
    }

    #[test]
    fn memory_range_and_capacity_guards() {
        range_and_capacity_guards(&MemoryDriver::new());
    }

    #[test]
    fn filesystem_range_and_capacity_guards() {
        range_and_capacity_guards(&FilesystemDriver::new("range-capacity"));
    }

    #[test]
    fn memory_reopen_preserves_staging_and_objects() {
        reopen_preserves_staging_and_objects(&MemoryDriver::new());
    }

    #[test]
    fn filesystem_reopen_preserves_staging_and_objects() {
        reopen_preserves_staging_and_objects(&FilesystemDriver::new("reopen"));
    }
}
