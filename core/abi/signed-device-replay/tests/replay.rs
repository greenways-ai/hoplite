use hoplite_data_plane_abi::{
    authenticate_application_request, ApplicationRequestExpectation, SignedDeviceError,
    SignedDevicePrincipal, SignedDeviceProvider, SignedDeviceRequest,
};
use hoplite_signed_device_replay::{
    authenticate_and_admit_application_request, MemoryReplayStore, ReplayCandidate, ReplayError,
    ReplayLookup, ReplayStatus, ReplayStore, SqliteReplayStore, REPLAY_EVIDENCE_PROFILE,
    REPLAY_RECEIPT_PROFILE,
};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

const NOW: i64 = 1_786_080_000;
const DIGEST_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const LOCK_DIGEST: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const SIGNATURE_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const SIGNATURE_B: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

#[derive(Clone)]
struct AllowProvider {
    principal: SignedDevicePrincipal,
    calls: usize,
}

impl SignedDeviceProvider for AllowProvider {
    fn authenticate(
        &mut self,
        _request: &SignedDeviceRequest<'_>,
    ) -> Result<SignedDevicePrincipal, SignedDeviceError> {
        self.calls += 1;
        Ok(self.principal.clone())
    }
}

fn principal(key_id: &str) -> SignedDevicePrincipal {
    SignedDevicePrincipal {
        subject: "profile.primary".into(),
        realm: "application".into(),
        device_id: "device.a".into(),
        key_id: key_id.into(),
        provider: "test/allow".into(),
        claims: BTreeMap::from([
            ("application/id".into(), "app.example".into()),
            ("application/version".into(), "1.0.0".into()),
            ("application/publisher".into(), "greenways.example".into()),
            ("application/lock-digest".into(), LOCK_DIGEST.into()),
            ("application/namespace".into(), "profile.primary".into()),
            ("application/collection".into(), "objects".into()),
            (
                "application/operations".into(),
                "object.upload,object.read".into(),
            ),
        ]),
    }
}

fn request<'a>(
    digest: &'a str,
    operation: &'a str,
    nonce: &'a str,
    idempotency_key: &'a str,
    key_id: &'a str,
    signature: &'a str,
) -> SignedDeviceRequest<'a> {
    SignedDeviceRequest {
        method: "PUT",
        target: "/objects",
        authority: "hoplite.local",
        content_digest: digest,
        operation,
        application: "app.example",
        namespace: "profile.primary",
        collection: "objects",
        timestamp: NOW,
        nonce,
        idempotency_key,
        key_id,
        signature,
    }
}

fn expectation<'a>(request: &SignedDeviceRequest<'a>) -> ApplicationRequestExpectation<'a> {
    ApplicationRequestExpectation {
        method: request.method,
        target: request.target,
        authority: request.authority,
        content_digest: request.content_digest,
        operation: request.operation,
        application: request.application,
        namespace: request.namespace,
        collection: request.collection,
    }
}

fn verified(
    request: &SignedDeviceRequest<'_>,
) -> hoplite_data_plane_abi::VerifiedApplicationRequest {
    let mut provider = AllowProvider {
        principal: principal(request.key_id),
        calls: 0,
    };
    authenticate_application_request(&mut provider, request, &expectation(request)).unwrap()
}

fn candidate(request: &SignedDeviceRequest<'_>, admitted_at: i64) -> ReplayCandidate {
    let verified = verified(request);
    ReplayCandidate::from_verified(request, &verified, admitted_at).unwrap()
}

fn base_request() -> SignedDeviceRequest<'static> {
    request(
        DIGEST_A,
        "object.upload",
        "nonce-device-a-0001",
        "idempotency-device-a-0001",
        "key.device-a",
        SIGNATURE_A,
    )
}

fn assert_store_law<S: ReplayStore>(store: &S) {
    let request = base_request();
    let first = store.admit(candidate(&request, NOW + 1)).unwrap();
    assert_eq!(first.profile(), REPLAY_RECEIPT_PROFILE);
    assert_eq!(first.status(), ReplayStatus::Applied);
    assert_eq!(first.evidence().profile(), REPLAY_EVIDENCE_PROFILE);

    let replayed = store.admit(candidate(&request, NOW + 99)).unwrap();
    assert_eq!(replayed.status(), ReplayStatus::Replayed);
    assert_eq!(replayed.evidence(), first.evidence());
    assert_eq!(replayed.evidence().admitted_at(), NOW + 1);

    let lookup = ReplayLookup::from_evidence(first.evidence());
    assert_eq!(
        store.lookup(&lookup).unwrap().as_ref(),
        Some(first.evidence())
    );
}

