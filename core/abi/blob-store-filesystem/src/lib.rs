#![forbid(unsafe_code)]

//! Restart-safe trusted-root filesystem implementation of Hoplite's generic
//! `hoplite.blob` mechanics.
//!
//! The driver owns staging, immutable installation, fsync and restart
//! recovery. Verified immutable reads and response ranges are delegated to
//! the shared filesystem object reader. It does not understand
//! Tahto uploads, applications, namespaces, quotas, graphs, manifests,
//! authorization, receipts or merge policy.

use fs2::FileExt;
use hoplite_blob_filesystem_reader::{
    FilesystemObjectReader, FilesystemResponseSource, Limits as ReaderLimits,
};
use hoplite_blob_store::{
    AppendReceipt, BlobStore, ByteSource, Digest, Error, Limits, MediaType, ObjectDescriptor,
    ObjectRange, StagingAppend, StagingCommit, StagingKey, StagingOpen, StagingStatus,
};
use sha2::{Digest as Sha2Digest, Sha256};
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

const STAGING_MAGIC: &[u8; 4] = b"HBS0";
const OBJECT_MAGIC: &[u8; 4] = b"HBO0";
const TEMP_PREFIX: &str = ".hoplite-tmp-";
const LOCK_FILE: &str = "store.lock";
const IO_CHUNK_BYTES: usize = 64 * 1024;

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

pub struct FilesystemBlobStore {
    root: PathBuf,
    staging_dir: PathBuf,
    objects_dir: PathBuf,
    lock_path: PathBuf,
    limits: Limits,
    reader: FilesystemObjectReader,
    process_lock: Mutex<()>,
}

