#![forbid(unsafe_code)]

//! Application-neutral replay admission for verified signed-device requests.
//!
//! This crate persists only bounded request identity and a SHA-256 fingerprint
//! of the exact `hoplite-signed-device/2` signing input. Signature bytes,
//! credentials, native handles, paths and application state remain outside the
//! replay ledger.

use hoplite_data_plane_abi::{
    authenticate_application_request, ApplicationAuthenticationError, ApplicationIdentity,
    ApplicationRequestExpectation, SignedDevicePrincipal, SignedDeviceProvider,
    SignedDeviceRequest, VerifiedApplicationRequest, VERIFIED_APPLICATION_REQUEST_PROFILE,
};
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use sha2::{Digest as ShaDigest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

pub const REPLAY_EVIDENCE_PROFILE: &str = "hoplite-replay-evidence/1";
pub const REPLAY_RECEIPT_PROFILE: &str = "hoplite-replay-receipt/1";
pub const SCHEMA_VERSION: i64 = 1;
pub const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const MAX_IDENTITY_BYTES: usize = 256;
const VALIDATION_SIGNATURE: &str = "replay-evidence-signature";

const INITIAL_SCHEMA: &str = r#"
CREATE TABLE signed_request_admissions (
  subject TEXT NOT NULL CHECK (length(subject) BETWEEN 1 AND 256),
  realm TEXT NOT NULL CHECK (realm = 'application'),
  device_id TEXT NOT NULL CHECK (length(device_id) BETWEEN 1 AND 256),
  key_id TEXT NOT NULL CHECK (length(key_id) BETWEEN 1 AND 256),
  application_version TEXT NOT NULL CHECK (
    length(application_version) BETWEEN 1 AND 128
  ),
  publisher TEXT NOT NULL CHECK (length(publisher) BETWEEN 1 AND 128),
  lock_digest TEXT NOT NULL CHECK (
    length(lock_digest) = 71
    AND substr(lock_digest, 1, 7) = 'sha256:'
    AND substr(lock_digest, 8) NOT GLOB '*[^0-9a-f]*'
  ),
  method TEXT NOT NULL CHECK (length(method) BETWEEN 1 AND 32),
  target TEXT NOT NULL CHECK (length(target) BETWEEN 1 AND 8192),
  authority TEXT NOT NULL CHECK (length(authority) BETWEEN 1 AND 255),
  operation TEXT NOT NULL CHECK (length(operation) BETWEEN 1 AND 128),
  application TEXT NOT NULL CHECK (length(application) BETWEEN 1 AND 128),
  namespace TEXT NOT NULL CHECK (length(namespace) BETWEEN 1 AND 128),
  collection TEXT NOT NULL CHECK (length(collection) BETWEEN 1 AND 128),
  content_digest TEXT NOT NULL CHECK (
    length(content_digest) = 71
    AND substr(content_digest, 1, 7) = 'sha256:'
    AND substr(content_digest, 8) NOT GLOB '*[^0-9a-f]*'
  ),
  request_timestamp INTEGER NOT NULL CHECK (request_timestamp > 0),
  nonce TEXT NOT NULL CHECK (length(nonce) BETWEEN 16 AND 256),
  idempotency_key TEXT NOT NULL CHECK (
    length(idempotency_key) BETWEEN 16 AND 256
  ),
  request_fingerprint TEXT NOT NULL CHECK (
    length(request_fingerprint) = 71
    AND substr(request_fingerprint, 1, 7) = 'sha256:'
    AND substr(request_fingerprint, 8) NOT GLOB '*[^0-9a-f]*'
  ),
  admitted_at INTEGER NOT NULL CHECK (admitted_at > 0),
  PRIMARY KEY (
    subject,
    device_id,
    application,
    namespace,
    collection,
    idempotency_key
  ),
  UNIQUE (subject, device_id, nonce)
) STRICT;

CREATE INDEX signed_request_admissions_fingerprint
  ON signed_request_admissions(request_fingerprint);

PRAGMA user_version = 1;
"#;

const SELECT_COLUMNS: &str = r#"
subject,
realm,
device_id,
key_id,
application_version,
publisher,
lock_digest,
method,
target,
authority,
operation,
application,
namespace,
collection,
content_digest,
request_timestamp,
nonce,
idempotency_key,
request_fingerprint,
admitted_at
"#;

/// Lower-case SHA-256 identifier for one exact versioned signing input.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestFingerprint(String);

impl RequestFingerprint {
    pub fn parse(value: impl Into<String>) -> Result<Self, ReplayError> {
        let value = value.into();
        if valid_sha256(&value) {
            Ok(Self(value))
        } else {
            Err(ReplayError::InvalidFingerprint)
        }
    }

    fn from_signing_input(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        let mut value = String::with_capacity(71);
        value.push_str("sha256:");
        encode_hex_into(&digest, &mut value);
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RequestFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl fmt::Debug for RequestFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RequestFingerprint")
            .field(&self.0)
            .finish()
    }
}

/// One proposed durable admission created only from closed verified evidence.
#[derive(Clone, PartialEq, Eq)]
pub struct ReplayCandidate {
    evidence: ReplayEvidence,
}

impl ReplayCandidate {
    pub fn from_verified(
        request: &SignedDeviceRequest<'_>,
        verified: &VerifiedApplicationRequest,
        admitted_at: i64,
    ) -> Result<Self, ReplayError> {
        if admitted_at <= 0 {
            return Err(ReplayError::InvalidAdmittedAt);
        }
        if verified.profile() != VERIFIED_APPLICATION_REQUEST_PROFILE
            || verified.identity().realm != "application"
            || verified.identity().application_id != verified.application()
            || verified.identity().key_id != request.key_id
            || verified.operation() != request.operation
            || verified.application() != request.application
            || verified.namespace() != request.namespace
            || verified.collection() != request.collection
            || verified.content_digest() != request.content_digest
            || verified.timestamp() != request.timestamp
            || verified.nonce() != request.nonce
            || verified.idempotency_key() != request.idempotency_key
        {
            return Err(ReplayError::InvalidVerifiedRequest);
        }

        request
            .validate()
            .map_err(|_| ReplayError::InvalidVerifiedRequest)?;
        let signing_input = request
            .signing_input()
            .map_err(|_| ReplayError::InvalidVerifiedRequest)?;
        let identity = verified.identity();

        let evidence = ReplayEvidence {
            profile: REPLAY_EVIDENCE_PROFILE,
            fingerprint: RequestFingerprint::from_signing_input(signing_input.as_bytes()),
            subject: identity.subject.clone(),
            realm: identity.realm.clone(),
            device_id: identity.device_id.clone(),
            key_id: identity.key_id.clone(),
            application_version: identity.application_version.clone(),
            publisher: identity.publisher.clone(),
            lock_digest: identity.lock_digest.clone(),
            method: request.method.to_owned(),
            target: request.target.to_owned(),
            authority: request.authority.to_owned(),
            operation: request.operation.to_owned(),
            application: request.application.to_owned(),
            namespace: request.namespace.to_owned(),
            collection: request.collection.to_owned(),
            content_digest: request.content_digest.to_owned(),
            request_timestamp: request.timestamp,
            nonce: request.nonce.to_owned(),
            idempotency_key: request.idempotency_key.to_owned(),
            admitted_at,
        };
        evidence.validate()?;
        Ok(Self { evidence })
    }

    pub const fn evidence(&self) -> &ReplayEvidence {
        &self.evidence
    }

    fn into_evidence(self) -> ReplayEvidence {
        self.evidence
    }
}

impl fmt::Debug for ReplayCandidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplayCandidate")
            .field("fingerprint", &self.evidence.fingerprint)
            .field("device_id", &self.evidence.device_id)
            .field("operation", &self.evidence.operation)
            .field("application", &self.evidence.application)
            .field("namespace", &self.evidence.namespace)
            .field("collection", &self.evidence.collection)
            .field("admitted_at", &self.evidence.admitted_at)
            .finish_non_exhaustive()
    }
}

