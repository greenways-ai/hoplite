#![forbid(unsafe_code)]

//! Canonical `hoplite.blob` request/result adapter for generic blob-store drivers.
//!
//! The adapter understands only application-neutral staged blob mechanics. A
//! request source is resolved through a scope-bound resolver, so a numeric
//! handle is never authority by itself. Immutable response sources are handed
//! to a trusted registrar and return to Hara only as opaque positive handles.

use hoplite_blob_store::{
    AppendReceipt, BlobStore, ByteSource, Digest, Error as BlobError, Limits, MediaType,
    ObjectDescriptor, ObjectRange, ResponseSource, StagingAppend, StagingCommit, StagingKey,
    StagingOpen, StagingStatus, REQUEST_PROTOCOL, RESULT_PROTOCOL,
};
use hoplite_provider_hta::{Document, Error as HtaError, Kind, Node};
use std::fmt;

const MAGIC: &[u8; 4] = b"HTA1";
const FALSE: u8 = 1;
const TRUE: u8 = 2;
const I64: u8 = 3;
const STRING: u8 = 4;
const KEYWORD: u8 = 6;
const MAP: u8 = 11;

const OPEN_FIELDS: &[&str] = &[
    "expected-digest",
    "expected-size",
    "media-type",
    "operation",
    "protocol",
    "staging-key",
];
const APPEND_FIELDS: &[&str] = &[
    "length",
    "offset",
    "operation",
    "protocol",
    "source-handle",
    "staging-key",
];
const ABORT_FIELDS: &[&str] = &["operation", "protocol", "staging-key"];
const COMMIT_FIELDS: &[&str] = &[
    "expected-digest",
    "expected-size",
    "operation",
    "protocol",
    "staging-key",
];
const OPEN_SOURCE_FIELDS: &[&str] = &["digest", "length", "offset", "operation", "protocol"];

pub trait RequestSourceResolver {
    type Source: ByteSource;

    /// Resolve through a context already bound to the exact request and work.
    fn resolve(&self, source_handle: u64) -> Result<Self::Source, BlobError>;
}

pub trait ResponseSourceRegistrar<S>
where
    S: ResponseSource,
{
    /// Register one immutable source in a trusted work-scoped registry.
    fn register(&self, source: S) -> Result<u64, BlobError>;
}