impl fmt::Debug for FilesystemBlobStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FilesystemBlobStore")
            .field("root", &self.root)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl FilesystemBlobStore {
    pub fn open(root: impl AsRef<Path>, limits: Limits) -> Result<Self, Error> {
        let limits = limits.validate()?;
        let root = prepare_root(root.as_ref())?;
        let staging_dir = root.join("staging");
        let objects_root = root.join("objects");
        let objects_dir = objects_root.join("sha256");
        ensure_directory(&staging_dir)?;
        ensure_directory(&objects_root)?;
        ensure_directory(&objects_dir)?;

        let lock_path = root.join(LOCK_FILE);
        ensure_regular_or_missing(&lock_path, "blob-filesystem-lock-invalid")?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| io_error("blob-filesystem-lock-open", error))?;
        lock.sync_all()
            .map_err(|error| io_error("blob-filesystem-lock-sync", error))?;
        drop(lock);
        sync_directory(&root)?;

        let reader_max_object_bytes = usize::try_from(limits.max_object_bytes)
            .map_err(|_| Error::InvalidLimits("max_object_bytes exceeds the host usize range"))?;
        let reader = FilesystemObjectReader::open(
            &root,
            ReaderLimits {
                max_object_bytes: reader_max_object_bytes,
                max_media_type_bytes: limits.max_media_type_bytes,
                io_chunk_bytes: IO_CHUNK_BYTES.min(reader_max_object_bytes),
            },
        )
        .map_err(|error| Error::driver(error.code(), error.to_string()))?;

        let store = Self {
            root,
            staging_dir,
            objects_dir,
            lock_path,
            limits,
            reader,
            process_lock: Mutex::new(()),
        };
        {
            let _guard = store.exclusive()?;
            store.cleanup_temporary_files_locked()?;
        }
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub const fn limits(&self) -> Limits {
        self.limits
    }

    pub fn verify(&self) -> Result<(), Error> {
        let _guard = self.exclusive()?;
        ensure_directory(&self.staging_dir)?;
        ensure_directory(&self.objects_dir)?;
        ensure_regular_or_missing(&self.lock_path, "blob-filesystem-lock-invalid")?;

        for entry in read_directory(&self.staging_dir)? {
            let path = entry.path();
            if is_temporary(&path) {
                continue;
            }
            if path.extension() == Some(OsStr::new("meta")) {
                let metadata = self.read_staging_metadata(&path)?;
                let (expected_meta, _) = self.staging_paths(&metadata.staging_key);
                if expected_meta != path {
                    return Err(corrupt(
                        "blob-filesystem-staging-identity",
                        "staging metadata is stored under the wrong physical identity",
                    ));
                }
                self.load_staging_locked(&metadata.staging_key)?
                    .ok_or_else(|| {
                        corrupt(
                            "blob-filesystem-staging-missing",
                            "staging metadata disappeared during verification",
                        )
                    })?;
            }
        }

        for prefix in read_directory(&self.objects_dir)? {
            let prefix_path = prefix.path();
            ensure_directory(&prefix_path)?;
            for entry in read_directory(&prefix_path)? {
                let path = entry.path();
                if is_temporary(&path) || path.extension() != Some(OsStr::new("meta")) {
                    continue;
                }
                let metadata = self.read_object_metadata(&path)?;
                match self.inspect_object_locked(metadata.digest)? {
                    ObjectPresence::Complete(descriptor) if descriptor == metadata.descriptor() => {
                    }
                    ObjectPresence::Complete(_) => {
                        return Err(corrupt(
                            "blob-filesystem-object-metadata",
                            "object metadata does not match its verified descriptor",
                        ));
                    }
                    ObjectPresence::Missing | ObjectPresence::DataOnly => {
                        return Err(corrupt(
                            "blob-filesystem-object-incomplete",
                            "authoritative object metadata lacks a complete object",
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn exclusive(&self) -> Result<OperationGuard<'_>, Error> {
        let process = self.process_lock.lock().map_err(|_| Error::Poisoned)?;
        ensure_regular_or_missing(&self.lock_path, "blob-filesystem-lock-invalid")?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.lock_path)
            .map_err(|error| io_error("blob-filesystem-lock-open", error))?;
        FileExt::lock_exclusive(&file)
            .map_err(|error| io_error("blob-filesystem-lock-acquire", error))?;
        Ok(OperationGuard {
            _process: process,
            file,
        })
    }

    fn cleanup_temporary_files_locked(&self) -> Result<(), Error> {
        cleanup_temporary_files(&self.staging_dir)?;
        for entry in read_directory(&self.objects_dir)? {
            let path = entry.path();
            ensure_directory(&path)?;
            cleanup_temporary_files(&path)?;
        }
        Ok(())
    }

    fn staging_paths(&self, key: &StagingKey) -> (PathBuf, PathBuf) {
        let id = staging_id(key);
        (
            self.staging_dir.join(format!("{id}.meta")),
            self.staging_dir.join(format!("{id}.data")),
        )
    }

    fn object_paths(&self, digest: Digest) -> (PathBuf, PathBuf) {
        let value = digest.to_string();
        let hex = value
            .strip_prefix("sha256:")
            .expect("Digest display always uses sha256 prefix");
        let directory = self.objects_dir.join(&hex[..2]);
        let stem = &hex[2..];
        (
            directory.join(format!("{stem}.meta")),
            directory.join(format!("{stem}.blob")),
        )
    }

    fn read_staging_metadata(&self, path: &Path) -> Result<StagingMetadata, Error> {
        let limit = 4_u64
            + 4
            + self.limits.max_staging_key_bytes as u64
            + 32
            + 8
            + 8
            + 4
            + self.limits.max_media_type_bytes as u64;
        let bytes = read_small_file(path, limit, "blob-filesystem-staging-metadata-read")?;
        StagingMetadata::decode(&bytes, self.limits)
    }

    fn read_object_metadata(&self, path: &Path) -> Result<ObjectMetadata, Error> {
        let limit = 4_u64 + 32 + 8 + 4 + self.limits.max_media_type_bytes as u64;
        let bytes = read_small_file(path, limit, "blob-filesystem-object-metadata-read")?;
        ObjectMetadata::decode(&bytes, self.limits)
    }

    fn load_staging_locked(&self, key: &StagingKey) -> Result<Option<StagingMetadata>, Error> {
        let (metadata_path, data_path) = self.staging_paths(key);
        let metadata_exists =
            regular_file_exists(&metadata_path, "blob-filesystem-staging-metadata-invalid")?;
        let data_exists = regular_file_exists(&data_path, "blob-filesystem-staging-data-invalid")?;

        if !metadata_exists {
            if data_exists {
                fs::remove_file(&data_path)
                    .map_err(|error| io_error("blob-filesystem-staging-orphan-remove", error))?;
                sync_directory(&self.staging_dir)?;
            }
            return Ok(None);
        }
        if !data_exists {
            return Err(corrupt(
                "blob-filesystem-staging-data-missing",
                "staging metadata exists without its data file",
            ));
        }

        let metadata = self.read_staging_metadata(&metadata_path)?;
        if &metadata.staging_key != key {
            return Err(corrupt(
                "blob-filesystem-staging-key-mismatch",
                "staging metadata does not match its logical key",
            ));
        }
        let actual = file_length(&data_path, "blob-filesystem-staging-stat")?;
        if actual < metadata.offset {
            return Err(corrupt(
                "blob-filesystem-staging-short",
                "staging bytes are shorter than the committed offset",
            ));
        }
        if actual > metadata.offset {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(data_path)
                .map_err(|error| io_error("blob-filesystem-staging-recover-open", error))?;
            file.set_len(metadata.offset)
                .map_err(|error| io_error("blob-filesystem-staging-recover-truncate", error))?;
            file.sync_all()
                .map_err(|error| io_error("blob-filesystem-staging-recover-sync", error))?;
            sync_directory(&self.staging_dir)?;
        }
        Ok(Some(metadata))
    }

    fn remove_staging_locked(&self, key: &StagingKey) -> Result<(), Error> {
        let (metadata_path, data_path) = self.staging_paths(key);
        remove_regular_if_exists(&metadata_path, "blob-filesystem-staging-metadata-remove")?;
        remove_regular_if_exists(&data_path, "blob-filesystem-staging-data-remove")?;
        sync_directory(&self.staging_dir)
    }

    fn inspect_object_locked(&self, digest: Digest) -> Result<ObjectPresence, Error> {
        let (metadata_path, data_path) = self.object_paths(digest);
        let metadata_exists =
            regular_file_exists(&metadata_path, "blob-filesystem-object-metadata-invalid")?;
        let data_exists = regular_file_exists(&data_path, "blob-filesystem-object-data-invalid")?;
        match (metadata_exists, data_exists) {
            (false, false) => Ok(ObjectPresence::Missing),
            (false, true) => Ok(ObjectPresence::DataOnly),
            (true, false) => Err(corrupt(
                "blob-filesystem-object-data-missing",
                "object metadata exists without object bytes",
            )),
            (true, true) => {
                let metadata = self.read_object_metadata(&metadata_path)?;
                if metadata.digest != digest {
                    return Err(corrupt(
                        "blob-filesystem-object-digest-mismatch",
                        "object metadata is stored under the wrong digest path",
                    ));
                }
                let (actual_digest, actual_size) = hash_file(&data_path, self.limits)?;
                if actual_digest != digest {
                    return Err(Error::DigestMismatch {
                        expected: digest,
                        actual: actual_digest,
                    });
                }
                if actual_size != metadata.size {
                    return Err(corrupt(
                        "blob-filesystem-object-size-mismatch",
                        "object metadata size does not match stored bytes",
                    ));
                }
                Ok(ObjectPresence::Complete(metadata.descriptor()))
            }
        }
    }

    fn install_object_locked(
        &self,
        staging: &StagingMetadata,
        staging_data: &Path,
    ) -> Result<ObjectDescriptor, Error> {
        let descriptor = ObjectDescriptor {
            digest: staging.expected_digest,
            size: staging.expected_size,
            media_type: staging.media_type.clone(),
        };
        let (metadata_path, data_path) = self.object_paths(descriptor.digest);
        let object_directory = data_path
            .parent()
            .ok_or_else(|| corrupt("blob-filesystem-object-path", "object path has no parent"))?;
        ensure_directory(object_directory)?;

        match self.inspect_object_locked(descriptor.digest)? {
            ObjectPresence::Complete(existing) => {
                if existing.size != descriptor.size {
                    return Err(Error::ObjectConflict {
                        digest: descriptor.digest,
                    });
                }
                return Ok(existing);
            }
            ObjectPresence::Missing => {
                if count_object_metadata(&self.objects_dir)? >= self.limits.max_objects {
                    return Err(Error::ObjectCapacity {
                        limit: self.limits.max_objects,
                    });
                }
                match fs::hard_link(staging_data, &data_path) {
                    Ok(()) => sync_directory(object_directory)?,
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(io_error("blob-filesystem-object-link", error));
                    }
                }
            }
            ObjectPresence::DataOnly => {
                if count_object_metadata(&self.objects_dir)? >= self.limits.max_objects {
                    return Err(Error::ObjectCapacity {
                        limit: self.limits.max_objects,
                    });
                }
            }
        }

        let (actual_digest, actual_size) = hash_file(&data_path, self.limits)?;
        if actual_digest != descriptor.digest {
            return Err(Error::DigestMismatch {
                expected: descriptor.digest,
                actual: actual_digest,
            });
        }
        if actual_size != descriptor.size {
            return Err(Error::ObjectConflict {
                digest: descriptor.digest,
            });
        }

        let object_metadata = ObjectMetadata::from_descriptor(&descriptor);
        let encoded = object_metadata.encode()?;
        if !atomic_create(&metadata_path, &encoded)? {
            let existing = self.read_object_metadata(&metadata_path)?;
            if existing != object_metadata {
                return Err(Error::ObjectConflict {
                    digest: descriptor.digest,
                });
            }
        }
        sync_directory(object_directory)?;
        Ok(descriptor)
    }

    fn consume_source(&self, source: &mut dyn ByteSource, length: usize) -> Result<Vec<u8>, Error> {
        let result = self.read_exact_source(source, length);
        let finish = source.finish();
        match (result, finish) {
            (Ok(bytes), Ok(())) => Ok(bytes),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    fn read_exact_source(
        &self,
        source: &mut dyn ByteSource,
        length: usize,
    ) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::with_capacity(length);
        while bytes.len() < length {
            let remaining = length - bytes.len();
            let capacity = remaining.min(self.limits.max_source_chunk_bytes);
            let mut chunk = vec![0_u8; capacity];
            let read = source.read(&mut chunk)?;
            if read > chunk.len() {
                return Err(Error::SourceProtocol {
                    detail: "source returned more bytes than requested",
                });
            }
            if read == 0 {
                return Err(Error::SourceShort {
                    expected: length,
                    actual: bytes.len(),
                });
            }
            bytes.extend_from_slice(&chunk[..read]);
        }

        let mut extra = [0_u8; 1];
        let read = source.read(&mut extra)?;
        if read > extra.len() {
            return Err(Error::SourceProtocol {
                detail: "source returned more bytes than requested",
            });
        }
        if read != 0 {
            return Err(Error::SourceLong { expected: length });
        }
        Ok(bytes)
    }
}

impl BlobStore for FilesystemBlobStore {
    type Source = FilesystemResponseSource;

    fn staging_open(&self, request: StagingOpen) -> Result<StagingStatus, Error> {
        let request = StagingOpen::new(
            request.staging_key,
            request.expected_digest,
            request.expected_size,
            request.media_type,
            self.limits,
        )?;
        let _guard = self.exclusive()?;
        if let Some(current) = self.load_staging_locked(&request.staging_key)? {
            if !current.compatible(&request) {
                return Err(Error::StagingConflict {
                    staging_key: request.staging_key,
                });
            }
            return Ok(StagingStatus {
                staging_key: request.staging_key,
                offset: current.offset,
            });
        }
        if count_extension(&self.staging_dir, "meta")? >= self.limits.max_staging_entries {
            return Err(Error::StagingCapacity {
                limit: self.limits.max_staging_entries,
            });
        }

        let metadata = StagingMetadata::from_open(&request);
        let (metadata_path, data_path) = self.staging_paths(&request.staging_key);
        let data = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&data_path)
            .map_err(|error| io_error("blob-filesystem-staging-create", error))?;
        data.sync_all()
            .map_err(|error| io_error("blob-filesystem-staging-create-sync", error))?;
        sync_directory(&self.staging_dir)?;
        let encoded = metadata.encode()?;
        match atomic_create(&metadata_path, &encoded) {
            Ok(true) => Ok(StagingStatus {
                staging_key: request.staging_key,
                offset: 0,
            }),
            Ok(false) => {
                let _ = fs::remove_file(&data_path);
                Err(Error::StagingConflict {
                    staging_key: request.staging_key,
                })
            }
            Err(error) => {
                let _ = fs::remove_file(&data_path);
                Err(error)
            }
        }
    }

    fn staging_append_from_source(
        &self,
        request: StagingAppend,
        source: &mut dyn ByteSource,
    ) -> Result<AppendReceipt, Error> {
        let request = StagingAppend::new(
            request.staging_key,
            request.offset,
            request.length,
            self.limits,
        )?;
        let _guard = self.exclusive()?;
        let mut metadata = self
            .load_staging_locked(&request.staging_key)?
            .ok_or_else(|| Error::StagingMissing {
                staging_key: request.staging_key.clone(),
            })?;
        if metadata.offset != request.offset {
            return Err(Error::OffsetMismatch {
                expected: metadata.offset,
                actual: request.offset,
            });
        }
        let next = request.offset.checked_add(request.length as u64).ok_or(
            Error::ObjectLimitExceeded {
                limit: metadata.expected_size,
                actual: u64::MAX,
            },
        )?;
        if next > metadata.expected_size {
            return Err(Error::ObjectLimitExceeded {
                limit: metadata.expected_size,
                actual: next,
            });
        }

        let bytes = self.consume_source(source, request.length)?;
        let (metadata_path, data_path) = self.staging_paths(&request.staging_key);
        let mut data = OpenOptions::new()
            .read(true)
            .write(true)
            .open(data_path)
            .map_err(|error| io_error("blob-filesystem-staging-append-open", error))?;
        let actual = data
            .seek(SeekFrom::End(0))
            .map_err(|error| io_error("blob-filesystem-staging-append-seek", error))?;
        if actual != request.offset {
            return Err(Error::OffsetMismatch {
                expected: actual,
                actual: request.offset,
            });
        }
        data.write_all(&bytes)
            .map_err(|error| io_error("blob-filesystem-staging-append-write", error))?;
        data.sync_all()
            .map_err(|error| io_error("blob-filesystem-staging-append-sync", error))?;

        metadata.offset = next;
        atomic_replace(&metadata_path, &metadata.encode()?)?;
        Ok(AppendReceipt {
            staging_key: request.staging_key,
            offset: next,
            length: request.length,
        })
    }

    fn staging_abort(&self, staging_key: &StagingKey) -> Result<(), Error> {
        let key = StagingKey::new(staging_key.as_str().to_owned(), self.limits)?;
        let _guard = self.exclusive()?;
        self.remove_staging_locked(&key)
    }

    fn staging_verify_commit(&self, request: StagingCommit) -> Result<ObjectDescriptor, Error> {
        let request = StagingCommit::new(
            request.staging_key,
            request.expected_digest,
            request.expected_size,
            self.limits,
        )?;
        let _guard = self.exclusive()?;

        if let ObjectPresence::Complete(existing) =
            self.inspect_object_locked(request.expected_digest)?
        {
            if existing.size != request.expected_size {
                return Err(Error::ObjectConflict {
                    digest: request.expected_digest,
                });
            }
            self.remove_staging_locked(&request.staging_key)?;
            return Ok(existing);
        }

        let staging = self
            .load_staging_locked(&request.staging_key)?
            .ok_or_else(|| Error::StagingMissing {
                staging_key: request.staging_key.clone(),
            })?;
        if staging.expected_digest != request.expected_digest
            || staging.expected_size != request.expected_size
        {
            return Err(Error::StagingConflict {
                staging_key: request.staging_key,
            });
        }
        if staging.offset != request.expected_size {
            return Err(Error::IncompleteStaging {
                expected: request.expected_size,
                actual: staging.offset,
            });
        }
        let (_, staging_data) = self.staging_paths(&staging.staging_key);
        let (actual_digest, actual_size) = hash_file(&staging_data, self.limits)?;
        if actual_size != request.expected_size {
            return Err(Error::IncompleteStaging {
                expected: request.expected_size,
                actual: actual_size,
            });
        }
        if actual_digest != request.expected_digest {
            return Err(Error::DigestMismatch {
                expected: request.expected_digest,
                actual: actual_digest,
            });
        }
        let data = OpenOptions::new()
            .read(true)
            .open(&staging_data)
            .map_err(|error| io_error("blob-filesystem-staging-commit-open", error))?;
        data.sync_all()
            .map_err(|error| io_error("blob-filesystem-staging-commit-sync", error))?;

        let descriptor = self.install_object_locked(&staging, &staging_data)?;
        self.remove_staging_locked(&staging.staging_key)?;
        Ok(descriptor)
    }

    fn object_open_source(&self, request: ObjectRange) -> Result<Self::Source, Error> {
        self.reader.open_source(request)
    }
}

struct OperationGuard<'a> {
    _process: MutexGuard<'a, ()>,
    file: File,
}

impl Drop for OperationGuard<'_> {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StagingMetadata {
    staging_key: StagingKey,
    expected_digest: Digest,
    expected_size: u64,
    offset: u64,
    media_type: MediaType,
}

impl StagingMetadata {
    fn from_open(request: &StagingOpen) -> Self {
        Self {
            staging_key: request.staging_key.clone(),
            expected_digest: request.expected_digest,
            expected_size: request.expected_size,
            offset: 0,
            media_type: request.media_type.clone(),
        }
    }

    fn compatible(&self, request: &StagingOpen) -> bool {
        self.expected_digest == request.expected_digest
            && self.expected_size == request.expected_size
            && self.media_type == request.media_type
    }

    fn encode(&self) -> Result<Vec<u8>, Error> {
        let key = self.staging_key.as_str().as_bytes();
        let media_type = self.media_type.as_str().as_bytes();
        let key_length = u32::try_from(key.len()).map_err(|_| {
            corrupt(
                "blob-filesystem-staging-metadata-size",
                "staging key length does not fit metadata",
            )
        })?;
        let media_length = u32::try_from(media_type.len()).map_err(|_| {
            corrupt(
                "blob-filesystem-staging-metadata-size",
                "media type length does not fit metadata",
            )
        })?;
        let mut output = Vec::with_capacity(64 + key.len() + media_type.len());
        output.extend_from_slice(STAGING_MAGIC);
        output.extend_from_slice(&key_length.to_be_bytes());
        output.extend_from_slice(key);
        output.extend_from_slice(self.expected_digest.bytes());
        output.extend_from_slice(&self.expected_size.to_be_bytes());
        output.extend_from_slice(&self.offset.to_be_bytes());
        output.extend_from_slice(&media_length.to_be_bytes());
        output.extend_from_slice(media_type);
        Ok(output)
    }

    fn decode(bytes: &[u8], limits: Limits) -> Result<Self, Error> {
        let mut reader = MetadataReader::new(bytes);
        reader.expect_magic(STAGING_MAGIC)?;
        let key = reader.text(limits.max_staging_key_bytes)?;
        let expected_digest = Digest::from_bytes(reader.array_32()?);
        let expected_size = reader.u64()?;
        let offset = reader.u64()?;
        let media_type = reader.text(limits.max_media_type_bytes)?;
        reader.finish()?;
        let staging_key = StagingKey::new(key, limits).map_err(|_| {
            corrupt(
                "blob-filesystem-staging-metadata-key",
                "staging metadata contains an invalid logical key",
            )
        })?;
        let media_type = MediaType::new(media_type, limits).map_err(|_| {
            corrupt(
                "blob-filesystem-staging-metadata-media-type",
                "staging metadata contains an invalid media type",
            )
        })?;
        if expected_size > limits.max_object_bytes || offset > expected_size {
            return Err(corrupt(
                "blob-filesystem-staging-metadata-bounds",
                "staging metadata exceeds configured bounds",
            ));
        }
        Ok(Self {
            staging_key,
            expected_digest,
            expected_size,
            offset,
            media_type,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ObjectMetadata {
    digest: Digest,
    size: u64,
    media_type: MediaType,
}

impl ObjectMetadata {
    fn from_descriptor(descriptor: &ObjectDescriptor) -> Self {
        Self {
            digest: descriptor.digest,
            size: descriptor.size,
            media_type: descriptor.media_type.clone(),
        }
    }

    fn descriptor(&self) -> ObjectDescriptor {
        ObjectDescriptor {
            digest: self.digest,
            size: self.size,
            media_type: self.media_type.clone(),
        }
    }

    fn encode(&self) -> Result<Vec<u8>, Error> {
        let media_type = self.media_type.as_str().as_bytes();
        let media_length = u32::try_from(media_type.len()).map_err(|_| {
            corrupt(
                "blob-filesystem-object-metadata-size",
                "media type length does not fit metadata",
            )
        })?;
        let mut output = Vec::with_capacity(48 + media_type.len());
        output.extend_from_slice(OBJECT_MAGIC);
        output.extend_from_slice(self.digest.bytes());
        output.extend_from_slice(&self.size.to_be_bytes());
        output.extend_from_slice(&media_length.to_be_bytes());
        output.extend_from_slice(media_type);
        Ok(output)
    }

    fn decode(bytes: &[u8], limits: Limits) -> Result<Self, Error> {
        let mut reader = MetadataReader::new(bytes);
        reader.expect_magic(OBJECT_MAGIC)?;
        let digest = Digest::from_bytes(reader.array_32()?);
        let size = reader.u64()?;
        let media_type = reader.text(limits.max_media_type_bytes)?;
        reader.finish()?;
        let media_type = MediaType::new(media_type, limits).map_err(|_| {
            corrupt(
                "blob-filesystem-object-metadata-media-type",
                "object metadata contains an invalid media type",
            )
        })?;
        if size > limits.max_object_bytes {
            return Err(corrupt(
                "blob-filesystem-object-metadata-bounds",
                "object metadata exceeds the configured object limit",
            ));
        }
        Ok(Self {
            digest,
            size,
            media_type,
        })
    }
}

enum ObjectPresence {
    Missing,
    DataOnly,
    Complete(ObjectDescriptor),
}

struct MetadataReader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> MetadataReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], Error> {
        let end = self.cursor.checked_add(length).ok_or_else(|| {
            corrupt(
                "blob-filesystem-metadata-overflow",
                "metadata length overflow",
            )
        })?;
        let value = self.bytes.get(self.cursor..end).ok_or_else(|| {
            corrupt(
                "blob-filesystem-metadata-truncated",
                "metadata is truncated",
            )
        })?;
        self.cursor = end;
        Ok(value)
    }

    fn expect_magic(&mut self, expected: &[u8; 4]) -> Result<(), Error> {
        if self.take(4)? == expected {
            Ok(())
        } else {
            Err(corrupt(
                "blob-filesystem-metadata-magic",
                "metadata has an unsupported format",
            ))
        }
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let bytes: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| corrupt("blob-filesystem-metadata-u32", "invalid u32"))?;
        Ok(u32::from_be_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| corrupt("blob-filesystem-metadata-u64", "invalid u64"))?;
        Ok(u64::from_be_bytes(bytes))
    }

    fn array_32(&mut self) -> Result<[u8; 32], Error> {
        self.take(32)?
            .try_into()
            .map_err(|_| corrupt("blob-filesystem-metadata-digest", "invalid digest"))
    }

    fn text(&mut self, limit: usize) -> Result<String, Error> {
        let length = usize::try_from(self.u32()?).map_err(|_| {
            corrupt(
                "blob-filesystem-metadata-text-length",
                "text length does not fit usize",
            )
        })?;
        if length == 0 || length > limit {
            return Err(corrupt(
                "blob-filesystem-metadata-text-bounds",
                "metadata text exceeds configured bounds",
            ));
        }
        let value = self.take(length)?;
        String::from_utf8(value.to_vec()).map_err(|_| {
            corrupt(
                "blob-filesystem-metadata-text-utf8",
                "metadata text is not UTF-8",
            )
        })
    }

    fn finish(self) -> Result<(), Error> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(corrupt(
                "blob-filesystem-metadata-trailing",
                "metadata contains trailing bytes",
            ))
        }
    }
}

fn prepare_root(path: &Path) -> Result<PathBuf, Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(corrupt(
                    "blob-filesystem-root-invalid",
                    "trusted blob root must be a real directory",
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .map_err(|error| io_error("blob-filesystem-root-create", error))?;
        }
        Err(error) => return Err(io_error("blob-filesystem-root-stat", error)),
    }
    let root = fs::canonicalize(path)
        .map_err(|error| io_error("blob-filesystem-root-canonicalize", error))?;
    let metadata = fs::symlink_metadata(&root)
        .map_err(|error| io_error("blob-filesystem-root-stat", error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(corrupt(
            "blob-filesystem-root-invalid",
            "trusted blob root must resolve to a real directory",
        ));
    }
    Ok(root)
}

fn ensure_directory(path: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(corrupt(
                    "blob-filesystem-directory-invalid",
                    "provider-owned path is not a real directory",
                ));
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path)
                .map_err(|error| io_error("blob-filesystem-directory-create", error))?;
            if let Some(parent) = path.parent() {
                sync_directory(parent)?;
            }
            Ok(())
        }
        Err(error) => Err(io_error("blob-filesystem-directory-stat", error)),
    }
}

