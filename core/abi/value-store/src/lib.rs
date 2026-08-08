#![forbid(unsafe_code)]

//! Dependency-free opaque canonical-value storage contracts for Hoplite.
//!
//! This crate owns mechanical persistence rules only: bounded canonical byte
//! spans, digest verification, initialization, revision compare-and-swap,
//! atomic value-and-receipt publication, and exact replay. Application state,
//! authorization, transaction plans, receipt meaning, and recovery policy stay
//! above this boundary in Hara.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::sync::{Mutex, MutexGuard};

pub const SERVICE: &str = "hara.store";
pub const REQUEST_PROTOCOL: &str = "hara.store-request/1";
pub const RESULT_PROTOCOL: &str = "hara.store-result/1";
pub const MAX_REVISION: u64 = i64::MAX as u64;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest([u8; 32]);

impl Digest {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn parse(value: &str) -> Result<Self, StoreError> {
        let hex = value
            .strip_prefix("sha256:")
            .ok_or(StoreError::InvalidDigest)?;
        if hex.len() != 64 {
            return Err(StoreError::InvalidDigest);
        }

        let mut bytes = [0_u8; 32];
        let encoded = hex.as_bytes();
        for index in 0..32 {
            let high = decode_lower_hex(encoded[index * 2])?;
            let low = decode_lower_hex(encoded[index * 2 + 1])?;
            bytes[index] = (high << 4) | low;
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
    type Err = StoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

fn decode_lower_hex(byte: u8) -> Result<u8, StoreError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(StoreError::InvalidDigest),
    }
}

pub trait DigestVerifier {
    fn sha256(&self, canonical_bytes: &[u8]) -> [u8; 32];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreLimits {
    pub max_value_bytes: usize,
    pub max_receipt_bytes: usize,
}

impl StoreLimits {
    pub const fn new(max_value_bytes: usize, max_receipt_bytes: usize) -> Self {
        Self {
            max_value_bytes,
            max_receipt_bytes,
        }
    }