/// Exact durable evidence returned unchanged on idempotent replay.
#[derive(Clone, PartialEq, Eq)]
pub struct ReplayEvidence {
    profile: &'static str,
    fingerprint: RequestFingerprint,
    subject: String,
    realm: String,
    device_id: String,
    key_id: String,
    application_version: String,
    publisher: String,
    lock_digest: String,
    method: String,
    target: String,
    authority: String,
    operation: String,
    application: String,
    namespace: String,
    collection: String,
    content_digest: String,
    request_timestamp: i64,
    nonce: String,
    idempotency_key: String,
    admitted_at: i64,
}

impl ReplayEvidence {
    pub const fn profile(&self) -> &'static str {
        self.profile
    }

    pub const fn fingerprint(&self) -> &RequestFingerprint {
        &self.fingerprint
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn realm(&self) -> &str {
        &self.realm
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn application_version(&self) -> &str {
        &self.application_version
    }

    pub fn publisher(&self) -> &str {
        &self.publisher
    }

    pub fn lock_digest(&self) -> &str {
        &self.lock_digest
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn authority(&self) -> &str {
        &self.authority
    }

    pub fn operation(&self) -> &str {
        &self.operation
    }

    pub fn application(&self) -> &str {
        &self.application
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn collection(&self) -> &str {
        &self.collection
    }

    pub fn content_digest(&self) -> &str {
        &self.content_digest
    }

    pub const fn request_timestamp(&self) -> i64 {
        self.request_timestamp
    }

    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }

    pub const fn admitted_at(&self) -> i64 {
        self.admitted_at
    }

    fn lookup(&self) -> ReplayLookup {
        ReplayLookup {
            subject: self.subject.clone(),
            device_id: self.device_id.clone(),
            application: self.application.clone(),
            namespace: self.namespace.clone(),
            collection: self.collection.clone(),
            idempotency_key: self.idempotency_key.clone(),
        }
    }

    fn nonce_key(&self) -> NonceKey {
        NonceKey {
            subject: self.subject.clone(),
            device_id: self.device_id.clone(),
            nonce: self.nonce.clone(),
        }
    }

    fn same_request(&self, other: &Self) -> bool {
        self.fingerprint == other.fingerprint
            && self.subject == other.subject
            && self.realm == other.realm
            && self.device_id == other.device_id
            && self.key_id == other.key_id
            && self.application_version == other.application_version
            && self.publisher == other.publisher
            && self.lock_digest == other.lock_digest
            && self.method == other.method
            && self.target == other.target
            && self.authority == other.authority
            && self.operation == other.operation
            && self.application == other.application
            && self.namespace == other.namespace
            && self.collection == other.collection
            && self.content_digest == other.content_digest
            && self.request_timestamp == other.request_timestamp
            && self.nonce == other.nonce
            && self.idempotency_key == other.idempotency_key
    }

    fn validate(&self) -> Result<(), ReplayError> {
        if self.profile != REPLAY_EVIDENCE_PROFILE
            || self.realm != "application"
            || self.request_timestamp <= 0
            || self.admitted_at <= 0
            || !bounded_graphic(&self.subject, 1, MAX_IDENTITY_BYTES)
            || !bounded_graphic(&self.device_id, 1, MAX_IDENTITY_BYTES)
            || !bounded_graphic(&self.key_id, 1, MAX_IDENTITY_BYTES)
        {
            return Err(ReplayError::CorruptRecord);
        }

        let principal = SignedDevicePrincipal {
            subject: self.subject.clone(),
            realm: self.realm.clone(),
            device_id: self.device_id.clone(),
            key_id: self.key_id.clone(),
            provider: "replay-validation".to_owned(),
            claims: BTreeMap::from([
                ("application/id".to_owned(), self.application.clone()),
                (
                    "application/version".to_owned(),
                    self.application_version.clone(),
                ),
                ("application/publisher".to_owned(), self.publisher.clone()),
                (
                    "application/lock-digest".to_owned(),
                    self.lock_digest.clone(),
                ),
                ("application/namespace".to_owned(), self.namespace.clone()),
                ("application/collection".to_owned(), self.collection.clone()),
                ("application/operations".to_owned(), self.operation.clone()),
            ]),
        };
        let identity =
            ApplicationIdentity::project(&principal).map_err(|_| ReplayError::CorruptRecord)?;
        if identity.application_id != self.application
            || identity.application_version != self.application_version
            || identity.publisher != self.publisher
            || identity.lock_digest != self.lock_digest
        {
            return Err(ReplayError::CorruptRecord);
        }

        let request = SignedDeviceRequest {
            method: &self.method,
            target: &self.target,
            authority: &self.authority,
            content_digest: &self.content_digest,
            operation: &self.operation,
            application: &self.application,
            namespace: &self.namespace,
            collection: &self.collection,
            timestamp: self.request_timestamp,
            nonce: &self.nonce,
            idempotency_key: &self.idempotency_key,
            key_id: &self.key_id,
            signature: VALIDATION_SIGNATURE,
        };
        request.validate().map_err(|_| ReplayError::CorruptRecord)?;
        let signing_input = request
            .signing_input()
            .map_err(|_| ReplayError::CorruptRecord)?;
        let expected = RequestFingerprint::from_signing_input(signing_input.as_bytes());
        if expected != self.fingerprint {
            return Err(ReplayError::CorruptRecord);
        }
        Ok(())
    }
}

impl fmt::Debug for ReplayEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplayEvidence")
            .field("profile", &self.profile)
            .field("fingerprint", &self.fingerprint)
            .field("device_id", &self.device_id)
            .field("key_id", &self.key_id)
            .field("operation", &self.operation)
            .field("application", &self.application)
            .field("namespace", &self.namespace)
            .field("collection", &self.collection)
            .field("content_digest", &self.content_digest)
            .field("request_timestamp", &self.request_timestamp)
            .field("admitted_at", &self.admitted_at)
            .finish_non_exhaustive()
    }
}