fn ensure_regular_or_missing(path: &Path, code: &'static str) -> Result<(), Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(corrupt(code, "provider-owned path is not a regular file"))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(code, error)),
    }
}

fn regular_file_exists(path: &Path, code: &'static str) -> Result<bool, Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(corrupt(code, "provider-owned path is not a regular file"))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(code, error)),
    }
}

fn remove_regular_if_exists(path: &Path, code: &'static str) -> Result<(), Error> {
    if regular_file_exists(path, code)? {
        fs::remove_file(path).map_err(|error| io_error(code, error))?;
    }
    Ok(())
}

fn read_directory(path: &Path) -> Result<Vec<fs::DirEntry>, Error> {
    fs::read_dir(path)
        .map_err(|error| io_error("blob-filesystem-directory-read", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| io_error("blob-filesystem-directory-entry", error))
}

fn cleanup_temporary_files(directory: &Path) -> Result<(), Error> {
    for entry in read_directory(directory)? {
        let path = entry.path();
        if is_temporary(&path) {
            remove_regular_if_exists(&path, "blob-filesystem-temporary-remove")?;
        }
    }
    sync_directory(directory)
}

fn is_temporary(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(|name| name.starts_with(TEMP_PREFIX))
        .unwrap_or(false)
}

fn count_extension(directory: &Path, extension: &str) -> Result<usize, Error> {
    let mut count = 0_usize;
    for entry in read_directory(directory)? {
        let path = entry.path();
        if path.extension() == Some(OsStr::new(extension)) {
            if !regular_file_exists(&path, "blob-filesystem-capacity-entry-invalid")? {
                continue;
            }
            count = count.checked_add(1).ok_or_else(|| {
                corrupt(
                    "blob-filesystem-capacity-overflow",
                    "filesystem entry count overflow",
                )
            })?;
        }
    }
    Ok(count)
}

fn count_object_metadata(objects_dir: &Path) -> Result<usize, Error> {
    let mut count = 0_usize;
    for entry in read_directory(objects_dir)? {
        let prefix = entry.path();
        ensure_directory(&prefix)?;
        count = count
            .checked_add(count_extension(&prefix, "meta")?)
            .ok_or_else(|| {
                corrupt(
                    "blob-filesystem-capacity-overflow",
                    "filesystem object count overflow",
                )
            })?;
    }
    Ok(count)
}

fn staging_id(key: &StagingKey) -> String {
    hex(&Sha256::digest(key.as_str().as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn hash_file(path: &Path, limits: Limits) -> Result<(Digest, u64), Error> {
    ensure_regular_or_missing(path, "blob-filesystem-data-invalid")?;
    let metadata =
        fs::metadata(path).map_err(|error| io_error("blob-filesystem-data-stat", error))?;
    let size = metadata.len();
    if size > limits.max_object_bytes {
        return Err(Error::ObjectLimitExceeded {
            limit: limits.max_object_bytes,
            actual: size,
        });
    }
    let mut file =
        File::open(path).map_err(|error| io_error("blob-filesystem-data-open", error))?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; IO_CHUNK_BYTES.min(limits.max_source_chunk_bytes.max(1))];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| io_error("blob-filesystem-data-read", error))?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or_else(|| {
            corrupt(
                "blob-filesystem-data-size-overflow",
                "stored object size overflow",
            )
        })?;
        if total > limits.max_object_bytes {
            return Err(Error::ObjectLimitExceeded {
                limit: limits.max_object_bytes,
                actual: total,
            });
        }
        hasher.update(&buffer[..read]);
    }
    if total != size {
        return Err(corrupt(
            "blob-filesystem-data-size-race",
            "stored object size changed while it was verified",
        ));
    }
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&digest);
    Ok((Digest::from_bytes(bytes), total))
}