    pub fn validate(self) -> Result<Self, StoreError> {
        if self.max_value_bytes == 0 {
            return Err(StoreError::InvalidLimits(
                "max_value_bytes must be positive",
            ));
        }
        if self.max_receipt_bytes == 0 {
            return Err(StoreError::InvalidLimits(
                "max_receipt_bytes must be positive",
            ));
        }
        Ok(self)
    }
}

impl Default for StoreLimits {
    fn default() -> Self {
        Self::new(16 * 1024 * 1024, 1024 * 1024)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Value,
    Receipt,
}

impl fmt::Display for SpanKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value => formatter.write_str("value"),
            Self::Receipt => formatter.write_str("receipt"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalValue {
    bytes: Vec<u8>,
    digest: Digest,
}

impl CanonicalValue {
    pub fn verify(
        bytes: Vec<u8>,
        claimed_digest: Digest,
        verifier: &impl DigestVerifier,
        limits: StoreLimits,
    ) -> Result<Self, StoreError> {
        let limits = limits.validate()?;
        validate_span(SpanKind::Value, &bytes, limits.max_value_bytes)?;
        let actual_digest = Digest::from_bytes(verifier.sha256(&bytes));
        if actual_digest != claimed_digest {
            return Err(StoreError::DigestMismatch {
                claimed: claimed_digest,
                actual: actual_digest,
            });
        }
        Ok(Self {
            bytes,
            digest: claimed_digest,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub const fn digest(&self) -> Digest {
        self.digest
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpaqueReceipt {
    bytes: Vec<u8>,
}

impl OpaqueReceipt {
    pub fn new(bytes: Vec<u8>, limits: StoreLimits) -> Result<Self, StoreError> {
        let limits = limits.validate()?;
        validate_span(SpanKind::Receipt, &bytes, limits.max_receipt_bytes)?;
        Ok(Self { bytes })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

fn validate_span(kind: SpanKind, bytes: &[u8], limit: usize) -> Result<(), StoreError> {
    if bytes.is_empty() {
        return Err(StoreError::EmptySpan(kind));
    }
    if bytes.len() > limit {
        return Err(StoreError::SpanLimitExceeded {
            kind,
            limit,
            actual: bytes.len(),
        });
    }
    Ok(())
}

fn validate_revision(revision: u64) -> Result<(), StoreError> {
    if revision > MAX_REVISION {
        return Err(StoreError::RevisionOutOfRange { revision });
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    revision: u64,
    value: CanonicalValue,
}

impl Snapshot {
    pub fn new(revision: u64, value: CanonicalValue) -> Result<Self, StoreError> {
        validate_revision(revision)?;
        Ok(Self { revision, value })
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub const fn value(&self) -> &CanonicalValue {
        &self.value
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompareAndSwap {
    expected_revision: u64,
    revision: u64,
    value: CanonicalValue,
    receipt_key: Digest,
    receipt: OpaqueReceipt,
}

impl CompareAndSwap {
    pub fn new(
        expected_revision: u64,
        revision: u64,
        value: CanonicalValue,
        receipt_key: Digest,
        receipt: OpaqueReceipt,
    ) -> Result<Self, StoreError> {
        validate_revision(expected_revision)?;
        validate_revision(revision)?;
        if expected_revision == MAX_REVISION || revision != expected_revision.saturating_add(1) {
            return Err(StoreError::InvalidRevisionStep {
                expected: expected_revision,
                revision,
            });
        }
        Ok(Self {
            expected_revision,
            revision,
            value,
            receipt_key,
            receipt,
        })
    }

    pub const fn expected_revision(&self) -> u64 {
        self.expected_revision
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub const fn value(&self) -> &CanonicalValue {
        &self.value
    }

    pub const fn receipt_key(&self) -> Digest {
        self.receipt_key
    }

    pub const fn receipt(&self) -> &OpaqueReceipt {
        &self.receipt
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyStatus {
    Applied,
    Replayed,
}

impl ApplyStatus {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Replayed => "replayed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitReceipt {
    status: ApplyStatus,
    revision: u64,
    receipt_key: Digest,
    receipt: OpaqueReceipt,
}

impl CommitReceipt {
    fn from_commit(status: ApplyStatus, commit: &StoredCommit) -> Self {
        Self {
            status,
            revision: commit.revision,
            receipt_key: commit.receipt_key,
            receipt: commit.receipt.clone(),
        }
    }

    pub const fn status(&self) -> ApplyStatus {
        self.status
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub const fn receipt_key(&self) -> Digest {
        self.receipt_key
    }

    pub const fn receipt(&self) -> &OpaqueReceipt {
        &self.receipt
    }
}

pub trait OpaqueValueStore: Send + Sync {
    fn load(&self) -> Result<Option<Snapshot>, StoreError>;

    fn initialize(&self, snapshot: Snapshot) -> Result<Snapshot, StoreError>;

    fn compare_and_swap(&self, request: CompareAndSwap) -> Result<CommitReceipt, StoreError>;

    fn receipt(&self, receipt_key: Digest) -> Result<Option<CommitReceipt>, StoreError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultPoint {
    BeforeCommit,
    AfterCommit,
}

impl fmt::Display for FaultPoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BeforeCommit => formatter.write_str("before-commit"),
            Self::AfterCommit => formatter.write_str("after-commit"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    InvalidLimits(&'static str),
    InvalidDigest,
    EmptySpan(SpanKind),
    SpanLimitExceeded {
        kind: SpanKind,
        limit: usize,
        actual: usize,
    },
    DigestMismatch {
        claimed: Digest,
        actual: Digest,
    },
    RevisionOutOfRange {
        revision: u64,
    },
    InvalidRevisionStep {
        expected: u64,
        revision: u64,
    },
    AlreadyInitialized {
        current_revision: u64,
    },
    Uninitialized,
    StaleRevision {
        expected: u64,
        actual: u64,
    },
    ReceiptCollision {
        receipt_key: Digest,
    },
    FaultAlreadyPending {
        fault: FaultPoint,
    },
    InjectedFault {
        fault: FaultPoint,
    },
    Poisoned,
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLimits(message) => write!(formatter, "invalid store limits: {message}"),
            Self::InvalidDigest => {
                formatter.write_str("digest must use canonical lowercase sha256:<64 hex digits>")
            }
            Self::EmptySpan(kind) => write!(formatter, "canonical {kind} span must not be empty"),
            Self::SpanLimitExceeded {
                kind,
                limit,
                actual,
            } => write!(
                formatter,
                "canonical {kind} span is {actual} bytes with limit {limit}"
            ),
            Self::DigestMismatch { claimed, actual } => write!(
                formatter,
                "canonical value digest mismatch: claimed {claimed}, computed {actual}"
            ),
            Self::RevisionOutOfRange { revision } => write!(
                formatter,
                "revision {revision} exceeds the signed 64-bit storage boundary"
            ),
            Self::InvalidRevisionStep { expected, revision } => write!(
                formatter,
                "revision {revision} must be exactly one greater than {expected}"
            ),
            Self::AlreadyInitialized { current_revision } => write!(
                formatter,
                "store is already initialized at revision {current_revision}"
            ),
            Self::Uninitialized => formatter.write_str("store is not initialized"),
            Self::StaleRevision { expected, actual } => write!(
                formatter,
                "compare-and-swap expected revision {expected}, current revision is {actual}"
            ),
            Self::ReceiptCollision { receipt_key } => write!(
                formatter,
                "receipt key {receipt_key} is already bound to different canonical bytes"
            ),
            Self::FaultAlreadyPending { fault } => {
                write!(formatter, "fault {fault} is already pending")
            }
            Self::InjectedFault { fault } => {
                write!(formatter, "injected store fault at {fault}")
            }
            Self::Poisoned => formatter.write_str("in-memory store lock is poisoned"),
        }
    }
}

impl std::error::Error for StoreError {}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredCommit {
    expected_revision: u64,
    revision: u64,
    value: CanonicalValue,
    receipt_key: Digest,
    receipt: OpaqueReceipt,
}

impl StoredCommit {
    fn from_request(request: &CompareAndSwap) -> Self {
        Self {
            expected_revision: request.expected_revision,
            revision: request.revision,
            value: request.value.clone(),
            receipt_key: request.receipt_key,
            receipt: request.receipt.clone(),
        }
    }

    fn matches(&self, request: &CompareAndSwap) -> bool {
        self.expected_revision == request.expected_revision
            && self.revision == request.revision
            && self.value == request.value
            && self.receipt_key == request.receipt_key
            && self.receipt == request.receipt
    }
}

#[derive(Debug, Default)]
struct MemoryState {
    snapshot: Option<Snapshot>,
    commits: BTreeMap<Digest, StoredCommit>,
    pending_fault: Option<FaultPoint>,
}

#[derive(Debug, Default)]
pub struct InMemoryStore {
    state: Mutex<MemoryState>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn inject_fault(&self, fault: FaultPoint) -> Result<(), StoreError> {
        let mut state = self.lock_state()?;
        if let Some(pending) = state.pending_fault {
            return Err(StoreError::FaultAlreadyPending { fault: pending });
        }
        state.pending_fault = Some(fault);
        Ok(())
    }

    pub fn pending_fault(&self) -> Result<Option<FaultPoint>, StoreError> {
        Ok(self.lock_state()?.pending_fault)
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, MemoryState>, StoreError> {
        self.state.lock().map_err(|_| StoreError::Poisoned)
    }
}

impl OpaqueValueStore for InMemoryStore {
    fn load(&self) -> Result<Option<Snapshot>, StoreError> {
        Ok(self.lock_state()?.snapshot.clone())
    }

    fn initialize(&self, snapshot: Snapshot) -> Result<Snapshot, StoreError> {
        let mut state = self.lock_state()?;
        match &state.snapshot {
            None => {
                state.snapshot = Some(snapshot.clone());
                Ok(snapshot)
            }
            Some(current) if current == &snapshot => Ok(current.clone()),
            Some(current) => Err(StoreError::AlreadyInitialized {
                current_revision: current.revision,
            }),
        }
    }

    fn compare_and_swap(&self, request: CompareAndSwap) -> Result<CommitReceipt, StoreError> {
        let mut state = self.lock_state()?;

        if let Some(commit) = state.commits.get(&request.receipt_key) {
            if commit.matches(&request) {
                return Ok(CommitReceipt::from_commit(ApplyStatus::Replayed, commit));
            }
            return Err(StoreError::ReceiptCollision {
                receipt_key: request.receipt_key,
            });
        }

        let actual_revision = state
            .snapshot
            .as_ref()
            .ok_or(StoreError::Uninitialized)?
            .revision;
        if actual_revision != request.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: request.expected_revision,
                actual: actual_revision,
            });
        }

        let pending_fault = state.pending_fault.take();
        if pending_fault == Some(FaultPoint::BeforeCommit) {
            return Err(StoreError::InjectedFault {
                fault: FaultPoint::BeforeCommit,
            });
        }

        let commit = StoredCommit::from_request(&request);
        state.snapshot = Some(Snapshot {
            revision: request.revision,
            value: request.value.clone(),
        });
        state.commits.insert(request.receipt_key, commit.clone());

        if pending_fault == Some(FaultPoint::AfterCommit) {
            return Err(StoreError::InjectedFault {
                fault: FaultPoint::AfterCommit,
            });
        }

        Ok(CommitReceipt::from_commit(ApplyStatus::Applied, &commit))
    }

    fn receipt(&self, receipt_key: Digest) -> Result<Option<CommitReceipt>, StoreError> {
        Ok(self
            .lock_state()?
            .commits
            .get(&receipt_key)
            .map(|commit| CommitReceipt::from_commit(ApplyStatus::Replayed, commit)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Clone, Copy, Debug)]
    struct TestVerifier;

    impl DigestVerifier for TestVerifier {
        fn sha256(&self, canonical_bytes: &[u8]) -> [u8; 32] {
            let mut digest = [0_u8; 32];
            for (index, byte) in canonical_bytes.iter().copied().enumerate() {
                let slot = index % digest.len();
                digest[slot] = digest[slot].wrapping_add(byte).wrapping_add(index as u8);
            }
            digest[31] ^= canonical_bytes.len() as u8;
            digest
        }
    }

    fn limits() -> StoreLimits {
        StoreLimits::new(1024, 512)
    }

    fn digest_for(bytes: &[u8]) -> Digest {
        Digest::from_bytes(TestVerifier.sha256(bytes))
    }

    fn value(bytes: &[u8]) -> CanonicalValue {
        CanonicalValue::verify(bytes.to_vec(), digest_for(bytes), &TestVerifier, limits())
            .expect("test value must verify")
    }

    fn receipt(bytes: &[u8]) -> OpaqueReceipt {
        OpaqueReceipt::new(bytes.to_vec(), limits()).expect("test receipt must be bounded")
    }

    fn snapshot(revision: u64, bytes: &[u8]) -> Snapshot {
        Snapshot::new(revision, value(bytes)).expect("test revision must be valid")
    }

    fn request(
        expected_revision: u64,
        revision: u64,
        value_bytes: &[u8],
        receipt_key_bytes: &[u8],
        receipt_bytes: &[u8],
    ) -> CompareAndSwap {
        CompareAndSwap::new(
            expected_revision,
            revision,
            value(value_bytes),
            digest_for(receipt_key_bytes),
            receipt(receipt_bytes),
        )
        .expect("test compare-and-swap must be valid")
    }

    #[test]
    fn constants_match_the_hal_profile() {
        assert_eq!(SERVICE, "hara.store");
        assert_eq!(REQUEST_PROTOCOL, "hara.store-request/1");
        assert_eq!(RESULT_PROTOCOL, "hara.store-result/1");
        assert_eq!(ApplyStatus::Applied.name(), "applied");
        assert_eq!(ApplyStatus::Replayed.name(), "replayed");
    }

    #[test]
    fn digests_are_strict_canonical_lowercase_sha256_values() {
        let digest = digest_for(b"value");
        let encoded = digest.to_string();

        assert_eq!(Digest::parse(&encoded), Ok(digest));
        assert_eq!(encoded.len(), "sha256:".len() + 64);
        assert_eq!(
            Digest::parse(&encoded.to_ascii_uppercase()),
            Err(StoreError::InvalidDigest)
        );
        assert_eq!(Digest::parse("sha256:abc"), Err(StoreError::InvalidDigest));
        assert_eq!(
            Digest::parse(
                "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            Err(StoreError::InvalidDigest)
        );
    }

    #[test]
    fn canonical_values_verify_digest_and_bounds() {
        let bytes = b"canonical nested hta bytes".to_vec();
        let digest = digest_for(&bytes);

        let verified = CanonicalValue::verify(bytes.clone(), digest, &TestVerifier, limits())
            .expect("matching digest must verify");
        assert_eq!(verified.bytes(), bytes.as_slice());
        assert_eq!(verified.digest(), digest);

        let wrong = Digest::from_bytes([9_u8; 32]);
        assert!(matches!(
            CanonicalValue::verify(bytes.clone(), wrong, &TestVerifier, limits()),
            Err(StoreError::DigestMismatch { .. })
        ));
        assert_eq!(
            CanonicalValue::verify(Vec::new(), digest_for(&[]), &TestVerifier, limits()),
            Err(StoreError::EmptySpan(SpanKind::Value))
        );
        assert!(matches!(
            CanonicalValue::verify(
                vec![1_u8; 1025],
                digest_for(&vec![1_u8; 1025]),
                &TestVerifier,
                limits(),
            ),
            Err(StoreError::SpanLimitExceeded {
                kind: SpanKind::Value,
                limit: 1024,
                actual: 1025,
            })
        ));
    }

    #[test]
    fn revisions_are_bounded_and_advance_exactly_once() {
        assert!(Snapshot::new(MAX_REVISION, value(b"max")).is_ok());
        assert_eq!(
            Snapshot::new(MAX_REVISION + 1, value(b"overflow")),
            Err(StoreError::RevisionOutOfRange {
                revision: MAX_REVISION + 1,
            })
        );
        assert_eq!(
            CompareAndSwap::new(
                4,
                6,
                value(b"skip"),
                digest_for(b"skip-key"),
                receipt(b"skip-receipt"),
            ),
            Err(StoreError::InvalidRevisionStep {
                expected: 4,
                revision: 6,
            })
        );
        assert_eq!(
            CompareAndSwap::new(
                MAX_REVISION,
                MAX_REVISION,
                value(b"terminal"),
                digest_for(b"terminal-key"),
                receipt(b"terminal-receipt"),
            ),
            Err(StoreError::InvalidRevisionStep {
                expected: MAX_REVISION,
                revision: MAX_REVISION,
            })
        );
    }

    #[test]
    fn initialize_is_exact_idempotent_and_loadable() {
        let store = InMemoryStore::new();
        assert_eq!(store.load(), Ok(None));

        let initial = snapshot(0, b"state-0");
        assert_eq!(store.initialize(initial.clone()), Ok(initial.clone()));
        assert_eq!(store.initialize(initial.clone()), Ok(initial.clone()));
        assert_eq!(store.load(), Ok(Some(initial)));

        assert_eq!(
            store.initialize(snapshot(0, b"different-state-0")),
            Err(StoreError::AlreadyInitialized {
                current_revision: 0,
            })
        );
    }

    #[test]
    fn compare_and_swap_atomically_publishes_value_and_receipt() {
        let store = InMemoryStore::new();
        store
            .initialize(snapshot(0, b"state-0"))
            .expect("store must initialize");
        let update = request(0, 1, b"state-1", b"plan-1", b"receipt-1");
        let key = update.receipt_key();

        let applied = store
            .compare_and_swap(update.clone())
            .expect("eligible compare-and-swap must apply");
        assert_eq!(applied.status(), ApplyStatus::Applied);
        assert_eq!(applied.revision(), 1);
        assert_eq!(applied.receipt_key(), key);
        assert_eq!(applied.receipt().bytes(), b"receipt-1");
        assert_eq!(store.load(), Ok(Some(snapshot(1, b"state-1"))));

        let loaded_receipt = store
            .receipt(key)
            .expect("receipt lookup must succeed")
            .expect("receipt must exist");
        assert_eq!(loaded_receipt.status(), ApplyStatus::Replayed);
        assert_eq!(loaded_receipt.receipt().bytes(), b"receipt-1");
    }

    #[test]
    fn exact_retries_replay_before_the_stale_revision_check() {
        let store = InMemoryStore::new();
        store
            .initialize(snapshot(0, b"state-0"))
            .expect("store must initialize");
        let update = request(0, 1, b"state-1", b"plan-1", b"receipt-1");

        assert_eq!(
            store
                .compare_and_swap(update.clone())
                .expect("first call must apply")
                .status(),
            ApplyStatus::Applied
        );
        assert_eq!(
            store
                .compare_and_swap(update)
                .expect("exact retry must replay")
                .status(),
            ApplyStatus::Replayed
        );
    }

    #[test]
    fn stale_writers_and_receipt_key_collisions_fail_closed() {
        let store = InMemoryStore::new();
        store
            .initialize(snapshot(0, b"state-0"))
            .expect("store must initialize");
        let first = request(0, 1, b"state-1", b"shared-key", b"receipt-1");
        store
            .compare_and_swap(first)
            .expect("first compare-and-swap must apply");

        assert_eq!(
            store.compare_and_swap(request(
                0,
                1,
                b"other-state",
                b"other-key",
                b"other-receipt",
            )),
            Err(StoreError::StaleRevision {
                expected: 0,
                actual: 1,
            })
        );
        let collision = request(0, 1, b"substituted-state", b"shared-key", b"receipt-1");
        assert_eq!(
            store.compare_and_swap(collision),
            Err(StoreError::ReceiptCollision {
                receipt_key: digest_for(b"shared-key"),
            })
        );
    }

    #[test]
    fn before_commit_fault_leaves_no_value_or_receipt() {
        let store = InMemoryStore::new();
        let initial = snapshot(0, b"state-0");
        store
            .initialize(initial.clone())
            .expect("store must initialize");
        let update = request(0, 1, b"state-1", b"plan-1", b"receipt-1");
        let key = update.receipt_key();

        store
            .inject_fault(FaultPoint::BeforeCommit)
            .expect("fault must be installed");
        assert_eq!(
            store.compare_and_swap(update),
            Err(StoreError::InjectedFault {
                fault: FaultPoint::BeforeCommit,
            })
        );
        assert_eq!(store.load(), Ok(Some(initial)));
        assert_eq!(store.receipt(key), Ok(None));
        assert_eq!(store.pending_fault(), Ok(None));
    }

    #[test]
    fn after_commit_fault_is_recoverable_by_load_receipt_and_retry() {
        let store = InMemoryStore::new();
        store
            .initialize(snapshot(0, b"state-0"))
            .expect("store must initialize");
        let update = request(0, 1, b"state-1", b"plan-1", b"receipt-1");
        let key = update.receipt_key();

        store
            .inject_fault(FaultPoint::AfterCommit)
            .expect("fault must be installed");
        assert_eq!(
            store.compare_and_swap(update.clone()),
            Err(StoreError::InjectedFault {
                fault: FaultPoint::AfterCommit,
            })
        );
        assert_eq!(store.load(), Ok(Some(snapshot(1, b"state-1"))));
        assert_eq!(
            store
                .receipt(key)
                .expect("receipt lookup must work")
                .expect("receipt must exist")
                .status(),
            ApplyStatus::Replayed
        );
        assert_eq!(
            store
                .compare_and_swap(update)
                .expect("retry must replay")
                .status(),
            ApplyStatus::Replayed
        );
    }

    #[test]
    fn ineligible_requests_do_not_consume_pending_faults() {
        let store = InMemoryStore::new();
        store
            .initialize(snapshot(0, b"state-0"))
            .expect("store must initialize");
        store
            .compare_and_swap(request(0, 1, b"state-1", b"plan-1", b"receipt-1"))
            .expect("first update must apply");
        store
            .inject_fault(FaultPoint::BeforeCommit)
            .expect("fault must be installed");

        assert!(matches!(
            store.compare_and_swap(request(0, 1, b"stale", b"stale-plan", b"stale-receipt",)),
            Err(StoreError::StaleRevision { .. })
        ));
        assert_eq!(store.pending_fault(), Ok(Some(FaultPoint::BeforeCommit)));
    }

    #[test]
    fn canonical_nested_spans_are_preserved_exactly() {
        let store = InMemoryStore::new();
        let initial_bytes = b"\x48\x54\x41\x01nested\x00value";
        let next_bytes = b"\x48\x54\x41\x01nested\x00next";
        let receipt_bytes = b"\x48\x54\x41\x01opaque\x00receipt";

        store
            .initialize(snapshot(0, initial_bytes))
            .expect("store must initialize");
        let update = request(0, 1, next_bytes, b"nested-plan", receipt_bytes);
        let key = update.receipt_key();
        store.compare_and_swap(update).expect("update must apply");

        assert_eq!(
            store
                .load()
                .expect("load must work")
                .expect("snapshot must exist")
                .value()
                .bytes(),
            next_bytes
        );
        assert_eq!(
            store
                .receipt(key)
                .expect("receipt lookup must work")
                .expect("receipt must exist")
                .receipt()
                .bytes(),
            receipt_bytes
        );
    }

    #[test]
    fn store_is_safe_to_share_between_host_workers() {
        let store = Arc::new(InMemoryStore::new());
        store
            .initialize(snapshot(0, b"state-0"))
            .expect("store must initialize");

        let first = Arc::clone(&store);
        let second = Arc::clone(&store);
        let left = std::thread::spawn(move || {
            first.compare_and_swap(request(0, 1, b"left-state", b"left-plan", b"left-receipt"))
        });
        let right = std::thread::spawn(move || {
            second.compare_and_swap(request(
                0,
                1,
                b"right-state",
                b"right-plan",
                b"right-receipt",
            ))
        });

        let left = left.join().expect("left worker must not panic");
        let right = right.join().expect("right worker must not panic");
        let left_applied = left.is_ok();
        let right_applied = right.is_ok();
        let left_stale = matches!(left, Err(StoreError::StaleRevision { .. }));
        let right_stale = matches!(right, Err(StoreError::StaleRevision { .. }));
        assert!((left_applied && right_stale) || (right_applied && left_stale));
        assert_eq!(
            store
                .load()
                .expect("load must work")
                .expect("snapshot must exist")
                .revision(),
            1
        );
    }
}
