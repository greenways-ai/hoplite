use super::*;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);

struct TestDatabase {
    path: PathBuf,
}

impl TestDatabase {
    fn new(label: &str) -> Self {
        let id = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "hoplite-value-store-{label}-{}-{id}.sqlite3",
            std::process::id()
        ));
        remove_database_files(&path);
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        remove_database_files(&self.path);
    }
}

fn companion_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn remove_database_files(path: &Path) {
    for candidate in [
        path.to_path_buf(),
        companion_path(path, "-wal"),
        companion_path(path, "-shm"),
    ] {
        let _ = fs::remove_file(candidate);
    }
}

fn limits() -> StoreLimits {
    StoreLimits::new(1024 * 1024, 128 * 1024)
}

fn digest_for(bytes: &[u8]) -> Digest {
    Digest::from_bytes(Sha256Verifier.sha256(bytes))
}

fn value(bytes: &[u8]) -> CanonicalValue {
    CanonicalValue::verify(bytes.to_vec(), digest_for(bytes), &Sha256Verifier, limits())
        .expect("test canonical value must verify")
}

fn receipt(bytes: &[u8]) -> OpaqueReceipt {
    OpaqueReceipt::new(bytes.to_vec(), limits()).expect("test receipt must be bounded")
}

fn snapshot(revision: u64, bytes: &[u8]) -> Snapshot {
    Snapshot::new(revision, value(bytes)).expect("test snapshot revision must be valid")
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

fn open(database: &TestDatabase) -> SqliteValueStore {
    SqliteValueStore::open(database.path(), limits()).expect("test database must open")
}

fn driver_code(error: &StoreError) -> Option<&'static str> {
    match error {
        StoreError::Driver { code, .. } => Some(*code),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct FixedVerifier([u8; 32]);

impl DigestVerifier for FixedVerifier {
    fn sha256(&self, _canonical_bytes: &[u8]) -> [u8; 32] {
        self.0
    }
}

#[test]
fn creates_configured_versioned_schema_and_reopens() {
    let database = TestDatabase::new("schema");
    {
        let store = open(&database);
        assert_eq!(store.path(), database.path());
        assert_eq!(store.limits(), limits());
        assert_eq!(store.load(), Ok(None));
        assert_eq!(store.verify(), Ok(()));

        let connection = store.connection.lock().expect("test lock must work");
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("schema version must be readable");
        let journal_mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("journal mode must be readable");
        let synchronous: i64 = connection
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .expect("synchronous mode must be readable");
        let foreign_keys: i64 = connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign key mode must be readable");

        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(journal_mode, "wal");
        assert_eq!(synchronous, 2);
        assert_eq!(foreign_keys, 1);
    }

    let reopened = open(&database);
    assert_eq!(reopened.verify(), Ok(()));
    assert_eq!(reopened.load(), Ok(None));
}

#[test]
fn initialization_is_exact_idempotent_loadable_and_restart_safe() {
    let database = TestDatabase::new("initialize");
    let initial = snapshot(0, b"state-0");

    {
        let store = open(&database);
        assert_eq!(store.initialize(initial.clone()), Ok(initial.clone()));
        assert_eq!(store.initialize(initial.clone()), Ok(initial.clone()));
        assert_eq!(store.load(), Ok(Some(initial.clone())));
        assert_eq!(
            store.initialize(snapshot(0, b"different-state-0")),
            Err(StoreError::AlreadyInitialized {
                current_revision: 0,
            })
        );
    }

    let reopened = open(&database);
    assert_eq!(reopened.load(), Ok(Some(initial)));
}

#[test]
fn compare_and_swap_atomically_persists_snapshot_and_receipt() {
    let database = TestDatabase::new("cas");
    let store = open(&database);
    store
        .initialize(snapshot(0, b"state-0"))
        .expect("store must initialize");
    let update = request(0, 1, b"state-1", b"plan-1", b"receipt-1");
    let key = update.receipt_key();

    let applied = store
        .compare_and_swap(update)
        .expect("eligible update must apply");
    assert_eq!(applied.status(), ApplyStatus::Applied);
    assert_eq!(applied.revision(), 1);
    assert_eq!(applied.receipt_key(), key);
    assert_eq!(applied.receipt().bytes(), b"receipt-1");
    assert_eq!(store.load(), Ok(Some(snapshot(1, b"state-1"))));

    let loaded = store
        .receipt(key)
        .expect("receipt lookup must work")
        .expect("receipt must exist");
    assert_eq!(loaded.status(), ApplyStatus::Replayed);
    assert_eq!(loaded.receipt().bytes(), b"receipt-1");

    drop(store);
    let reopened = open(&database);
    assert_eq!(reopened.load(), Ok(Some(snapshot(1, b"state-1"))));
    assert_eq!(
        reopened
            .receipt(key)
            .expect("receipt lookup must work")
            .expect("receipt must survive restart")
            .receipt()
            .bytes(),
        b"receipt-1"
    );
}

#[test]
fn exact_retries_replay_even_after_later_commits() {
    let database = TestDatabase::new("replay");
    let store = open(&database);
    store
        .initialize(snapshot(0, b"state-0"))
        .expect("store must initialize");
    let first = request(0, 1, b"state-1", b"plan-1", b"receipt-1");
    let second = request(1, 2, b"state-2", b"plan-2", b"receipt-2");

    assert_eq!(
        store
            .compare_and_swap(first.clone())
            .expect("first update must apply")
            .status(),
        ApplyStatus::Applied
    );
    assert_eq!(
        store
            .compare_and_swap(second)
            .expect("second update must apply")
            .status(),
        ApplyStatus::Applied
    );
    assert_eq!(store.load(), Ok(Some(snapshot(2, b"state-2"))));
    assert_eq!(
        store
            .compare_and_swap(first)
            .expect("exact old retry must replay before stale checking")
            .status(),
        ApplyStatus::Replayed
    );
    assert_eq!(store.load(), Ok(Some(snapshot(2, b"state-2"))));
}

#[test]
fn two_connections_reject_stale_writers() {
    let database = TestDatabase::new("stale");
    let first = open(&database);
    first
        .initialize(snapshot(0, b"state-0"))
        .expect("store must initialize");
    let second = open(&database);

    first
        .compare_and_swap(request(0, 1, b"winner", b"winner-plan", b"winner-receipt"))
        .expect("first connection must win");
    assert_eq!(
        second.compare_and_swap(request(
            0,
            1,
            b"stale",
            b"stale-plan",
            b"stale-receipt",
        )),
        Err(StoreError::StaleRevision {
            expected: 0,
            actual: 1,
        })
    );
    assert_eq!(second.load(), Ok(Some(snapshot(1, b"winner"))));
}

#[test]
fn receipt_keys_and_revisions_cannot_be_rebound() {
    let database = TestDatabase::new("collisions");
    let store = open(&database);
    store
        .initialize(snapshot(0, b"state-0"))
        .expect("store must initialize");
    store
        .compare_and_swap(request(
            0,
            1,
            b"state-1",
            b"shared-plan",
            b"receipt-1",
        ))
        .expect("first update must apply");

    let receipt_collision = store.compare_and_swap(request(
        0,
        1,
        b"substituted-state",
        b"shared-plan",
        b"receipt-1",
    ));
    assert_eq!(
        receipt_collision,
        Err(StoreError::ReceiptCollision {
            receipt_key: digest_for(b"shared-plan"),
        })
    );

    let connection = store.connection.lock().expect("test lock must work");
    connection
        .execute(
            "INSERT INTO value_receipts \
             (receipt_key, expected_revision, revision, value, value_digest, receipt) \
             VALUES (?1, 0, 2, ?2, ?3, ?4)",
            params![
                digest_for(b"manually-conflicting-plan").to_string(),
                b"manual".as_slice(),
                digest_for(b"manual").to_string(),
                b"manual-receipt".as_slice(),
            ],
        )
        .expect_err("schema must reject a non-contiguous stored revision");
}

#[test]
fn sqlite_recomputes_real_sha256_at_the_driver_boundary() {
    let database = TestDatabase::new("digest-boundary");
    let store = open(&database);
    let fake_digest = Digest::from_bytes([7_u8; 32]);
    let fake_value = CanonicalValue::verify(
        b"not-really-seven".to_vec(),
        fake_digest,
        &FixedVerifier([7_u8; 32]),
        limits(),
    )
    .expect("fixture verifier must construct the adversarial value");
    let fake_snapshot = Snapshot::new(0, fake_value).expect("revision must be valid");

    assert!(matches!(
        store.initialize(fake_snapshot),
        Err(StoreError::DigestMismatch { claimed, .. }) if claimed == fake_digest
    ));
    assert_eq!(store.load(), Ok(None));
}

#[test]
fn receipt_insert_failure_rolls_back_the_snapshot_update() {
    let database = TestDatabase::new("rollback");
    let store = open(&database);
    let initial = snapshot(0, b"state-0");
    store
        .initialize(initial.clone())
        .expect("store must initialize");
    {
        let connection = store.connection.lock().expect("test lock must work");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_receipt_insert \
                 BEFORE INSERT ON value_receipts \
                 BEGIN SELECT RAISE(FAIL, 'forced receipt failure'); END;",
            )
            .expect("failure trigger must install");
    }
    let update = request(0, 1, b"state-1", b"plan-1", b"receipt-1");
    let key = update.receipt_key();

    let error = store
        .compare_and_swap(update)
        .expect_err("receipt failure must reject the transaction");
    assert_eq!(driver_code(&error), Some("sqlite-receipt-insert"));
    assert_eq!(store.load(), Ok(Some(initial)));
    assert_eq!(store.receipt(key), Ok(None));
}

#[test]
fn corrupted_snapshot_bytes_are_detected_on_load_and_verify() {
    let database = TestDatabase::new("corruption");
    let store = open(&database);
    store
        .initialize(snapshot(0, b"state-0"))
        .expect("store must initialize");
    {
        let connection = store.connection.lock().expect("test lock must work");
        connection
            .execute(
                "UPDATE value_snapshot SET value = ?1 WHERE singleton = 1",
                params![b"corrupt-state".as_slice()],
            )
            .expect("test corruption must apply");
    }

    assert!(matches!(store.load(), Err(StoreError::DigestMismatch { .. })));
    assert!(matches!(store.verify(), Err(StoreError::DigestMismatch { .. })));
}

#[test]
fn unsupported_schema_versions_are_rejected_without_rewrite() {
    let database = TestDatabase::new("schema-version");
    {
        let connection = Connection::open(database.path()).expect("raw database must open");
        connection
            .execute_batch("PRAGMA user_version = 99;")
            .expect("test schema version must install");
    }

    let error = SqliteValueStore::open(database.path(), limits())
        .expect_err("unsupported schema must be rejected");
    assert_eq!(driver_code(&error), Some("sqlite-schema-version"));

    let connection = Connection::open(database.path()).expect("raw database must reopen");
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("schema version must remain readable");
    assert_eq!(version, 99);
}

#[test]
fn orphan_receipts_are_rejected_on_restart() {
    let database = TestDatabase::new("orphan");
    {
        let store = open(&database);
        let connection = store.connection.lock().expect("test lock must work");
        connection
            .execute(
                "INSERT INTO value_receipts \
                 (receipt_key, expected_revision, revision, value, value_digest, receipt) \
                 VALUES (?1, 0, 1, ?2, ?3, ?4)",
                params![
                    digest_for(b"orphan-plan").to_string(),
                    b"orphan-state".as_slice(),
                    digest_for(b"orphan-state").to_string(),
                    b"orphan-receipt".as_slice(),
                ],
            )
            .expect("test orphan receipt must insert");
    }

    let error = SqliteValueStore::open(database.path(), limits())
        .expect_err("orphan receipts must fail verification");
    assert_eq!(driver_code(&error), Some("sqlite-corrupt-orphan-receipts"));
}

#[test]
fn canonical_nested_value_and_receipt_spans_survive_restart_exactly() {
    let database = TestDatabase::new("nested-spans");
    let initial_bytes = b"\x48\x54\x41\x01nested\x00initial";
    let next_bytes = b"\x48\x54\x41\x01nested\x00next\xff";
    let receipt_bytes = b"\x48\x54\x41\x01opaque\x00receipt\xfe";
    let key;

    {
        let store = open(&database);
        store
            .initialize(snapshot(0, initial_bytes))
            .expect("store must initialize");
        let update = request(0, 1, next_bytes, b"nested-plan", receipt_bytes);
        key = update.receipt_key();
        store
            .compare_and_swap(update)
            .expect("nested update must apply");
    }

    let reopened = open(&database);
    let loaded = reopened
        .load()
        .expect("load must work")
        .expect("snapshot must exist");
    assert_eq!(loaded.value().bytes(), next_bytes);
    assert_eq!(loaded.value().digest(), digest_for(next_bytes));
    let loaded_receipt = reopened
        .receipt(key)
        .expect("receipt lookup must work")
        .expect("receipt must exist");
    assert_eq!(loaded_receipt.receipt().bytes(), receipt_bytes);
}