fn file_length(path: &Path, code: &'static str) -> Result<u64, Error> {
    ensure_regular_or_missing(path, code)?;
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| io_error(code, error))
}

fn read_small_file(path: &Path, limit: u64, code: &'static str) -> Result<Vec<u8>, Error> {
    ensure_regular_or_missing(path, code)?;
    let length = fs::metadata(path)
        .map_err(|error| io_error(code, error))?
        .len();
    if length == 0 || length > limit {
        return Err(corrupt(code, "metadata file exceeds its bounded profile"));
    }
    let mut file = File::open(path).map_err(|error| io_error(code, error))?;
    let mut bytes = Vec::with_capacity(length as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| io_error(code, error))?;
    if bytes.len() as u64 != length {
        return Err(corrupt(code, "metadata length changed while reading"));
    }
    Ok(bytes)
}

fn atomic_create(path: &Path, bytes: &[u8]) -> Result<bool, Error> {
    let parent = path
        .parent()
        .ok_or_else(|| corrupt("blob-filesystem-atomic-path", "atomic path has no parent"))?;
    ensure_directory(parent)?;
    ensure_regular_or_missing(path, "blob-filesystem-atomic-target-invalid")?;
    let temporary = temporary_path(parent);
    write_temporary(&temporary, bytes)?;
    let linked = match fs::hard_link(&temporary, path) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(io_error("blob-filesystem-atomic-link", error));
        }
    };
    fs::remove_file(&temporary)
        .map_err(|error| io_error("blob-filesystem-temporary-remove", error))?;
    if linked {
        sync_directory(parent)?;
    }
    Ok(linked)
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let parent = path
        .parent()
        .ok_or_else(|| corrupt("blob-filesystem-atomic-path", "atomic path has no parent"))?;
    ensure_directory(parent)?;
    ensure_regular_or_missing(path, "blob-filesystem-atomic-target-invalid")?;
    let temporary = temporary_path(parent);
    write_temporary(&temporary, bytes)?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(io_error("blob-filesystem-atomic-replace", error));
    }
    sync_directory(parent)
}

fn write_temporary(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| io_error("blob-filesystem-temporary-create", error))?;
    if let Err(error) = file.write_all(bytes) {
        let _ = fs::remove_file(path);
        return Err(io_error("blob-filesystem-temporary-write", error));
    }
    if let Err(error) = file.sync_all() {
        let _ = fs::remove_file(path);
        return Err(io_error("blob-filesystem-temporary-sync", error));
    }
    Ok(())
}

fn temporary_path(parent: &Path) -> PathBuf {
    let id = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    parent.join(format!("{TEMP_PREFIX}{}-{id}.tmp", std::process::id()))
}

fn sync_directory(path: &Path) -> Result<(), Error> {
    let directory =
        File::open(path).map_err(|error| io_error("blob-filesystem-directory-sync-open", error))?;
    directory
        .sync_all()
        .map_err(|error| io_error("blob-filesystem-directory-sync", error))
}

fn io_error(code: &'static str, error: io::Error) -> Error {
    Error::driver(code, error.to_string())
}

fn corrupt(code: &'static str, detail: &'static str) -> Error {
    Error::driver(code, detail)
}

#[cfg(test)]
mod tests;
