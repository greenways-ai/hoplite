#![forbid(unsafe_code)]

//! Canonical `hara.store` request/result adapter for opaque value-store drivers.
//!
//! The adapter understands only the application-neutral storage protocol. It
//! preserves nested value and receipt frames exactly and delegates mechanical
//! persistence to an `OpaqueValueStore`. Application records remain opaque.

use hoplite_provider_hta::{Document, Error as HtaError, Kind, Node};
use hoplite_value_store::{
    ApplyStatus, CanonicalValue, CommitReceipt, CompareAndSwap, Digest, DigestVerifier,
    OpaqueReceipt, OpaqueValueStore, Snapshot, StoreError, StoreLimits, REQUEST_PROTOCOL,
    RESULT_PROTOCOL,
};
use std::fmt;

const MAGIC: &[u8; 4] = b"HTA1";
const NIL: u8 = 0;
const I64: u8 = 3;
const STRING: u8 = 4;
const KEYWORD: u8 = 6;
const VECTOR: u8 = 9;
const MAP: u8 = 11;

const LOAD_FIELDS: &[&str] = &["operation", "protocol"];
const INITIALIZE_FIELDS: &[&str] = &[
    "operation",
    "protocol",
    "revision",
    "value",
    "value-digest",
];
const COMPARE_AND_SWAP_FIELDS: &[&str] = &[
    "expected-revision",
    "operation",
    "protocol",
    "receipt",
    "receipt-key",
    "revision",
    "value",
    "value-digest",
];
const RECEIPT_FIELDS: &[&str] = &["operation", "protocol", "receipt-key"];

#[derive(Debug)]
pub enum Error {
    Hta(HtaError),
    InvalidRequest(&'static str),
    OperationMismatch {
        call: String,
        request: String,
    },
    Store(StoreError),
}

impl Error {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Hta(_) => "store-request-hta",
            Self::InvalidRequest(_) => "store-request-invalid",
            Self::OperationMismatch { .. } => "store-operation-mismatch",
            Self::Store(error) => match error {
                StoreError::InvalidLimits(_) => "store-limits-invalid",
                StoreError::InvalidDigest => "store-digest-invalid",
                StoreError::EmptySpan(_) => "store-span-empty",
                StoreError::SpanLimitExceeded { .. } => "store-span-limit",
                StoreError::DigestMismatch { .. } => "store-digest-mismatch",
                StoreError::RevisionOutOfRange { .. }
                | StoreError::InvalidRevisionStep { .. }
                | StoreError::InvalidReceiptRevision { .. } => "store-revision-invalid",
                StoreError::AlreadyInitialized { .. } => "store-already-initialized",
                StoreError::Uninitialized => "store-uninitialized",
                StoreError::StaleRevision { .. } => "store-stale-revision",
                StoreError::ReceiptCollision { .. } => "store-receipt-collision",
                StoreError::FaultAlreadyPending { .. } | StoreError::InjectedFault { .. } => {
                    "store-fault"
                }
                StoreError::Driver { code, .. } => code,
                StoreError::Poisoned => "store-poisoned",
            },
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hta(error) => write!(formatter, "invalid provider HTA: {error}"),
            Self::InvalidRequest(message) => write!(formatter, "invalid hara.store request: {message}"),
            Self::OperationMismatch { call, request } => write!(
                formatter,
                "host operation {call:?} does not match request operation {request:?}"
            ),
            Self::Store(error) => write!(formatter, "value-store error: {error}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<HtaError> for Error {
    fn from(error: HtaError) -> Self {
        Self::Hta(error)
    }
}

impl From<StoreError> for Error {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

pub struct Provider<S, V> {
    store: S,
    verifier: V,
    limits: StoreLimits,
}

impl<S, V> Provider<S, V>
where
    S: OpaqueValueStore,
    V: DigestVerifier,
{
    pub fn new(store: S, verifier: V, limits: StoreLimits) -> Result<Self, Error> {
        Ok(Self {
            store,
            verifier,
            limits: limits.validate()?,
        })
    }

    pub const fn store(&self) -> &S {
        &self.store
    }

    pub const fn limits(&self) -> StoreLimits {
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
            "load" => {
                exact_fields(request, LOAD_FIELDS)?;
                self.load()
            }
            "initialize" => {
                exact_fields(request, INITIALIZE_FIELDS)?;
                self.initialize(request)
            }
            "compare-and-swap" => {
                exact_fields(request, COMPARE_AND_SWAP_FIELDS)?;
                self.compare_and_swap(request)
            }
            "receipt" => {
                exact_fields(request, RECEIPT_FIELDS)?;
                self.receipt(request)
            }
            _ => Err(Error::InvalidRequest("operation is not supported")),
        }
    }