/// Scope used to recover prior durable evidence after a lost completion.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReplayLookup {
    subject: String,
    device_id: String,
    application: String,
    namespace: String,
    collection: String,
    idempotency_key: String,
}

impl ReplayLookup {
    pub fn from_verified(verified: &VerifiedApplicationRequest) -> Self {
        Self {
            subject: verified.identity().subject.clone(),
            device_id: verified.identity().device_id.clone(),
            application: verified.application().to_owned(),
            namespace: verified.namespace().to_owned(),
            collection: verified.collection().to_owned(),
            idempotency_key: verified.idempotency_key().to_owned(),
        }
    }

    pub fn from_evidence(evidence: &ReplayEvidence) -> Self {
        evidence.lookup()
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn application(&self) -> &str {
        &self.application
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn collection(&self) -> &str {
        &self.collection
    }

    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }
}

impl fmt::Debug for ReplayLookup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplayLookup")
            .field("device_id", &self.device_id)
            .field("application", &self.application)
            .field("namespace", &self.namespace)
            .field("collection", &self.collection)
            .field("idempotency_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NonceKey {
    subject: String,
    device_id: String,
    nonce: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayStatus {
    Applied,
    Replayed,
}

impl ReplayStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Replayed => "replayed",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ReplayReceipt {
    profile: &'static str,
    status: ReplayStatus,
    evidence: ReplayEvidence,
}

impl ReplayReceipt {
    fn new(status: ReplayStatus, evidence: ReplayEvidence) -> Self {
        Self {
            profile: REPLAY_RECEIPT_PROFILE,
            status,
            evidence,
        }
    }

    pub const fn profile(&self) -> &'static str {
        self.profile
    }

    pub const fn status(&self) -> ReplayStatus {
        self.status
    }

    pub const fn evidence(&self) -> &ReplayEvidence {
        &self.evidence
    }
}

impl fmt::Debug for ReplayReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplayReceipt")
            .field("profile", &self.profile)
            .field("status", &self.status)
            .field("evidence", &self.evidence)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayError {
    InvalidVerifiedRequest,
    InvalidAdmittedAt,
    InvalidFingerprint,
    IdempotencyCollision,
    NonceReused,
    CorruptRecord,
    UnsupportedSchema,
    Driver,
}

impl ReplayError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidVerifiedRequest => "hoplite.replay/verified-request-invalid",
            Self::InvalidAdmittedAt => "hoplite.replay/admitted-at-invalid",
            Self::InvalidFingerprint => "hoplite.replay/fingerprint-invalid",
            Self::IdempotencyCollision => "hoplite.replay/idempotency-collision",
            Self::NonceReused => "hoplite.replay/nonce-reused",
            Self::CorruptRecord => "hoplite.replay/record-corrupt",
            Self::UnsupportedSchema => "hoplite.replay/schema-unsupported",
            Self::Driver => "hoplite.replay/driver-failed",
        }
    }
}

