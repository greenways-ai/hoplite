use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    fn new(label: &str) -> Self {
        let id = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "hoplite-blob-filesystem-reader-{label}-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("objects/sha256")).unwrap();
        File::create(path.join(LOCK_FILE)).unwrap();
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

fn limits() -> Limits {
    Limits {
        max_object_bytes: 4096,
        max_media_type_bytes: 128,
        io_chunk_bytes: 11,
    }
}

fn digest(bytes: &[u8]) -> Digest {
    let value = Sha256::digest(bytes);
    let mut output = [0_u8; 32];
    output.copy_from_slice(&value);
    Digest::from_bytes(output)
}

fn object_paths(root: &TestRoot, object_digest: Digest) -> (PathBuf, PathBuf) {
    let value = object_digest.to_string();
    let hex = value.strip_prefix("sha256:").unwrap();
    let directory = root.path().join("objects/sha256").join(&hex[..2]);
    (
        directory.join(format!("{}.meta", &hex[2..])),
        directory.join(format!("{}.blob", &hex[2..])),
    )
}

fn metadata(object_digest: Digest, size: u64, media_type: &str) -> Vec<u8> {
    let mut output = OBJECT_MAGIC.to_vec();
    output.extend_from_slice(object_digest.bytes());
    output.extend_from_slice(&size.to_be_bytes());
    output.extend_from_slice(&(media_type.len() as u32).to_be_bytes());
    output.extend_from_slice(media_type.as_bytes());
    output
}

fn install(root: &TestRoot, bytes: &[u8]) -> Digest {
    let object_digest = digest(bytes);
    let (metadata_path, data_path) = object_paths(root, object_digest);
    fs::create_dir_all(metadata_path.parent().unwrap()).unwrap();
    fs::write(data_path, bytes).unwrap();
    fs::write(
        metadata_path,
        metadata(
            object_digest,
            bytes.len() as u64,
            "application/vnd.hara.hta",
        ),
    )
    .unwrap();
    object_digest
}

fn read_all(source: &mut FilesystemResponseSource) -> Result<Vec<u8>, BlobError> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 3];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            return Ok(output);
        }
        output.extend_from_slice(&buffer[..read]);
    }
}

#[test]
fn reads_exact_verified_bytes_and_reopens() {
    let root = TestRoot::new("read-restart");
    let bytes = b"HTA1fixture";
    let object_digest = install(&root, bytes);

    let reader = FilesystemObjectReader::open(root.path(), limits()).unwrap();
    let object = reader.read_verified(object_digest, bytes.len()).unwrap();
    assert_eq!(object.digest(), object_digest);
    assert_eq!(object.byte_length(), bytes.len());
    assert_eq!(object.bytes(), bytes);
    drop(reader);

    let reopened = FilesystemObjectReader::open(root.path(), limits()).unwrap();
    assert_eq!(
        reopened
            .read_verified(object_digest, bytes.len())
            .unwrap()
            .into_bytes(),
        bytes
    );
}

#[test]
fn opens_verified_ranges_without_materializing_the_full_response() {
    let root = TestRoot::new("source");
    let bytes = b"0123456789abcdef";
    let object_digest = install(&root, bytes);
    let reader = FilesystemObjectReader::open(root.path(), limits()).unwrap();

    let summary = reader.inspect_verified(object_digest).unwrap();
    assert_eq!(summary.digest(), object_digest);
    assert_eq!(summary.size(), bytes.len() as u64);

    let mut source = reader
        .open_source(ObjectRange::new(object_digest, 4, 7).unwrap())
        .unwrap();
    assert_eq!(source.declared_length(), 7);
    assert_eq!(read_all(&mut source).unwrap(), b"456789a");
    source.close().unwrap();
    assert!(matches!(source.close(), Err(BlobError::SourceClosed)));
}

#[test]
fn rejects_missing_incomplete_and_excess_objects() {
    let root = TestRoot::new("bounded");
    let bytes = b"bounded-object";
    let object_digest = install(&root, bytes);
    let reader = FilesystemObjectReader::open(root.path(), limits()).unwrap();

    assert_eq!(
        reader.read_verified(digest(b"missing"), 64),
        Err(Failure::Missing)
    );
    assert_eq!(
        reader.read_verified(object_digest, bytes.len() - 1),
        Err(Failure::Maximum)
    );
    assert_eq!(
        reader.read_verified(object_digest, limits().max_object_bytes + 1),
        Err(Failure::Maximum)
    );
    assert!(matches!(
        reader.open_source(ObjectRange::new(object_digest, 0, bytes.len() as u64 + 1).unwrap()),
        Err(BlobError::InvalidRange { .. })
    ));

    let (metadata_path, _) = object_paths(&root, object_digest);
    fs::remove_file(metadata_path).unwrap();
    assert_eq!(
        reader.read_verified(object_digest, bytes.len()),
        Err(Failure::Provider)
    );
}

#[test]
fn rejects_tampered_bytes_and_metadata() {
    let root = TestRoot::new("integrity");
    let bytes = b"verified-object";
    let object_digest = install(&root, bytes);
    let reader = FilesystemObjectReader::open(root.path(), limits()).unwrap();
    let (metadata_path, data_path) = object_paths(&root, object_digest);

    fs::write(&data_path, b"tampered-object").unwrap();
    assert_eq!(
        reader.read_verified(object_digest, bytes.len()),
        Err(Failure::Digest)
    );
    assert!(matches!(
        reader.inspect_verified(object_digest),
        Err(BlobError::DigestMismatch { .. })
    ));

    fs::write(&data_path, bytes).unwrap();
    fs::write(
        metadata_path,
        metadata(
            object_digest,
            (bytes.len() + 1) as u64,
            "application/vnd.hara.hta",
        ),
    )
    .unwrap();
    assert_eq!(
        reader.read_verified(object_digest, bytes.len() + 1),
        Err(Failure::Provider)
    );
}

#[test]
fn fails_closed_on_malformed_installation_paths() {
    let root = TestRoot::new("paths");
    fs::remove_file(root.path().join(LOCK_FILE)).unwrap();
    assert!(FilesystemObjectReader::open(root.path(), limits()).is_err());

    let invalid = Limits {
        max_object_bytes: 0,
        ..limits()
    };
    assert!(matches!(
        FilesystemObjectReader::open(root.path(), invalid),
        Err(Error::InvalidLimits(_))
    ));
}