#[derive(Debug)]
pub enum Error {
    Hta(HtaError),
    InvalidRequest(&'static str),
    OperationMismatch { call: String, request: String },
    Blob(BlobError),
    ResponseHandleInvalid,
}

impl Error {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Hta(_) => "blob-request-hta",
            Self::InvalidRequest(_) => "blob-request-invalid",
            Self::OperationMismatch { .. } => "blob-operation-mismatch",
            Self::Blob(error) => error.code(),
            Self::ResponseHandleInvalid => "blob-response-handle-invalid",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hta(error) => write!(formatter, "invalid provider HTA: {error}"),
            Self::InvalidRequest(message) => {
                write!(formatter, "invalid hoplite.blob request: {message}")
            }
            Self::OperationMismatch { call, request } => write!(
                formatter,
                "host operation {call:?} does not match request operation {request:?}"
            ),
            Self::Blob(error) => write!(formatter, "blob-store error: {error}"),
            Self::ResponseHandleInvalid => {
                formatter.write_str("response-source registry returned an invalid handle")
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<HtaError> for Error {
    fn from(error: HtaError) -> Self {
        Self::Hta(error)
    }
}

impl From<BlobError> for Error {
    fn from(error: BlobError) -> Self {
        Self::Blob(error)
    }
}

pub struct Provider<S, I, O> {
    store: S,
    request_sources: I,
    response_sources: O,
    limits: Limits,
}

impl<S, I, O> Provider<S, I, O>
where
    S: BlobStore,
    I: RequestSourceResolver,
    O: ResponseSourceRegistrar<S::Source>,
{
    pub fn new(
        store: S,
        request_sources: I,
        response_sources: O,
        limits: Limits,
    ) -> Result<Self, Error> {
        Ok(Self {
            store,
            request_sources,
            response_sources,
            limits: limits.validate()?,
        })
    }

    pub const fn store(&self) -> &S {
        &self.store
    }

    pub const fn request_sources(&self) -> &I {
        &self.request_sources
    }

    pub const fn response_sources(&self) -> &O {
        &self.response_sources
    }

    pub const fn limits(&self) -> Limits {
        self.limits
    }

    pub fn execute(&self, operation: &str, arguments_hta: &[u8]) -> Result<Vec<u8>, Error> {
        let document = Document::parse(arguments_hta)?;
        let arguments = document.root();
        if arguments.kind() != Kind::Vector || arguments.len()? != 1 {
            return Err(Error::InvalidRequest(
                "host arguments must be a vector containing one request map",
            ));
        }
        let request = arguments.get(0)?;
        let request_operation = request_text(request, "operation")?;
        if operation != request_operation {
            return Err(Error::OperationMismatch {
                call: operation.to_owned(),
                request: request_operation.to_owned(),
            });
        }
        if request_text(request, "protocol")? != REQUEST_PROTOCOL {
            return Err(Error::InvalidRequest("request protocol is not supported"));
        }

        match operation {
            "staging/open" => {
                exact_fields(request, OPEN_FIELDS)?;
                self.staging_open(request)
            }
            "staging/append-from-source" => {
                exact_fields(request, APPEND_FIELDS)?;
                self.staging_append(request)
            }
            "staging/abort" => {
                exact_fields(request, ABORT_FIELDS)?;
                self.staging_abort(request)
            }
            "staging/verify-commit" => {
                exact_fields(request, COMMIT_FIELDS)?;
                self.staging_commit(request)
            }
            "object/open-source" => {
                exact_fields(request, OPEN_SOURCE_FIELDS)?;
                self.object_open_source(request)
            }
            _ => Err(Error::InvalidRequest("operation is not supported")),
        }
    }

    fn staging_open(&self, request: Node<'_, '_>) -> Result<Vec<u8>, Error> {
        let request = StagingOpen::new(
            request_staging_key(request, self.limits)?,
            request_digest(request, "expected-digest")?,
            request_u64(request, "expected-size", false)?,
            request_media_type(request, self.limits)?,
            self.limits,
        )?;
        let status = self.store.staging_open(request)?;
        open_result(&status)
    }

    fn staging_append(&self, request: Node<'_, '_>) -> Result<Vec<u8>, Error> {
        let source_handle = request_u64(request, "source-handle", true)?;
        let append = StagingAppend::new(
            request_staging_key(request, self.limits)?,
            request_u64(request, "offset", false)?,
            request_usize(request, "length", true)?,
            self.limits,
        )?;
        let mut source = self.request_sources.resolve(source_handle)?;
        let receipt = self.store.staging_append_from_source(append, &mut source)?;
        append_result(&receipt)
    }

    fn staging_abort(&self, request: Node<'_, '_>) -> Result<Vec<u8>, Error> {
        let staging_key = request_staging_key(request, self.limits)?;
        self.store.staging_abort(&staging_key)?;
        abort_result(&staging_key)
    }

    fn staging_commit(&self, request: Node<'_, '_>) -> Result<Vec<u8>, Error> {
        let staging_key = request_staging_key(request, self.limits)?;
        let commit = StagingCommit::new(
            staging_key.clone(),
            request_digest(request, "expected-digest")?,
            request_u64(request, "expected-size", false)?,
            self.limits,
        )?;
        let descriptor = self.store.staging_verify_commit(commit)?;
        commit_result(&staging_key, &descriptor)
    }

    fn object_open_source(&self, request: Node<'_, '_>) -> Result<Vec<u8>, Error> {
        let digest = request_digest(request, "digest")?;
        let offset = request_u64(request, "offset", false)?;
        let length = request_u64(request, "length", true)?;
        let source = self
            .store
            .object_open_source(ObjectRange::new(digest, offset, length)?)?;
        let source_handle = self.response_sources.register(source)?;
        if source_handle == 0 || source_handle > i64::MAX as u64 {
            return Err(Error::ResponseHandleInvalid);
        }
        open_source_result(source_handle, digest, offset, length)
    }
}

fn exact_fields(request: Node<'_, '_>, expected: &[&str]) -> Result<(), Error> {
    if request.kind() != Kind::Map || request.len()? != expected.len() {
        return Err(Error::InvalidRequest("request fields are not exact"));
    }
    let mut seen = Vec::with_capacity(expected.len());
    for index in 0..request.len()? {
        let (key, _) = request.pair(index)?;
        if !matches!(key.kind(), Kind::String | Kind::Keyword) {
            return Err(Error::InvalidRequest(
                "request keys must be strings or keywords",
            ));
        }
        let key = key.as_text()?;
        if !expected.contains(&key) || seen.contains(&key) {
            return Err(Error::InvalidRequest(
                "request contains unknown or duplicate fields",
            ));
        }
        seen.push(key);
    }
    Ok(())
}

fn request_text<'a>(request: Node<'_, 'a>, name: &str) -> Result<&'a str, Error> {
    let value = request.require(name)?;
    if value.kind() != Kind::String {
        return Err(Error::InvalidRequest("text fields must be strings"));
    }
    Ok(value.as_text()?)
}

fn request_u64(request: Node<'_, '_>, name: &str, positive: bool) -> Result<u64, Error> {
    let value = request.require(name)?.as_i64()?;
    let value = u64::try_from(value)
        .map_err(|_| Error::InvalidRequest("integer fields must be non-negative"))?;
    if positive && value == 0 {
        return Err(Error::InvalidRequest("integer field must be positive"));
    }
    Ok(value)
}

fn request_usize(request: Node<'_, '_>, name: &str, positive: bool) -> Result<usize, Error> {
    usize::try_from(request_u64(request, name, positive)?)
        .map_err(|_| Error::InvalidRequest("integer field exceeds the host size range"))
}

fn request_digest(request: Node<'_, '_>, name: &str) -> Result<Digest, Error> {
    Ok(Digest::parse(request_text(request, name)?)?)
}

fn request_staging_key(request: Node<'_, '_>, limits: Limits) -> Result<StagingKey, Error> {
    Ok(StagingKey::new(
        request_text(request, "staging-key")?.to_owned(),
        limits,
    )?)
}

fn request_media_type(request: Node<'_, '_>, limits: Limits) -> Result<MediaType, Error> {
    Ok(MediaType::new(
        request_text(request, "media-type")?.to_owned(),
        limits,
    )?)
}

fn open_result(status: &StagingStatus) -> Result<Vec<u8>, Error> {
    result_map(vec![
        ("offset", bare_i64(status.offset)?),
        ("opened", bare_bool(true)),
        ("operation", bare_string("staging/open")),
        ("protocol", bare_string(RESULT_PROTOCOL)),
        ("staging-key", bare_string(status.staging_key.as_str())),
    ])
}

fn append_result(receipt: &AppendReceipt) -> Result<Vec<u8>, Error> {
    result_map(vec![
        ("consumed", bare_bool(true)),
        ("length", bare_usize(receipt.length)?),
        ("offset", bare_i64(receipt.offset)?),
        ("operation", bare_string("staging/append-from-source")),
        ("protocol", bare_string(RESULT_PROTOCOL)),
        ("staging-key", bare_string(receipt.staging_key.as_str())),
    ])
}

fn abort_result(staging_key: &StagingKey) -> Result<Vec<u8>, Error> {
    result_map(vec![
        ("aborted", bare_bool(true)),
        ("operation", bare_string("staging/abort")),
        ("protocol", bare_string(RESULT_PROTOCOL)),
        ("staging-key", bare_string(staging_key.as_str())),
    ])
}

fn commit_result(
    staging_key: &StagingKey,
    descriptor: &ObjectDescriptor,
) -> Result<Vec<u8>, Error> {
    result_map(vec![
        ("committed", bare_bool(true)),
        ("digest", bare_string(&descriptor.digest.to_string())),
        ("operation", bare_string("staging/verify-commit")),
        ("protocol", bare_string(RESULT_PROTOCOL)),
        ("size", bare_i64(descriptor.size)?),
        ("staging-key", bare_string(staging_key.as_str())),
        ("verified", bare_bool(true)),
    ])
}

fn open_source_result(
    source_handle: u64,
    digest: Digest,
    offset: u64,
    length: u64,
) -> Result<Vec<u8>, Error> {
    result_map(vec![
        ("digest", bare_string(&digest.to_string())),
        ("length", bare_i64(length)?),
        ("offset", bare_i64(offset)?),
        ("operation", bare_string("object/open-source")),
        ("protocol", bare_string(RESULT_PROTOCOL)),
        ("source-handle", bare_i64(source_handle)?),
    ])
}

fn result_map(entries: Vec<(&str, Vec<u8>)>) -> Result<Vec<u8>, Error> {
    let mut entries = entries
        .into_iter()
        .map(|(key, value)| (bare_keyword(key), value))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let count = u32::try_from(entries.len())
        .map_err(|_| Error::InvalidRequest("too many result fields"))?;
    let mut output = MAGIC.to_vec();
    output.push(MAP);
    output.extend_from_slice(&count.to_be_bytes());
    for (key, value) in entries {
        output.extend_from_slice(&key);
        output.extend_from_slice(&value);
    }
    Ok(output)
}

fn bare_keyword(value: &str) -> Vec<u8> {
    bare_text(KEYWORD, value)
}

fn bare_string(value: &str) -> Vec<u8> {
    bare_text(STRING, value)
}

fn bare_text(tag: u8, value: &str) -> Vec<u8> {
    let mut output = vec![tag];
    output.extend_from_slice(&(value.len() as u32).to_be_bytes());
    output.extend_from_slice(value.as_bytes());
    output
}

fn bare_bool(value: bool) -> Vec<u8> {
    vec![if value { TRUE } else { FALSE }]
}

fn bare_i64(value: u64) -> Result<Vec<u8>, Error> {
    let value = i64::try_from(value)
        .map_err(|_| Error::InvalidRequest("integer result exceeds signed 64-bit range"))?;
    let mut output = vec![I64];
    output.extend_from_slice(&value.to_be_bytes());
    Ok(output)
}

fn bare_usize(value: usize) -> Result<Vec<u8>, Error> {
    let value = u64::try_from(value)
        .map_err(|_| Error::InvalidRequest("size result exceeds unsigned 64-bit range"))?;
    bare_i64(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoplite_blob_store::{DigestVerifier, InMemoryBlobStore, MemoryResponseSource};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    const NIL: u8 = 0;
    const VECTOR: u8 = 9;

    #[derive(Clone, Copy)]
    struct TestVerifier;

    impl DigestVerifier for TestVerifier {
        fn sha256(&self, bytes: &[u8]) -> [u8; 32] {
            let mut output = [0_u8; 32];
            for (index, byte) in bytes.iter().copied().enumerate() {
                let slot = index % output.len();
                output[slot] = output[slot].wrapping_add(byte).wrapping_add(index as u8);
            }
            output
        }
    }

    #[derive(Clone)]
    struct SharedIngress {
        fixtures: Arc<Mutex<BTreeMap<u64, SourceFixture>>>,
    }

    struct SourceFixture {
        bytes: Vec<u8>,
        finishes: Arc<AtomicUsize>,
    }

    impl SharedIngress {
        fn new() -> Self {
            Self {
                fixtures: Arc::new(Mutex::new(BTreeMap::new())),
            }
        }

        fn insert(&self, handle: u64, bytes: Vec<u8>) -> Arc<AtomicUsize> {
            let finishes = Arc::new(AtomicUsize::new(0));
            self.fixtures.lock().expect("ingress lock").insert(
                handle,
                SourceFixture {
                    bytes,
                    finishes: finishes.clone(),
                },
            );
            finishes
        }
    }

    impl RequestSourceResolver for SharedIngress {
        type Source = VecSource;

        fn resolve(&self, source_handle: u64) -> Result<Self::Source, BlobError> {
            let fixture = self
                .fixtures
                .lock()
                .map_err(|_| BlobError::Poisoned)?
                .remove(&source_handle)
                .ok_or_else(|| {
                    BlobError::source("blob-source-forbidden", "unknown source handle")
                })?;
            Ok(VecSource {
                bytes: fixture.bytes,
                cursor: 0,
                finishes: fixture.finishes,
            })
        }
    }

    struct VecSource {
        bytes: Vec<u8>,
        cursor: usize,
        finishes: Arc<AtomicUsize>,
    }

    impl ByteSource for VecSource {
        fn read(&mut self, output: &mut [u8]) -> Result<usize, BlobError> {
            let amount = output
                .len()
                .min(self.bytes.len().saturating_sub(self.cursor));
            output[..amount].copy_from_slice(&self.bytes[self.cursor..self.cursor + amount]);
            self.cursor += amount;
            Ok(amount)
        }

        fn finish(&mut self) -> Result<(), BlobError> {
            self.finishes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Clone)]
    struct SharedEgress {
        state: Arc<Mutex<EgressState>>,
    }

    struct EgressState {
        next: u64,
        sources: BTreeMap<u64, MemoryResponseSource>,
    }

    impl SharedEgress {
        fn new() -> Self {
            Self {
                state: Arc::new(Mutex::new(EgressState {
                    next: 100,
                    sources: BTreeMap::new(),
                })),
            }
        }

        fn read_all(&self, handle: u64) -> Vec<u8> {
            let mut state = self.state.lock().expect("egress lock");
            let source = state.sources.get_mut(&handle).expect("registered source");
            let mut output = vec![0_u8; source.declared_length() as usize];
            let mut cursor = 0;
            while cursor < output.len() {
                let read = source.read(&mut output[cursor..]).expect("read source");
                assert_ne!(read, 0, "source ended before declared length");
                cursor += read;
            }
            assert_eq!(source.read(&mut [0_u8; 1]).expect("source eof"), 0);
            source.close().expect("close source");
            output
        }
    }

    impl ResponseSourceRegistrar<MemoryResponseSource> for SharedEgress {
        fn register(&self, source: MemoryResponseSource) -> Result<u64, BlobError> {
            let mut state = self.state.lock().map_err(|_| BlobError::Poisoned)?;
            let handle = state.next;
            state.next = state.next.checked_add(1).ok_or_else(|| {
                BlobError::driver("blob-response-handle-exhausted", "handle overflow")
            })?;
            state.sources.insert(handle, source);
            Ok(handle)
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

    type TestProvider = Provider<InMemoryBlobStore<TestVerifier>, SharedIngress, SharedEgress>;

    fn provider() -> (TestProvider, SharedIngress, SharedEgress) {
        let ingress = SharedIngress::new();
        let egress = SharedEgress::new();
        let provider = Provider::new(
            InMemoryBlobStore::new(TestVerifier, limits()).unwrap(),
            ingress.clone(),
            egress.clone(),
            limits(),
        )
        .unwrap();
        (provider, ingress, egress)
    }

    fn frame(bare: &[u8]) -> Vec<u8> {
        let mut output = MAGIC.to_vec();
        output.extend_from_slice(bare);
        output
    }

    fn bare_vector(values: &[Vec<u8>]) -> Vec<u8> {
        let mut output = vec![VECTOR];
        output.extend_from_slice(&(values.len() as u32).to_be_bytes());
        for value in values {
            output.extend_from_slice(value);
        }
        output
    }

    fn bare_map(entries: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
        let mut entries = entries
            .into_iter()
            .map(|(key, value)| (bare_keyword(key), value))
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let mut output = vec![MAP];
        output.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (key, value) in entries {
            output.extend_from_slice(&key);
            output.extend_from_slice(&value);
        }
        output
    }

    fn arguments(request: Vec<u8>) -> Vec<u8> {
        frame(&bare_vector(&[request]))
    }

    fn request(entries: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
        bare_map(entries)
    }

    fn open_request(staging_key: &str, bytes: &[u8]) -> Vec<u8> {
        request(vec![
            ("expected-digest", bare_string(&digest(bytes).to_string())),
            ("expected-size", bare_i64(bytes.len() as u64).unwrap()),
            ("media-type", bare_string("application/octet-stream")),
            ("operation", bare_string("staging/open")),
            ("protocol", bare_string(REQUEST_PROTOCOL)),
            ("staging-key", bare_string(staging_key)),
        ])
    }

    fn append_request(staging_key: &str, offset: u64, length: u64, handle: u64) -> Vec<u8> {
        request(vec![
            ("length", bare_i64(length).unwrap()),
            ("offset", bare_i64(offset).unwrap()),
            ("operation", bare_string("staging/append-from-source")),
            ("protocol", bare_string(REQUEST_PROTOCOL)),
            ("source-handle", bare_i64(handle).unwrap()),
            ("staging-key", bare_string(staging_key)),
        ])
    }

    fn commit_request(staging_key: &str, bytes: &[u8]) -> Vec<u8> {
        request(vec![
            ("expected-digest", bare_string(&digest(bytes).to_string())),
            ("expected-size", bare_i64(bytes.len() as u64).unwrap()),
            ("operation", bare_string("staging/verify-commit")),
            ("protocol", bare_string(REQUEST_PROTOCOL)),
            ("staging-key", bare_string(staging_key)),
        ])
    }

    fn open_source_request(bytes: &[u8], offset: u64, length: u64) -> Vec<u8> {
        request(vec![
            ("digest", bare_string(&digest(bytes).to_string())),
            ("length", bare_i64(length).unwrap()),
            ("offset", bare_i64(offset).unwrap()),
            ("operation", bare_string("object/open-source")),
            ("protocol", bare_string(REQUEST_PROTOCOL)),
        ])
    }

    fn result_text(result: &[u8], field: &str) -> String {
        let document = Document::parse(result).unwrap();
        document
            .root()
            .map_get(field)
            .unwrap()
            .unwrap()
            .as_text()
            .unwrap()
            .to_owned()
    }

    fn result_i64(result: &[u8], field: &str) -> i64 {
        let document = Document::parse(result).unwrap();
        document
            .root()
            .map_get(field)
            .unwrap()
            .unwrap()
            .as_i64()
            .unwrap()
    }

    fn result_bool(result: &[u8], field: &str) -> bool {
        let document = Document::parse(result).unwrap();
        document
            .root()
            .map_get(field)
            .unwrap()
            .unwrap()
            .as_bool()
            .unwrap()
    }

    #[test]
    fn executes_the_complete_generic_blob_flow() {
        let (provider, ingress, egress) = provider();
        let bytes = b"abcdefgh";

        let opened = provider
            .execute("staging/open", &arguments(open_request("upload.a", bytes)))
            .unwrap();
        assert_eq!(result_text(&opened, "operation"), "staging/open");
        assert_eq!(result_text(&opened, "staging-key"), "upload.a");
        assert!(result_bool(&opened, "opened"));
        assert_eq!(result_i64(&opened, "offset"), 0);

        let finishes = ingress.insert(7, bytes.to_vec());
        let appended = provider
            .execute(
                "staging/append-from-source",
                &arguments(append_request("upload.a", 0, bytes.len() as u64, 7)),
            )
            .unwrap();
        assert!(result_bool(&appended, "consumed"));
        assert_eq!(result_i64(&appended, "offset"), bytes.len() as i64);
        assert_eq!(result_i64(&appended, "length"), bytes.len() as i64);
        assert_eq!(finishes.load(Ordering::SeqCst), 1);

        let committed = provider
            .execute(
                "staging/verify-commit",
                &arguments(commit_request("upload.a", bytes)),
            )
            .unwrap();
        assert!(result_bool(&committed, "verified"));
        assert!(result_bool(&committed, "committed"));
        assert_eq!(result_text(&committed, "digest"), digest(bytes).to_string());
        assert_eq!(result_i64(&committed, "size"), bytes.len() as i64);

        let opened_source = provider
            .execute(
                "object/open-source",
                &arguments(open_source_request(bytes, 2, 4)),
            )
            .unwrap();
        assert_eq!(
            result_text(&opened_source, "operation"),
            "object/open-source"
        );
        assert_eq!(result_i64(&opened_source, "offset"), 2);
        assert_eq!(result_i64(&opened_source, "length"), 4);
        let source_handle = result_i64(&opened_source, "source-handle") as u64;
        assert_eq!(egress.read_all(source_handle), b"cdef");
    }

    #[test]
    fn executes_abort_and_rejects_open_requests() {
        let (provider, _, _) = provider();
        let bytes = b"abc";
        provider
            .execute("staging/open", &arguments(open_request("upload.a", bytes)))
            .unwrap();
        let aborted = provider
            .execute(
                "staging/abort",
                &arguments(request(vec![
                    ("operation", bare_string("staging/abort")),
                    ("protocol", bare_string(REQUEST_PROTOCOL)),
                    ("staging-key", bare_string("upload.a")),
                ])),
            )
            .unwrap();
        assert!(result_bool(&aborted, "aborted"));

        let open = request(vec![
            ("extra", bare_string("forbidden")),
            ("operation", bare_string("staging/abort")),
            ("protocol", bare_string(REQUEST_PROTOCOL)),
            ("staging-key", bare_string("upload.a")),
        ]);
        assert_eq!(
            provider
                .execute("staging/abort", &arguments(open))
                .unwrap_err()
                .code(),
            "blob-request-invalid"
        );
    }

    #[test]
    fn requires_a_scope_bound_source_and_preserves_source_failures() {
        let (provider, ingress, _) = provider();
        let bytes = b"abcd";
        provider
            .execute("staging/open", &arguments(open_request("upload.a", bytes)))
            .unwrap();

        let forbidden = provider
            .execute(
                "staging/append-from-source",
                &arguments(append_request("upload.a", 0, 4, 99)),
            )
            .unwrap_err();
        assert_eq!(forbidden.code(), "blob-source-forbidden");

        ingress.insert(8, b"ab".to_vec());
        let short = provider
            .execute(
                "staging/append-from-source",
                &arguments(append_request("upload.a", 0, 4, 8)),
            )
            .unwrap_err();
        assert_eq!(short.code(), "blob-source-short");

        ingress.insert(9, b"abcde".to_vec());
        let long = provider
            .execute(
                "staging/append-from-source",
                &arguments(append_request("upload.a", 0, 4, 9)),
            )
            .unwrap_err();
        assert_eq!(long.code(), "blob-source-long");
    }

    #[test]
    fn rejects_operation_mismatch_and_zero_source_handle() {
        let (provider, _, _) = provider();
        let abort = request(vec![
            ("operation", bare_string("staging/abort")),
            ("protocol", bare_string(REQUEST_PROTOCOL)),
            ("staging-key", bare_string("upload.a")),
        ]);
        assert_eq!(
            provider
                .execute("staging/open", &arguments(abort))
                .unwrap_err()
                .code(),
            "blob-operation-mismatch"
        );

        let zero = append_request("upload.a", 0, 1, 0);
        assert_eq!(
            provider
                .execute("staging/append-from-source", &arguments(zero))
                .unwrap_err()
                .code(),
            "blob-request-invalid"
        );
    }

    #[test]
    fn rejects_non_vector_and_empty_argument_frames() {
        let (provider, _, _) = provider();
        let request = open_request("upload.a", b"x");
        assert_eq!(
            provider
                .execute("staging/open", &frame(&request))
                .unwrap_err()
                .code(),
            "blob-request-invalid"
        );
        assert_eq!(
            provider
                .execute("staging/open", &frame(&bare_vector(&[])))
                .unwrap_err()
                .code(),
            "blob-request-invalid"
        );
        assert_ne!(NIL, TRUE);
    }
}