impl fmt::Display for ReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidVerifiedRequest => "verified application request is invalid",
            Self::InvalidAdmittedAt => "replay admission time is invalid",
            Self::InvalidFingerprint => "request fingerprint is invalid",
            Self::IdempotencyCollision => "idempotency key is bound to another request",
            Self::NonceReused => "signed-device nonce was already consumed",
            Self::CorruptRecord => "durable replay evidence is invalid",
            Self::UnsupportedSchema => "durable replay schema is unsupported",
            Self::Driver => "durable replay store failed",
        })
    }
}

impl std::error::Error for ReplayError {}

pub trait ReplayStore: Send + Sync {
    fn admit(&self, candidate: ReplayCandidate) -> Result<ReplayReceipt, ReplayError>;

    fn lookup(&self, lookup: &ReplayLookup) -> Result<Option<ReplayEvidence>, ReplayError>;
}

#[derive(Default)]
struct MemoryState {
    by_idempotency: BTreeMap<ReplayLookup, ReplayEvidence>,
    by_nonce: BTreeMap<NonceKey, RequestFingerprint>,
}

/// Deterministic in-memory conformance implementation.
#[derive(Default)]
pub struct MemoryReplayStore {
    state: Mutex<MemoryState>,
}

impl fmt::Debug for MemoryReplayStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryReplayStore")
            .finish_non_exhaustive()
    }
}