fn assert_collision_law<S: ReplayStore>(store: &S) {
    let first = base_request();
    store.admit(candidate(&first, NOW + 1)).unwrap();

    let collision = request(
        DIGEST_B,
        "object.upload",
        "nonce-device-a-0002",
        first.idempotency_key,
        first.key_id,
        SIGNATURE_A,
    );
    assert_eq!(
        store.admit(candidate(&collision, NOW + 2)).unwrap_err(),
        ReplayError::IdempotencyCollision
    );

    let reused_nonce = request(
        DIGEST_A,
        "object.upload",
        first.nonce,
        "idempotency-device-a-0002",
        first.key_id,
        SIGNATURE_A,
    );
    assert_eq!(
        store.admit(candidate(&reused_nonce, NOW + 3)).unwrap_err(),
        ReplayError::NonceReused
    );
}

#[test]
fn memory_store_applies_replays_and_recovers_exact_evidence() {
    assert_store_law(&MemoryReplayStore::default());
}

#[test]
fn memory_store_rejects_idempotency_collisions_and_nonce_reuse() {
    assert_collision_law(&MemoryReplayStore::default());
}

#[test]
fn the_fingerprint_excludes_only_the_signature() {
    let first = base_request();
    let second = request(
        first.content_digest,
        first.operation,
        first.nonce,
        first.idempotency_key,
        first.key_id,
        SIGNATURE_B,
    );
    let first = candidate(&first, NOW + 1);
    let second = candidate(&second, NOW + 2);
    assert_eq!(
        first.evidence().fingerprint(),
        second.evidence().fingerprint()
    );
}

#[test]
fn candidates_require_the_exact_verified_request() {
    let first = base_request();
    let verified = verified(&first);
    let changed = request(
        first.content_digest,
        first.operation,
        "nonce-device-a-9999",
        first.idempotency_key,
        first.key_id,
        first.signature,
    );
    assert_eq!(
        ReplayCandidate::from_verified(&changed, &verified, NOW + 1).unwrap_err(),
        ReplayError::InvalidVerifiedRequest
    );
    assert_eq!(
        ReplayCandidate::from_verified(&first, &verified, 0).unwrap_err(),
        ReplayError::InvalidAdmittedAt
    );
}

#[test]
fn authentication_precedes_durable_admission() {
    let store = MemoryReplayStore::default();
    let request = base_request();
    let mut provider = AllowProvider {
        principal: principal(request.key_id),
        calls: 0,
    };
    let mut wrong = expectation(&request);
    wrong.operation = "object.read";

    let error = authenticate_and_admit_application_request(
        &mut provider,
        &store,
        &request,
        &wrong,
        NOW + 1,
    )
    .unwrap_err();
    assert_eq!(error.code(), "hoplite.application-auth/request-mismatch");
    assert_eq!(provider.calls, 0);
    assert!(store
        .lookup(&ReplayLookup::from_verified(&verified(&request)))
        .unwrap()
        .is_none());

    let applied = authenticate_and_admit_application_request(
        &mut provider,
        &store,
        &request,
        &expectation(&request),
        NOW + 1,
    )
    .unwrap();
    assert_eq!(provider.calls, 1);
    assert_eq!(applied.replay().status(), ReplayStatus::Applied);
    assert_eq!(applied.verified().operation(), "object.upload");
}

#[test]
fn debug_output_redacts_nonce_idempotency_and_signature_authority() {
    let store = MemoryReplayStore::default();
    let request = base_request();
    let receipt = store.admit(candidate(&request, NOW + 1)).unwrap();
    let debug = format!("{receipt:?}");
    assert!(!debug.contains(request.nonce));
    assert!(!debug.contains(request.idempotency_key));
    assert!(!debug.contains(request.signature));
    assert!(!debug.contains("test/allow"));
}

#[test]
fn sqlite_store_matches_the_memory_admission_law() {
    let store = SqliteReplayStore::open_in_memory().unwrap();
    assert_store_law(&store);
}

#[test]
fn sqlite_store_rejects_collisions_and_nonce_reuse() {
    let store = SqliteReplayStore::open_in_memory().unwrap();
    assert_collision_law(&store);
}