    fn load(&self) -> Result<Vec<u8>, Error> {
        match self.store.load()? {
            Some(snapshot) => snapshot_result("load", &snapshot),
            None => Ok(nil_frame()),
        }
    }

    fn initialize(&self, request: Node<'_, '_>) -> Result<Vec<u8>, Error> {
        let snapshot = Snapshot::new(
            request_revision(request, "revision")?,
            request_value(request)?,
        )?;
        let snapshot = self.store.initialize(snapshot)?;
        snapshot_result("initialize", &snapshot)
    }

    fn compare_and_swap(&self, request: Node<'_, '_>) -> Result<Vec<u8>, Error> {
        let value = request_value(request)?;
        let receipt = OpaqueReceipt::new(
            request.require("receipt")?.standalone_frame(),
            self.limits,
        )?;
        let compare = CompareAndSwap::new(
            request_revision(request, "expected-revision")?,
            request_revision(request, "revision")?,
            value,
            request_digest(request, "receipt-key")?,
            receipt,
        )?;
        let receipt = self.store.compare_and_swap(compare)?;
        receipt_result("compare-and-swap", &receipt)
    }

    fn receipt(&self, request: Node<'_, '_>) -> Result<Vec<u8>, Error> {
        let key = request_digest(request, "receipt-key")?;
        match self.store.receipt(key)? {
            Some(receipt) => receipt_result("receipt", &receipt),
            None => Ok(nil_frame()),
        }
    }