impl ReplayStore for MemoryReplayStore {
    fn admit(&self, candidate: ReplayCandidate) -> Result<ReplayReceipt, ReplayError> {
        candidate.evidence.validate()?;
        let lookup = candidate.evidence.lookup();
        let nonce_key = candidate.evidence.nonce_key();
        let mut state = self.state.lock().map_err(|_| ReplayError::Driver)?;

        if let Some(existing) = state.by_idempotency.get(&lookup) {
            if existing.same_request(candidate.evidence()) {
                return Ok(ReplayReceipt::new(ReplayStatus::Replayed, existing.clone()));
            }
            return Err(ReplayError::IdempotencyCollision);
        }
        if state.by_nonce.contains_key(&nonce_key) {
            return Err(ReplayError::NonceReused);
        }

        let evidence = candidate.into_evidence();
        state
            .by_nonce
            .insert(nonce_key, evidence.fingerprint.clone());
        state.by_idempotency.insert(lookup, evidence.clone());
        Ok(ReplayReceipt::new(ReplayStatus::Applied, evidence))
    }

    fn lookup(&self, lookup: &ReplayLookup) -> Result<Option<ReplayEvidence>, ReplayError> {
        let state = self.state.lock().map_err(|_| ReplayError::Driver)?;
        Ok(state.by_idempotency.get(lookup).cloned())
    }
}

