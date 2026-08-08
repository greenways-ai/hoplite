#![forbid(unsafe_code)]

//! Application-neutral staged blob mechanics for Hoplite.
//!
//! This crate owns bounded source consumption, resumable staging offsets,
//! digest verification, atomic in-memory commit and immutable range sources.
//! Application upload state, authorization, quotas, graphs and recovery policy
//! remain above this boundary in Hara.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard};

pub const SERVICE: &str = "hara.blob";
pub const REQUEST_PROTOCOL: &str = "hara.blob-request/1";
pub const RESULT_PROTOCOL: &str = "hara.blob-result/1";

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest([u8; 32]);

impl Digest {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn parse(value: &str) -> Result<Self, Error> {
        let hex = value.strip_prefix("sha256:").ok_or(Error::InvalidDigest)?;
        if hex.len() != 64 {
            return Err(Error::InvalidDigest);
        }
        let mut bytes = [0_u8; 32];
        for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
            bytes[index] = (lower_hex(pair[0])? << 4) | lower_hex(pair[1])?;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Digest")
            .field(&self.to_string())
            .finish()
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("sha256:")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for Digest {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

fn lower_hex(byte: u8) -> Result<u8, Error> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(Error::InvalidDigest),
    }
}

pub trait DigestVerifier {
    fn sha256(&self, bytes: &[u8]) -> [u8; 32];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_object_bytes: u64,
    pub max_append_bytes: usize,
    pub max_source_chunk_bytes: usize,
    pub max_staging_key_bytes: usize,
    pub max_media_type_bytes: usize,
    pub max_staging_entries: usize,
    pub max_objects: usize,
}

impl Limits {
    pub fn validate(self) -> Result<Self, Error> {
        if self.max_object_bytes == 0 {
            return Err(Error::InvalidLimits("max_object_bytes must be positive"));
        }
        if self.max_append_bytes == 0 {
            return Err(Error::InvalidLimits("max_append_bytes must be positive"));
        }
        if self.max_source_chunk_bytes == 0
            || self.max_source_chunk_bytes > self.max_append_bytes
        {
            return Err(Error::InvalidLimits(
                "max_source_chunk_bytes must be positive and no greater than max_append_bytes",
            ));
        }
        if self.max_staging_key_bytes == 0
            || self.max_media_type_bytes == 0
            || self.max_staging_entries == 0
            || self.max_objects == 0
        {
            return Err(Error::InvalidLimits(
                "key, media type, staging and object limits must be positive",
            ));
        }
        Ok(self)
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_object_bytes: 16 * 1024 * 1024,
            max_append_bytes: 1024 * 1024,
            max_source_chunk_bytes: 64 * 1024,
            max_staging_key_bytes: 256,
            max_media_type_bytes: 256,
            max_staging_entries: 1024,
            max_objects: 65_536,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StagingKey(String);

impl StagingKey {
    pub fn new(value: impl Into<String>, limits: Limits) -> Result<Self, Error> {
        let value = value.into();
        validate_logical_text(
            &value,
            limits.validate()?.max_staging_key_bytes,
            Error::InvalidStagingKey,
        )?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaType(String);

impl MediaType {
    pub fn new(value: impl Into<String>, limits: Limits) -> Result<Self, Error> {
        let value = value.into();
        validate_logical_text(
            &value,
            limits.validate()?.max_media_type_bytes,
            Error::InvalidMediaType,
        )?;
        if !value.contains('/') || value.chars().any(char::is_whitespace) {
            return Err(Error::InvalidMediaType);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate_logical_text(
    value: &str,
    limit: usize,
    error: Error,
) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > limit
        || value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(error);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagingOpen {
    pub staging_key: StagingKey,
    pub expected_digest: Digest,
    pub expected_size: u64,
    pub media_type: MediaType,
}

impl StagingOpen {
    pub fn new(
        staging_key: StagingKey,
        expected_digest: Digest,
        expected_size: u64,
        media_type: MediaType,
        limits: Limits,
    ) -> Result<Self, Error> {
        let limits = limits.validate()?;
        if expected_size > limits.max_object_bytes {
            return Err(Error::ObjectLimitExceeded {
                limit: limits.max_object_bytes,
                actual: expected_size,
            });
        }
        Ok(Self {
            staging_key,
            expected_digest,
            expected_size,
            media_type,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagingStatus {
    pub staging_key: StagingKey,
    pub offset: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagingAppend {
    pub staging_key: StagingKey,
    pub offset: u64,
    pub length: usize,
}

impl StagingAppend {
    pub fn new(
        staging_key: StagingKey,
        offset: u64,
        length: usize,
        limits: Limits,
    ) -> Result<Self, Error> {
        let limits = limits.validate()?;
        if length == 0 || length > limits.max_append_bytes {
            return Err(Error::AppendLimitExceeded {
                limit: limits.max_append_bytes,
                actual: length,
            });
        }
        Ok(Self {
            staging_key,
            offset,
            length,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppendReceipt {
    pub staging_key: StagingKey,
    pub offset: u64,
    pub length: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagingCommit {
    pub staging_key: StagingKey,
    pub expected_digest: Digest,
    pub expected_size: u64,
}

impl StagingCommit {
    pub fn new(
        staging_key: StagingKey,
        expected_digest: Digest,
        expected_size: u64,
        limits: Limits,
    ) -> Result<Self, Error> {
        let limits = limits.validate()?;
        if expected_size > limits.max_object_bytes {
            return Err(Error::ObjectLimitExceeded {
                limit: limits.max_object_bytes,
                actual: expected_size,
            });
        }
        Ok(Self {
            staging_key,
            expected_digest,
            expected_size,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectDescriptor {
    pub digest: Digest,
    pub size: u64,
    pub media_type: MediaType,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectRange {
    pub digest: Digest,
    pub offset: u64,
    pub length: u64,
}

impl ObjectRange {
    pub fn new(digest: Digest, offset: u64, length: u64) -> Result<Self, Error> {
        if length == 0 || offset.checked_add(length).is_none() {
            return Err(Error::InvalidRange {
                offset,
                length,
                size: None,
            });
        }
        Ok(Self {
            digest,
            offset,
            length,
        })
    }
}

pub trait ByteSource {
    fn read(&mut self, output: &mut [u8]) -> Result<usize, Error>;
    fn finish(&mut self) -> Result<(), Error>;
}

pub trait ResponseSource {
    fn declared_length(&self) -> u64;
    fn read(&mut self, output: &mut [u8]) -> Result<usize, Error>;
    fn close(&mut self) -> Result<(), Error>;
}

pub trait BlobStore: Send + Sync {
    type Source: ResponseSource;

    fn staging_open(&self, request: StagingOpen) -> Result<StagingStatus, Error>;

    fn staging_append_from_source(
        &self,
        request: StagingAppend,
        source: &mut dyn ByteSource,
    ) -> Result<AppendReceipt, Error>;

    fn staging_abort(&self, staging_key: &StagingKey) -> Result<(), Error>;

    fn staging_verify_commit(
        &self,
        request: StagingCommit,
    ) -> Result<ObjectDescriptor, Error>;

    fn object_open_source(&self, request: ObjectRange) -> Result<Self::Source, Error>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidLimits(&'static str),
    InvalidDigest,
    InvalidStagingKey,
    InvalidMediaType,
    ObjectLimitExceeded {
        limit: u64,
        actual: u64,
    },
    AppendLimitExceeded {
        limit: usize,
        actual: usize,
    },
    StagingCapacity {
        limit: usize,
    },
    ObjectCapacity {
        limit: usize,
    },
    StagingConflict {
        staging_key: StagingKey,
    },
    StagingMissing {
        staging_key: StagingKey,
    },
    OffsetMismatch {
        expected: u64,
        actual: u64,
    },
    SourceShort {
        expected: usize,
        actual: usize,
    },
    SourceLong {
        expected: usize,
    },
    SourceProtocol {
        detail: &'static str,
    },
    SourceFailure {
        code: &'static str,
        detail: String,
    },
    IncompleteStaging {
        expected: u64,
        actual: u64,
    },
    DigestMismatch {
        expected: Digest,
        actual: Digest,
    },
    ObjectConflict {
        digest: Digest,
    },
    ObjectMissing {
        digest: Digest,
    },
    InvalidRange {
        offset: u64,
        length: u64,
        size: Option<u64>,
    },
    SourceClosed,
    Poisoned,
    Driver {
        code: &'static str,
        detail: String,
    },
}

impl Error {
    pub fn source(code: &'static str, detail: impl Into<String>) -> Self {
        Self::SourceFailure {
            code,
            detail: detail.into(),
        }
    }

    pub fn driver(code: &'static str, detail: impl Into<String>) -> Self {
        Self::Driver {
            code,
            detail: detail.into(),
        }
    }

    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidLimits(_) => "blob-limits-invalid",
            Self::InvalidDigest => "blob-digest-invalid",
            Self::InvalidStagingKey => "blob-staging-key-invalid",
            Self::InvalidMediaType => "blob-media-type-invalid",
            Self::ObjectLimitExceeded { .. } => "blob-object-limit",
            Self::AppendLimitExceeded { .. } => "blob-append-limit",
            Self::StagingCapacity { .. } => "blob-staging-capacity",
            Self::ObjectCapacity { .. } => "blob-object-capacity",
            Self::StagingConflict { .. } => "blob-staging-conflict",
            Self::StagingMissing { .. } => "blob-staging-missing",
            Self::OffsetMismatch { .. } => "blob-offset-mismatch",
            Self::SourceShort { .. } => "blob-source-short",
            Self::SourceLong { .. } => "blob-source-long",
            Self::SourceProtocol { .. } => "blob-source-protocol",
            Self::SourceFailure { code, .. } | Self::Driver { code, .. } => code,
            Self::IncompleteStaging { .. } => "blob-staging-incomplete",
            Self::DigestMismatch { .. } => "blob-digest-mismatch",
            Self::ObjectConflict { .. } => "blob-object-conflict",
            Self::ObjectMissing { .. } => "blob-object-missing",
            Self::InvalidRange { .. } => "blob-range-invalid",
            Self::SourceClosed => "blob-source-closed",
            Self::Poisoned => "blob-lock-poisoned",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.code())
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Debug)]
struct StagingEntry {
    expected_digest: Digest,
    expected_size: u64,
    media_type: MediaType,
    bytes: Vec<u8>,
}

impl StagingEntry {
    fn compatible(&self, request: &StagingOpen) -> bool {
        self.expected_digest == request.expected_digest
            && self.expected_size == request.expected_size
            && self.media_type == request.media_type
    }
}

#[derive(Clone, Debug)]
struct ObjectEntry {
    descriptor: ObjectDescriptor,
    bytes: Arc<[u8]>,
}

#[derive(Debug, Default)]
struct MemoryState {
    staging: BTreeMap<StagingKey, StagingEntry>,
    objects: BTreeMap<Digest, ObjectEntry>,
}

pub struct InMemoryBlobStore<V> {
    state: Mutex<MemoryState>,
    verifier: V,
    limits: Limits,
}

impl<V> InMemoryBlobStore<V>
where
    V: DigestVerifier + Send + Sync,
{
    pub fn new(verifier: V, limits: Limits) -> Result<Self, Error> {
        Ok(Self {
            state: Mutex::new(MemoryState::default()),
            verifier,
            limits: limits.validate()?,
        })
    }

    pub const fn limits(&self) -> Limits {
        self.limits
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, MemoryState>, Error> {
        self.state.lock().map_err(|_| Error::Poisoned)
    }

    pub fn staging_offset(&self, key: &StagingKey) -> Result<Option<u64>, Error> {
        let state = self.lock_state()?;
        state
            .staging
            .get(key)
            .map(|entry| u64::try_from(entry.bytes.len()).map_err(|_| Error::SourceProtocol {
                detail: "staging length exceeds u64",
            }))
            .transpose()
    }

    pub fn object_descriptor(&self, digest: Digest) -> Result<Option<ObjectDescriptor>, Error> {
        Ok(self
            .lock_state()?
            .objects
            .get(&digest)
            .map(|entry| entry.descriptor.clone()))
    }

    fn consume_source(
        &self,
        source: &mut dyn ByteSource,
        length: usize,
    ) -> Result<Vec<u8>, Error> {
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

impl<V> BlobStore for InMemoryBlobStore<V>
where
    V: DigestVerifier + Send + Sync,
{
    type Source = MemoryResponseSource;

    fn staging_open(&self, request: StagingOpen) -> Result<StagingStatus, Error> {
        let mut state = self.lock_state()?;
        if let Some(current) = state.staging.get(&request.staging_key) {
            if !current.compatible(&request) {
                return Err(Error::StagingConflict {
                    staging_key: request.staging_key,
                });
            }
            return Ok(StagingStatus {
                staging_key: request.staging_key,
                offset: current.bytes.len() as u64,
            });
        }
        if state.staging.len() >= self.limits.max_staging_entries {
            return Err(Error::StagingCapacity {
                limit: self.limits.max_staging_entries,
            });
        }
        state.staging.insert(
            request.staging_key.clone(),
            StagingEntry {
                expected_digest: request.expected_digest,
                expected_size: request.expected_size,
                media_type: request.media_type,
                bytes: Vec::new(),
            },
        );
        Ok(StagingStatus {
            staging_key: request.staging_key,
            offset: 0,
        })
    }

    fn staging_append_from_source(
        &self,
        request: StagingAppend,
        source: &mut dyn ByteSource,
    ) -> Result<AppendReceipt, Error> {
        if request.length == 0 || request.length > self.limits.max_append_bytes {
            return Err(Error::AppendLimitExceeded {
                limit: self.limits.max_append_bytes,
                actual: request.length,
            });
        }
        let mut state = self.lock_state()?;
        let current = state
            .staging
            .get_mut(&request.staging_key)
            .ok_or_else(|| Error::StagingMissing {
                staging_key: request.staging_key.clone(),
            })?;
        let offset = current.bytes.len() as u64;
        if offset != request.offset {
            return Err(Error::OffsetMismatch {
                expected: offset,
                actual: request.offset,
            });
        }
        let next = request
            .offset
            .checked_add(request.length as u64)
            .ok_or(Error::ObjectLimitExceeded {
                limit: current.expected_size,
                actual: u64::MAX,
            })?;
        if next > current.expected_size {
            return Err(Error::ObjectLimitExceeded {
                limit: current.expected_size,
                actual: next,
            });
        }
        let bytes = self.consume_source(source, request.length)?;
        current.bytes.extend_from_slice(&bytes);
        Ok(AppendReceipt {
            staging_key: request.staging_key,
            offset: next,
            length: request.length,
        })
    }

    fn staging_abort(&self, staging_key: &StagingKey) -> Result<(), Error> {
        self.lock_state()?.staging.remove(staging_key);
        Ok(())
    }

    fn staging_verify_commit(
        &self,
        request: StagingCommit,
    ) -> Result<ObjectDescriptor, Error> {
        let mut state = self.lock_state()?;
        if let Some(object) = state.objects.get(&request.expected_digest) {
            if object.descriptor.size == request.expected_size {
                state.staging.remove(&request.staging_key);
                return Ok(object.descriptor.clone());
            }
            return Err(Error::ObjectConflict {
                digest: request.expected_digest,
            });
        }
        let current = state
            .staging
            .get(&request.staging_key)
            .ok_or_else(|| Error::StagingMissing {
                staging_key: request.staging_key.clone(),
            })?;
        if current.expected_digest != request.expected_digest
            || current.expected_size != request.expected_size
        {
            return Err(Error::StagingConflict {
                staging_key: request.staging_key,
            });
        }
        let actual_size = current.bytes.len() as u64;
        if actual_size != request.expected_size {
            return Err(Error::IncompleteStaging {
                expected: request.expected_size,
                actual: actual_size,
            });
        }
        let actual_digest = Digest::from_bytes(self.verifier.sha256(&current.bytes));
        if actual_digest != request.expected_digest {
            return Err(Error::DigestMismatch {
                expected: request.expected_digest,
                actual: actual_digest,
            });
        }
        if state.objects.len() >= self.limits.max_objects {
            return Err(Error::ObjectCapacity {
                limit: self.limits.max_objects,
            });
        }
        let current = state
            .staging
            .remove(&request.staging_key)
            .expect("staging entry was checked above");
        let descriptor = ObjectDescriptor {
            digest: request.expected_digest,
            size: request.expected_size,
            media_type: current.media_type,
        };
        state.objects.insert(
            descriptor.digest,
            ObjectEntry {
                descriptor: descriptor.clone(),
                bytes: Arc::from(current.bytes),
            },
        );
        Ok(descriptor)
    }

    fn object_open_source(&self, request: ObjectRange) -> Result<Self::Source, Error> {
        let state = self.lock_state()?;
        let object = state
            .objects
            .get(&request.digest)
            .ok_or(Error::ObjectMissing {
                digest: request.digest,
            })?;
        let end = request
            .offset
            .checked_add(request.length)
            .ok_or(Error::InvalidRange {
                offset: request.offset,
                length: request.length,
                size: Some(object.descriptor.size),
            })?;
        if request.length == 0 || end > object.descriptor.size {
            return Err(Error::InvalidRange {
                offset: request.offset,
                length: request.length,
                size: Some(object.descriptor.size),
            });
        }
        Ok(MemoryResponseSource {
            bytes: object.bytes.clone(),
            start: request.offset as usize,
            end: end as usize,
            cursor: request.offset as usize,
            closed: false,
        })
    }
}

#[derive(Debug)]
pub struct MemoryResponseSource {
    bytes: Arc<[u8]>,
    start: usize,
    end: usize,
    cursor: usize,
    closed: bool,
}

impl ResponseSource for MemoryResponseSource {
    fn declared_length(&self) -> u64 {
        (self.end - self.start) as u64
    }

    fn read(&mut self, output: &mut [u8]) -> Result<usize, Error> {
        if self.closed {
            return Err(Error::SourceClosed);
        }
        if output.is_empty() || self.cursor == self.end {
            return Ok(0);
        }
        let amount = output.len().min(self.end - self.cursor);
        output[..amount].copy_from_slice(&self.bytes[self.cursor..self.cursor + amount]);
        self.cursor += amount;
        Ok(amount)
    }

    fn close(&mut self) -> Result<(), Error> {
        if self.closed {
            return Err(Error::SourceClosed);
        }
        self.closed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    struct TestVerifier;

    impl DigestVerifier for TestVerifier {
        fn sha256(&self, bytes: &[u8]) -> [u8; 32] {
            let mut output = [0_u8; 32];
            for (index, byte) in bytes.iter().copied().enumerate() {
                let slot = index % output.len();
                output[slot] = output[slot]
                    .wrapping_add(byte)
                    .wrapping_add(index as u8);
            }
            output
        }
    }

    #[derive(Debug)]
    struct VecSource {
        bytes: Vec<u8>,
        cursor: usize,
        finished: usize,
        fail_after: Option<usize>,
    }

    impl VecSource {
        fn new(bytes: impl Into<Vec<u8>>) -> Self {
            Self {
                bytes: bytes.into(),
                cursor: 0,
                finished: 0,
                fail_after: None,
            }
        }
    }

    impl ByteSource for VecSource {
        fn read(&mut self, output: &mut [u8]) -> Result<usize, Error> {
            if self
                .fail_after
                .is_some_and(|limit| self.cursor >= limit)
            {
                return Err(Error::source("fixture-read", "injected read failure"));
            }
            let amount = output.len().min(self.bytes.len() - self.cursor);
            output[..amount]
                .copy_from_slice(&self.bytes[self.cursor..self.cursor + amount]);
            self.cursor += amount;
            Ok(amount)
        }

        fn finish(&mut self) -> Result<(), Error> {
            self.finished += 1;
            Ok(())
        }
    }

    fn limits() -> Limits {
        Limits {
            max_object_bytes: 1024,
            max_append_bytes: 128,
            max_source_chunk_bytes: 3,
            max_staging_key_bytes: 64,
            max_media_type_bytes: 64,
            max_staging_entries: 8,
            max_objects: 8,
        }
    }

    fn digest(bytes: &[u8]) -> Digest {
        Digest::from_bytes(TestVerifier.sha256(bytes))
    }

    fn key(value: &str) -> StagingKey {
        StagingKey::new(value, limits()).unwrap()
    }

    fn media_type() -> MediaType {
        MediaType::new("application/octet-stream", limits()).unwrap()
    }

    fn store() -> InMemoryBlobStore<TestVerifier> {
        InMemoryBlobStore::new(TestVerifier, limits()).unwrap()
    }

    fn open(store: &InMemoryBlobStore<TestVerifier>, key: &str, bytes: &[u8]) {
        store
            .staging_open(
                StagingOpen::new(
                    self::key(key),
                    digest(bytes),
                    bytes.len() as u64,
                    media_type(),
                    limits(),
                )
                .unwrap(),
            )
            .unwrap();
    }

    #[test]
    fn opens_resumes_appends_commits_and_reads_ranges() {
        let store = store();
        let bytes = b"abcdefgh";
        open(&store, "upload.a", bytes);
        assert_eq!(
            store
                .staging_open(
                    StagingOpen::new(
                        key("upload.a"),
                        digest(bytes),
                        bytes.len() as u64,
                        media_type(),
                        limits(),
                    )
                    .unwrap(),
                )
                .unwrap()
                .offset,
            0
        );

        let mut first = VecSource::new(&bytes[..3]);
        let receipt = store
            .staging_append_from_source(
                StagingAppend::new(key("upload.a"), 0, 3, limits()).unwrap(),
                &mut first,
            )
            .unwrap();
        assert_eq!(receipt.offset, 3);
        assert_eq!(first.finished, 1);

        let mut second = VecSource::new(&bytes[3..]);
        store
            .staging_append_from_source(
                StagingAppend::new(key("upload.a"), 3, 5, limits()).unwrap(),
                &mut second,
            )
            .unwrap();
        let descriptor = store
            .staging_verify_commit(
                StagingCommit::new(
                    key("upload.a"),
                    digest(bytes),
                    bytes.len() as u64,
                    limits(),
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(descriptor.size, 8);
        assert!(store.staging_offset(&key("upload.a")).unwrap().is_none());

        let mut source = store
            .object_open_source(ObjectRange::new(digest(bytes), 2, 4).unwrap())
            .unwrap();
        assert_eq!(source.declared_length(), 4);
        let mut output = [0_u8; 8];
        assert_eq!(source.read(&mut output).unwrap(), 4);
        assert_eq!(&output[..4], b"cdef");
        assert_eq!(source.read(&mut output).unwrap(), 0);
        source.close().unwrap();
        assert_eq!(source.read(&mut output).unwrap_err(), Error::SourceClosed);
    }

    #[test]
    fn rejects_short_long_wrong_offset_and_failed_sources_without_advancing() {
        let store = store();
        open(&store, "upload.a", b"abcd");

        let mut short = VecSource::new(b"a".to_vec());
        assert!(matches!(
            store.staging_append_from_source(
                StagingAppend::new(key("upload.a"), 0, 2, limits()).unwrap(),
                &mut short
            ),
            Err(Error::SourceShort { .. })
        ));
        assert_eq!(short.finished, 1);
        assert_eq!(store.staging_offset(&key("upload.a")).unwrap(), Some(0));

        let mut long = VecSource::new(b"abc".to_vec());
        assert!(matches!(
            store.staging_append_from_source(
                StagingAppend::new(key("upload.a"), 0, 2, limits()).unwrap(),
                &mut long
            ),
            Err(Error::SourceLong { .. })
        ));
        assert_eq!(long.finished, 1);
        assert_eq!(store.staging_offset(&key("upload.a")).unwrap(), Some(0));

        let mut exact = VecSource::new(b"ab".to_vec());
        assert!(matches!(
            store.staging_append_from_source(
                StagingAppend::new(key("upload.a"), 1, 2, limits()).unwrap(),
                &mut exact
            ),
            Err(Error::OffsetMismatch { .. })
        ));
        assert_eq!(exact.finished, 0);

        let mut failing = VecSource::new(b"ab".to_vec());
        failing.fail_after = Some(0);
        assert_eq!(
            store
                .staging_append_from_source(
                    StagingAppend::new(key("upload.a"), 0, 2, limits()).unwrap(),
                    &mut failing
                )
                .unwrap_err()
                .code(),
            "fixture-read"
        );
        assert_eq!(failing.finished, 1);
        assert_eq!(store.staging_offset(&key("upload.a")).unwrap(), Some(0));
    }

    #[test]
    fn rejects_incomplete_and_digest_mismatched_commit() {
        let store = store();
        open(&store, "upload.a", b"abcd");
        let mut source = VecSource::new(b"ab".to_vec());
        store
            .staging_append_from_source(
                StagingAppend::new(key("upload.a"), 0, 2, limits()).unwrap(),
                &mut source,
            )
            .unwrap();
        assert!(matches!(
            store.staging_verify_commit(
                StagingCommit::new(key("upload.a"), digest(b"abcd"), 4, limits()).unwrap()
            ),
            Err(Error::IncompleteStaging { .. })
        ));

        let store = store();
        let wrong = digest(b"wxyz");
        store
            .staging_open(
                StagingOpen::new(key("upload.b"), wrong, 4, media_type(), limits()).unwrap(),
            )
            .unwrap();
        let mut source = VecSource::new(b"abcd".to_vec());
        store
            .staging_append_from_source(
                StagingAppend::new(key("upload.b"), 0, 4, limits()).unwrap(),
                &mut source,
            )
            .unwrap();
        assert!(matches!(
            store.staging_verify_commit(
                StagingCommit::new(key("upload.b"), wrong, 4, limits()).unwrap()
            ),
            Err(Error::DigestMismatch { .. })
        ));
    }

    #[test]
    fn rejects_staging_identity_collisions_and_supports_abort() {
        let store = store();
        open(&store, "upload.a", b"abc");
        let conflict = StagingOpen::new(
            key("upload.a"),
            digest(b"different"),
            9,
            media_type(),
            limits(),
        )
        .unwrap();
        assert!(matches!(
            store.staging_open(conflict),
            Err(Error::StagingConflict { .. })
        ));
        store.staging_abort(&key("upload.a")).unwrap();
        assert_eq!(store.staging_offset(&key("upload.a")).unwrap(), None);
        store.staging_abort(&key("upload.a")).unwrap();
    }

    #[test]
    fn commit_is_idempotent_for_an_existing_exact_object() {
        let store = store();
        let bytes = b"abc";
        open(&store, "upload.a", bytes);
        let mut source = VecSource::new(bytes.to_vec());
        store
            .staging_append_from_source(
                StagingAppend::new(key("upload.a"), 0, 3, limits()).unwrap(),
                &mut source,
            )
            .unwrap();
        let request = StagingCommit::new(key("upload.a"), digest(bytes), 3, limits()).unwrap();
        let first = store.staging_verify_commit(request.clone()).unwrap();
        let second = store.staging_verify_commit(request).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn validates_canonical_identifiers_and_ranges() {
        assert!(Digest::parse(&digest(b"x").to_string()).is_ok());
        assert!(Digest::parse("sha256:ABC").is_err());
        assert!(StagingKey::new("../escape", limits()).is_err());
        assert!(MediaType::new("bad type", limits()).is_err());
        assert!(ObjectRange::new(digest(b"x"), 0, 0).is_err());
    }
}