    fn verified_value(&self, bytes: Vec<u8>, digest: Digest) -> Result<CanonicalValue, Error> {
        Ok(CanonicalValue::verify(
            bytes,
            digest,
            &self.verifier,
            self.limits,
        )?)
    }
}

impl<S, V> Provider<S, V>
where
    S: OpaqueValueStore,
    V: DigestVerifier,
{
    fn value_from_request(&self, request: Node<'_, '_>) -> Result<CanonicalValue, Error> {
        let bytes = request.require("value")?.standalone_frame();
        let digest = request_digest(request, "value-digest")?;
        self.verified_value(bytes, digest)
    }
}

fn request_value<S, V>(request: Node<'_, '_>) -> Result<CanonicalValue, Error>
where
    S: OpaqueValueStore,
    V: DigestVerifier,
{
    let _ = std::marker::PhantomData::<(S, V)>;
    Err(Error::InvalidRequest(
        "internal request value helper was not bound to a provider",
    ))
}

fn exact_fields(request: Node<'_, '_>, expected: &[&str]) -> Result<(), Error> {
    if request.kind() != Kind::Map || request.len()? != expected.len() {
        return Err(Error::InvalidRequest("request fields are not exact"));
    }
    let mut seen = Vec::with_capacity(expected.len());
    for index in 0..request.len()? {
        let (key, _) = request.pair(index)?;
        if !matches!(key.kind(), Kind::String | Kind::Keyword) {
            return Err(Error::InvalidRequest("request keys must be strings or keywords"));
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
        return Err(Error::InvalidRequest("protocol fields must be strings"));
    }
    Ok(value.as_text()?)
}

fn request_revision(request: Node<'_, '_>, name: &str) -> Result<u64, Error> {
    let value = request.require(name)?.as_i64()?;
    u64::try_from(value).map_err(|_| Error::InvalidRequest("revision must be non-negative"))
}

fn request_digest(request: Node<'_, '_>, name: &str) -> Result<Digest, Error> {
    let value = request.require(name)?;
    if value.kind() != Kind::String {
        return Err(Error::InvalidRequest("digest fields must be strings"));
    }
    Ok(Digest::parse(value.as_text()?)?)
}

fn snapshot_result(operation: &str, snapshot: &Snapshot) -> Result<Vec<u8>, Error> {
    result_map(vec![
        ("operation", bare_string(operation)),
        ("protocol", bare_string(RESULT_PROTOCOL)),
        ("revision", bare_i64(snapshot.revision())?),
        ("value", bare_from_frame(snapshot.value().bytes())?),
        ("value-digest", bare_string(&snapshot.value().digest().to_string())),
    ])
}

fn receipt_result(operation: &str, receipt: &CommitReceipt) -> Result<Vec<u8>, Error> {
    let status = if operation == "receipt" {
        ApplyStatus::Replayed
    } else {
        receipt.status()
    };
    result_map(vec![
        ("operation", bare_string(operation)),
        ("protocol", bare_string(RESULT_PROTOCOL)),
        ("receipt", bare_from_frame(receipt.receipt().bytes())?),
        ("receipt-key", bare_string(&receipt.receipt_key().to_string())),
        ("revision", bare_i64(receipt.revision())?),
        ("status", bare_string(status.name())),
    ])
}

fn nil_frame() -> Vec<u8> {
    let mut output = MAGIC.to_vec();
    output.push(NIL);
    output
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

fn bare_i64(value: u64) -> Result<Vec<u8>, Error> {
    let value = i64::try_from(value)
        .map_err(|_| Error::InvalidRequest("revision exceeds signed 64-bit range"))?;
    let mut output = vec![I64];
    output.extend_from_slice(&value.to_be_bytes());
    Ok(output)
}

fn bare_from_frame(frame: &[u8]) -> Result<Vec<u8>, Error> {
    if !frame.starts_with(MAGIC) || frame.len() <= MAGIC.len() {
        return Err(Error::InvalidRequest("opaque value is not a complete HTA1 frame"));
    }
    Document::parse(frame)?;
    Ok(frame[MAGIC.len()..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoplite_value_store::InMemoryStore;

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

    fn digest(bytes: &[u8]) -> String {
        Digest::from_bytes(TestVerifier.sha256(bytes)).to_string()
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

    fn bare_map(mut entries: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
        let mut entries = entries
            .drain(..)
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

    fn value(revision: i64, label: &str) -> Vec<u8> {
        frame(&bare_map(vec![
            ("label", bare_string(label)),
            ("metadata-revision", {
                let mut bytes = vec![I64];
                bytes.extend_from_slice(&revision.to_be_bytes());
                bytes
            }),
        ]))
    }

    fn provider() -> Provider<InMemoryStore, TestVerifier> {
        Provider::new(
            InMemoryStore::new(),
            TestVerifier,
            StoreLimits::default(),
        )
        .unwrap()
    }

    fn initialize_request(value: &[u8], revision: i64) -> Vec<u8> {
        request(vec![
            ("operation", bare_string("initialize")),
            ("protocol", bare_string(REQUEST_PROTOCOL)),
            ("revision", {
                let mut bytes = vec![I64];
                bytes.extend_from_slice(&revision.to_be_bytes());
                bytes
            }),
            ("value", value[MAGIC.len()..].to_vec()),
            ("value-digest", bare_string(&digest(value))),
        ])
    }

    #[test]
    fn initializes_loads_applies_and_replays_exact_opaque_frames() {
        let provider = provider();
        let initial = value(0, "initial");
        let initialized = provider
            .execute("initialize", &arguments(initialize_request(&initial, 0)))
            .unwrap();
        let result = Document::parse(&initialized).unwrap().root();
        assert_eq!(result.map_get("operation").unwrap().unwrap().as_text().unwrap(), "initialize");
        assert_eq!(result.map_get("value").unwrap().unwrap().standalone_frame(), initial);

        let loaded = provider
            .execute(
                "load",
                &arguments(request(vec![
                    ("operation", bare_string("load")),
                    ("protocol", bare_string(REQUEST_PROTOCOL)),
                ])),
            )
            .unwrap();
        assert_eq!(
            Document::parse(&loaded)
                .unwrap()
                .root()
                .map_get("value")
                .unwrap()
                .unwrap()
                .standalone_frame(),
            initial
        );

        let next = value(1, "next");
        let receipt = frame(&bare_vector(&[bare_string("opaque"), bare_i64(1).unwrap()]));
        let receipt_key = digest(b"receipt-key");
        let compare = request(vec![
            ("expected-revision", bare_i64(0).unwrap()),
            ("operation", bare_string("compare-and-swap")),
            ("protocol", bare_string(REQUEST_PROTOCOL)),
            ("receipt", receipt[MAGIC.len()..].to_vec()),
            ("receipt-key", bare_string(&receipt_key)),
            ("revision", bare_i64(1).unwrap()),
            ("value", next[MAGIC.len()..].to_vec()),
            ("value-digest", bare_string(&digest(&next))),
        ]);
        let applied = provider
            .execute("compare-and-swap", &arguments(compare.clone()))
            .unwrap();
        let applied = Document::parse(&applied).unwrap().root();
        assert_eq!(applied.map_get("status").unwrap().unwrap().as_text().unwrap(), "applied");
        assert_eq!(applied.map_get("receipt").unwrap().unwrap().standalone_frame(), receipt);

        let replayed = provider
            .execute("compare-and-swap", &arguments(compare))
            .unwrap();
        assert_eq!(
            Document::parse(&replayed)
                .unwrap()
                .root()
                .map_get("status")
                .unwrap()
                .unwrap()
                .as_text()
                .unwrap(),
            "replayed"
        );

        let receipt_lookup = provider
            .execute(
                "receipt",
                &arguments(request(vec![
                    ("operation", bare_string("receipt")),
                    ("protocol", bare_string(REQUEST_PROTOCOL)),
                    ("receipt-key", bare_string(&receipt_key)),
                ])),
            )
            .unwrap();
        let receipt_lookup = Document::parse(&receipt_lookup).unwrap().root();
        assert_eq!(receipt_lookup.map_get("status").unwrap().unwrap().as_text().unwrap(), "replayed");
        assert_eq!(receipt_lookup.map_get("receipt").unwrap().unwrap().standalone_frame(), receipt);
    }

    #[test]
    fn returns_nil_for_absent_load_and_receipt() {
        let provider = provider();
        let load = provider
            .execute(
                "load",
                &arguments(request(vec![
                    ("operation", bare_string("load")),
                    ("protocol", bare_string(REQUEST_PROTOCOL)),
                ])),
            )
            .unwrap();
        assert_eq!(load, nil_frame());

        let receipt = provider
            .execute(
                "receipt",
                &arguments(request(vec![
                    ("operation", bare_string("receipt")),
                    ("protocol", bare_string(REQUEST_PROTOCOL)),
                    ("receipt-key", bare_string(&digest(b"missing"))),
                ])),
            )
            .unwrap();
        assert_eq!(receipt, nil_frame());
    }

    #[test]
    fn rejects_open_fields_operation_mismatch_and_digest_mismatch() {
        let provider = provider();
        let open = request(vec![
            ("extra", bare_string("no")),
            ("operation", bare_string("load")),
            ("protocol", bare_string(REQUEST_PROTOCOL)),
        ]);
        assert!(matches!(
            provider.execute("load", &arguments(open)),
            Err(Error::InvalidRequest(_))
        ));

        let load = request(vec![
            ("operation", bare_string("load")),
            ("protocol", bare_string(REQUEST_PROTOCOL)),
        ]);
        assert!(matches!(
            provider.execute("receipt", &arguments(load)),
            Err(Error::OperationMismatch { .. })
        ));

        let initial = value(0, "initial");
        let invalid = request(vec![
            ("operation", bare_string("initialize")),
            ("protocol", bare_string(REQUEST_PROTOCOL)),
            ("revision", bare_i64(0).unwrap()),
            ("value", initial[MAGIC.len()..].to_vec()),
            ("value-digest", bare_string(&digest(b"other"))),
        ]);
        let error = provider
            .execute("initialize", &arguments(invalid))
            .unwrap_err();
        assert_eq!(error.code(), "store-digest-mismatch");
    }
}