/// Restart-safe SQLite replay ledger.
pub struct SqliteReplayStore {
    path: PathBuf,
    connection: Mutex<Connection>,
}

impl SqliteReplayStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ReplayError> {
        let path = path.as_ref().to_path_buf();
        let in_memory = path == Path::new(":memory:");
        if !in_memory {
            if let Some(parent) = path.parent().filter(|value| !value.as_os_str().is_empty()) {
                fs::create_dir_all(parent).map_err(driver)?;
                set_private_directory(parent)?;
            }
        }

        let connection = Connection::open(&path).map_err(driver)?;
        configure_connection(&connection)?;
        if !in_memory {
            set_private_file(&path)?;
        }

        let store = Self {
            path,
            connection: Mutex::new(connection),
        };
        store.ensure_schema()?;
        store.verify()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, ReplayError> {
        Self::open(":memory:")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn verify(&self) -> Result<(), ReplayError> {
        let connection = self.lock_connection()?;
        verify_connection(&connection)
    }

    fn ensure_schema(&self) -> Result<(), ReplayError> {
        let connection = self.lock_connection()?;
        match schema_version(&connection)? {
            0 => {
                let table_count: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master \
                         WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(driver)?;
                if table_count != 0 {
                    return Err(ReplayError::UnsupportedSchema);
                }
                connection.execute_batch(INITIAL_SCHEMA).map_err(driver)
            }
            SCHEMA_VERSION => Ok(()),
            _ => Err(ReplayError::UnsupportedSchema),
        }
    }

    fn lock_connection(&self) -> Result<MutexGuard<'_, Connection>, ReplayError> {
        self.connection.lock().map_err(|_| ReplayError::Driver)
    }
}

impl fmt::Debug for SqliteReplayStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteReplayStore")
            .field("persistent", &(self.path != Path::new(":memory:")))
            .field("schema_version", &SCHEMA_VERSION)
            .finish_non_exhaustive()
    }
}

impl ReplayStore for SqliteReplayStore {
    fn admit(&self, candidate: ReplayCandidate) -> Result<ReplayReceipt, ReplayError> {
        candidate.evidence.validate()?;
        let lookup = candidate.evidence.lookup();
        let nonce_key = candidate.evidence.nonce_key();
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(driver)?;

        if let Some(existing) = load_by_lookup(&transaction, &lookup)? {
            if existing.same_request(candidate.evidence()) {
                transaction.commit().map_err(driver)?;
                return Ok(ReplayReceipt::new(ReplayStatus::Replayed, existing));
            }
            return Err(ReplayError::IdempotencyCollision);
        }
        if load_nonce_fingerprint(&transaction, &nonce_key)?.is_some() {
            return Err(ReplayError::NonceReused);
        }

        let evidence = candidate.into_evidence();
        transaction
            .execute(
                "INSERT INTO signed_request_admissions (
                   subject,
                   realm,
                   device_id,
                   key_id,
                   application_version,
                   publisher,
                   lock_digest,
                   method,
                   target,
                   authority,
                   operation,
                   application,
                   namespace,
                   collection,
                   content_digest,
                   request_timestamp,
                   nonce,
                   idempotency_key,
                   request_fingerprint,
                   admitted_at
                 ) VALUES (
                   ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                   ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20
                 )",
                params![
                    &evidence.subject,
                    &evidence.realm,
                    &evidence.device_id,
                    &evidence.key_id,
                    &evidence.application_version,
                    &evidence.publisher,
                    &evidence.lock_digest,
                    &evidence.method,
                    &evidence.target,
                    &evidence.authority,
                    &evidence.operation,
                    &evidence.application,
                    &evidence.namespace,
                    &evidence.collection,
                    &evidence.content_digest,
                    evidence.request_timestamp,
                    &evidence.nonce,
                    &evidence.idempotency_key,
                    evidence.fingerprint.as_str(),
                    evidence.admitted_at,
                ],
            )
            .map_err(driver)?;
        transaction.commit().map_err(driver)?;
        Ok(ReplayReceipt::new(ReplayStatus::Applied, evidence))
    }

    fn lookup(&self, lookup: &ReplayLookup) -> Result<Option<ReplayEvidence>, ReplayError> {
        let connection = self.lock_connection()?;
        load_by_lookup(&connection, lookup)
    }
}