#[test]
fn sqlite_replay_survives_a_fresh_store_process() {
    let path = temp_path("restart");
    let request = base_request();
    let first = {
        let store = SqliteReplayStore::open(&path).unwrap();
        store.admit(candidate(&request, NOW + 1)).unwrap()
    };
    let reopened = SqliteReplayStore::open(&path).unwrap();
    let replayed = reopened.admit(candidate(&request, NOW + 500)).unwrap();
    assert_eq!(replayed.status(), ReplayStatus::Replayed);
    assert_eq!(replayed.evidence(), first.evidence());
    drop(reopened);
    cleanup(&path);
}

#[test]
fn concurrent_sqlite_admission_resolves_to_one_apply_and_one_replay() {
    let path = temp_path("race");
    let first_store = Arc::new(SqliteReplayStore::open(&path).unwrap());
    let second_store = Arc::new(SqliteReplayStore::open(&path).unwrap());
    let barrier = Arc::new(Barrier::new(2));
    let request_a = base_request();
    let request_b = base_request();

    let first = {
        let store = Arc::clone(&first_store);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            store.admit(candidate(&request_a, NOW + 1)).unwrap()
        })
    };
    let second = {
        let store = Arc::clone(&second_store);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            store.admit(candidate(&request_b, NOW + 2)).unwrap()
        })
    };

    let first = first.join().unwrap();
    let second = second.join().unwrap();
    let statuses = [first.status(), second.status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == ReplayStatus::Applied)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == ReplayStatus::Replayed)
            .count(),
        1
    );
    assert_eq!(first.evidence(), second.evidence());
    drop(first_store);
    drop(second_store);
    cleanup(&path);
}

#[test]
fn nonce_and_idempotency_scopes_survive_key_rotation() {
    let store = MemoryReplayStore::default();
    let old = base_request();
    store.admit(candidate(&old, NOW + 1)).unwrap();

    let rotated_nonce_reuse = request(
        old.content_digest,
        old.operation,
        old.nonce,
        "idempotency-device-a-rotated",
        "key.device-a.rotated",
        SIGNATURE_B,
    );
    assert_eq!(
        store
            .admit(candidate(&rotated_nonce_reuse, NOW + 2))
            .unwrap_err(),
        ReplayError::NonceReused
    );

    let rotated_collision = request(
        old.content_digest,
        old.operation,
        "nonce-device-a-rotated",
        old.idempotency_key,
        "key.device-a.rotated",
        SIGNATURE_B,
    );
    assert_eq!(
        store
            .admit(candidate(&rotated_collision, NOW + 3))
            .unwrap_err(),
        ReplayError::IdempotencyCollision
    );
}

#[test]
fn sqlite_open_detects_corrupt_fingerprints_without_leaking_paths_or_sql() {
    let path = temp_path("corrupt");
    let request = base_request();
    let debug = {
        let store = SqliteReplayStore::open(&path).unwrap();
        store.admit(candidate(&request, NOW + 1)).unwrap();
        format!("{store:?}")
    };
    assert!(!debug.contains(path.to_string_lossy().as_ref()));

    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE signed_request_admissions
             SET request_fingerprint = ?1",
            ["sha256:0000000000000000000000000000000000000000000000000000000000000000"],
        )
        .unwrap();
    drop(connection);

    let error = SqliteReplayStore::open(&path).unwrap_err();
    assert_eq!(error, ReplayError::CorruptRecord);
    let message = error.to_string();
    assert!(!message.contains(path.to_string_lossy().as_ref()));
    assert!(!message.to_ascii_lowercase().contains("select"));
    assert!(!message.to_ascii_lowercase().contains("update"));
    cleanup(&path);
}

#[test]
fn sqlite_rejects_unknown_schema_versions_with_a_stable_error() {
    let path = temp_path("schema");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("PRAGMA user_version = 99;")
        .unwrap();
    drop(connection);

    let error = SqliteReplayStore::open(&path).unwrap_err();
    assert_eq!(error, ReplayError::UnsupportedSchema);
    assert_eq!(error.code(), "hoplite.replay/schema-unsupported");
    cleanup(&path);
}

fn temp_path(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    std::env::temp_dir()
        .join(format!(
            "hoplite-signed-replay-{}-{}-{label}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
        .join("replay.sqlite3")
}

fn cleanup(path: &Path) {
    let _ = fs::remove_file(path);
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    let _ = fs::remove_file(PathBuf::from(wal));
    let mut shm = path.as_os_str().to_os_string();
    shm.push("-shm");
    let _ = fs::remove_file(PathBuf::from(shm));
    if let Some(parent) = path.parent() {
        let _ = fs::remove_dir(parent);
    }
}