/// Verified application request together with its atomic replay receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedApplicationRequest {
    verified: VerifiedApplicationRequest,
    replay: ReplayReceipt,
}

impl AdmittedApplicationRequest {
    pub const fn verified(&self) -> &VerifiedApplicationRequest {
        &self.verified
    }

    pub const fn replay(&self) -> &ReplayReceipt {
        &self.replay
    }
}

#[derive(Debug)]
pub enum ApplicationIngressError {
    Authentication(ApplicationAuthenticationError),
    Replay(ReplayError),
}

impl ApplicationIngressError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Authentication(error) => error.code(),
            Self::Replay(error) => error.code(),
        }
    }
}

impl fmt::Display for ApplicationIngressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Authentication(_) => {
                formatter.write_str("signed application authentication failed")
            }
            Self::Replay(_) => formatter.write_str("signed application replay admission failed"),
        }
    }
}

impl std::error::Error for ApplicationIngressError {}

/// Authenticate first, then atomically consume nonce/idempotency authority.
pub fn authenticate_and_admit_application_request<P, S>(
    provider: &mut P,
    store: &S,
    request: &SignedDeviceRequest<'_>,
    expectation: &ApplicationRequestExpectation<'_>,
    admitted_at: i64,
) -> Result<AdmittedApplicationRequest, ApplicationIngressError>
where
    P: SignedDeviceProvider,
    S: ReplayStore,
{
    let verified = authenticate_application_request(provider, request, expectation)
        .map_err(ApplicationIngressError::Authentication)?;
    let candidate = ReplayCandidate::from_verified(request, &verified, admitted_at)
        .map_err(ApplicationIngressError::Replay)?;
    let replay = store
        .admit(candidate)
        .map_err(ApplicationIngressError::Replay)?;
    Ok(AdmittedApplicationRequest { verified, replay })
}

#[derive(Clone)]
struct StoredRow {
    subject: String,
    realm: String,
    device_id: String,
    key_id: String,
    application_version: String,
    publisher: String,
    lock_digest: String,
    method: String,
    target: String,
    authority: String,
    operation: String,
    application: String,
    namespace: String,
    collection: String,
    content_digest: String,
    request_timestamp: i64,
    nonce: String,
    idempotency_key: String,
    request_fingerprint: String,
    admitted_at: i64,
}

impl StoredRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            subject: row.get(0)?,
            realm: row.get(1)?,
            device_id: row.get(2)?,
            key_id: row.get(3)?,
            application_version: row.get(4)?,
            publisher: row.get(5)?,
            lock_digest: row.get(6)?,
            method: row.get(7)?,
            target: row.get(8)?,
            authority: row.get(9)?,
            operation: row.get(10)?,
            application: row.get(11)?,
            namespace: row.get(12)?,
            collection: row.get(13)?,
            content_digest: row.get(14)?,
            request_timestamp: row.get(15)?,
            nonce: row.get(16)?,
            idempotency_key: row.get(17)?,
            request_fingerprint: row.get(18)?,
            admitted_at: row.get(19)?,
        })
    }

    fn into_evidence(self) -> Result<ReplayEvidence, ReplayError> {
        let evidence = ReplayEvidence {
            profile: REPLAY_EVIDENCE_PROFILE,
            fingerprint: RequestFingerprint::parse(self.request_fingerprint)
                .map_err(|_| ReplayError::CorruptRecord)?,
            subject: self.subject,
            realm: self.realm,
            device_id: self.device_id,
            key_id: self.key_id,
            application_version: self.application_version,
            publisher: self.publisher,
            lock_digest: self.lock_digest,
            method: self.method,
            target: self.target,
            authority: self.authority,
            operation: self.operation,
            application: self.application,
            namespace: self.namespace,
            collection: self.collection,
            content_digest: self.content_digest,
            request_timestamp: self.request_timestamp,
            nonce: self.nonce,
            idempotency_key: self.idempotency_key,
            admitted_at: self.admitted_at,
        };
        evidence.validate()?;
        Ok(evidence)
    }
}

fn configure_connection(connection: &Connection) -> Result<(), ReplayError> {
    connection
        .busy_timeout(DEFAULT_BUSY_TIMEOUT)
        .map_err(driver)?;
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA temp_store = MEMORY;",
        )
        .map_err(driver)
}

fn schema_version(connection: &Connection) -> Result<i64, ReplayError> {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(driver)
}

fn verify_connection(connection: &Connection) -> Result<(), ReplayError> {
    if schema_version(connection)? != SCHEMA_VERSION {
        return Err(ReplayError::UnsupportedSchema);
    }
    let quick_check: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(driver)?;
    if quick_check != "ok" {
        return Err(ReplayError::CorruptRecord);
    }

    let query = format!(
        "SELECT {SELECT_COLUMNS}
         FROM signed_request_admissions
         ORDER BY subject, device_id, application, namespace, collection, idempotency_key"
    );
    let mut statement = connection.prepare(&query).map_err(driver)?;
    let rows = statement
        .query_map([], StoredRow::from_row)
        .map_err(driver)?;
    for row in rows {
        row.map_err(driver)?.into_evidence()?;
    }
    Ok(())
}

fn load_by_lookup(
    connection: &Connection,
    lookup: &ReplayLookup,
) -> Result<Option<ReplayEvidence>, ReplayError> {
    let query = format!(
        "SELECT {SELECT_COLUMNS}
         FROM signed_request_admissions
         WHERE subject = ?1
           AND device_id = ?2
           AND application = ?3
           AND namespace = ?4
           AND collection = ?5
           AND idempotency_key = ?6"
    );
    let row = connection
        .query_row(
            &query,
            params![
                &lookup.subject,
                &lookup.device_id,
                &lookup.application,
                &lookup.namespace,
                &lookup.collection,
                &lookup.idempotency_key,
            ],
            StoredRow::from_row,
        )
        .optional()
        .map_err(driver)?;
    row.map(StoredRow::into_evidence).transpose()
}

fn load_nonce_fingerprint(
    connection: &Connection,
    nonce: &NonceKey,
) -> Result<Option<RequestFingerprint>, ReplayError> {
    let value = connection
        .query_row(
            "SELECT request_fingerprint
             FROM signed_request_admissions
             WHERE subject = ?1
               AND device_id = ?2
               AND nonce = ?3",
            params![&nonce.subject, &nonce.device_id, &nonce.nonce],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(driver)?;
    value.map(RequestFingerprint::parse).transpose()
}

fn bounded_graphic(value: &str, minimum: usize, maximum: usize) -> bool {
    (minimum..=maximum).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

fn encode_hex_into(bytes: &[u8], output: &mut String) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
}

fn driver<T>(_error: T) -> ReplayError {
    ReplayError::Driver
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), ReplayError> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(driver)
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), ReplayError> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<(), ReplayError> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(driver)
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<(), ReplayError> {
    Ok(())
}
