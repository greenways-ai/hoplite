#![forbid(unsafe_code)]

//! SQLite implementation of Hoplite's application-neutral `hara.store`
//! mechanics.
//!
//! The driver persists canonical value bytes and opaque receipt bytes. It does
//! not parse either value, and it contains no Tahto state, transaction, receipt,
//! authorization, or recovery semantics.

use hoplite_value_store::{
    ApplyStatus, CanonicalValue, CommitReceipt, CompareAndSwap, Digest, DigestVerifier,
    OpaqueReceipt, OpaqueValueStore, Snapshot, StoreError, StoreLimits, MAX_REVISION,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest as ShaDigest, Sha256};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

pub const SCHEMA_VERSION: i64 = 1;
pub const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const INITIAL_SCHEMA: &str = r#"
CREATE TABLE value_snapshot (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  revision INTEGER NOT NULL CHECK (revision >= 0),
  value BLOB NOT NULL CHECK (length(value) > 0),
  value_digest TEXT NOT NULL CHECK (
    length(value_digest) = 71
    AND substr(value_digest, 1, 7) = 'sha256:'
    AND substr(value_digest, 8) NOT GLOB '*[^0-9a-f]*'
  )
) STRICT;

CREATE TABLE value_receipts (
  receipt_key TEXT PRIMARY KEY CHECK (
    length(receipt_key) = 71
    AND substr(receipt_key, 1, 7) = 'sha256:'
    AND substr(receipt_key, 8) NOT GLOB '*[^0-9a-f]*'
  ),
  expected_revision INTEGER NOT NULL CHECK (expected_revision >= 0),
  revision INTEGER NOT NULL UNIQUE CHECK (
    revision > 0
    AND revision = expected_revision + 1
  ),
  value BLOB NOT NULL CHECK (length(value) > 0),
  value_digest TEXT NOT NULL CHECK (
    length(value_digest) = 71
    AND substr(value_digest, 1, 7) = 'sha256:'
    AND substr(value_digest, 8) NOT GLOB '*[^0-9a-f]*'
  ),
  receipt BLOB NOT NULL CHECK (length(receipt) > 0)
) STRICT;

PRAGMA user_version = 1;
"#;

#[derive(Clone, Copy, Debug, Default)]
pub struct Sha256Verifier;

impl DigestVerifier for Sha256Verifier {
    fn sha256(&self, canonical_bytes: &[u8]) -> [u8; 32] {
        let digest = Sha256::digest(canonical_bytes);
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(&digest);
        bytes
    }
}

pub struct SqliteValueStore {
    path: PathBuf,
    connection: Mutex<Connection>,
    limits: StoreLimits,
}

impl fmt::Debug for SqliteValueStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteValueStore")
            .field("path", &self.path)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl SqliteValueStore {
    pub fn open(path: impl AsRef<Path>, limits: StoreLimits) -> Result<Self, StoreError> {
        let limits = limits.validate()?;
        let path = path.as_ref().to_path_buf();
        let connection =
            Connection::open(&path).map_err(|error| driver_error("sqlite-open", error))?;
        configure_connection(&connection)?;

        let store = Self {
            path,
            connection: Mutex::new(connection),
            limits,
        };
        store.ensure_schema()?;
        store.verify()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub const fn limits(&self) -> StoreLimits {
        self.limits
    }

    pub fn verify(&self) -> Result<(), StoreError> {
        let connection = self.lock_connection()?;
        verify_connection(&connection, self.limits)
    }

    fn ensure_schema(&self) -> Result<(), StoreError> {
        let connection = self.lock_connection()?;
        let version = schema_version(&connection)?;
        match version {
            0 => {
                let table_count: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master \
                         WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| driver_error("sqlite-schema-inspect", error))?;
                if table_count != 0 {
                    return Err(StoreError::driver(
                        "sqlite-unversioned-schema",
                        format!(
                            "database has {table_count} application tables but user_version is zero"
                        ),
                    ));
                }
                connection
                    .execute_batch(INITIAL_SCHEMA)
                    .map_err(|error| driver_error("sqlite-schema-create", error))?;
                Ok(())
            }
            SCHEMA_VERSION => Ok(()),
            unsupported => Err(StoreError::driver(
                "sqlite-schema-version",
                format!("schema version {unsupported} is unsupported; expected {SCHEMA_VERSION}"),
            )),
        }
    }

    fn lock_connection(&self) -> Result<MutexGuard<'_, Connection>, StoreError> {
        self.connection
            .lock()
            .map_err(|_| StoreError::driver("sqlite-lock", "connection lock is poisoned"))
    }
}

impl OpaqueValueStore for SqliteValueStore {
    fn load(&self) -> Result<Option<Snapshot>, StoreError> {
        let connection = self.lock_connection()?;
        load_snapshot(&connection, self.limits)
    }

    fn initialize(&self, snapshot: Snapshot) -> Result<Snapshot, StoreError> {
        let snapshot = normalize_snapshot(&snapshot, self.limits)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| driver_error("sqlite-transaction-begin", error))?;

        if let Some(current) = load_snapshot(&transaction, self.limits)? {
            if current == snapshot {
                transaction
                    .commit()
                    .map_err(|error| driver_error("sqlite-transaction-commit", error))?;
                return Ok(current);
            }
            return Err(StoreError::AlreadyInitialized {
                current_revision: current.revision(),
            });
        }

        let receipt_count: i64 = transaction
            .query_row("SELECT COUNT(*) FROM value_receipts", [], |row| row.get(0))
            .map_err(|error| driver_error("sqlite-receipt-count", error))?;
        if receipt_count != 0 {
            return Err(StoreError::driver(
                "sqlite-corrupt-orphan-receipts",
                format!("found {receipt_count} receipts without a snapshot"),
            ));
        }

        transaction
            .execute(
                "INSERT INTO value_snapshot \
                 (singleton, revision, value, value_digest) VALUES (1, ?1, ?2, ?3)",
                params![
                    to_sql_revision(snapshot.revision())?,
                    snapshot.value().bytes(),
                    snapshot.value().digest().to_string(),
                ],
            )
            .map_err(|error| driver_error("sqlite-initialize", error))?;
        transaction
            .commit()
            .map_err(|error| driver_error("sqlite-transaction-commit", error))?;
        Ok(snapshot)
    }

    fn compare_and_swap(&self, request: CompareAndSwap) -> Result<CommitReceipt, StoreError> {
        let request = normalize_compare_and_swap(&request, self.limits)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| driver_error("sqlite-transaction-begin", error))?;

        if let Some(existing) =
            load_commit_by_key(&transaction, request.receipt_key(), self.limits)?
        {
            if existing.matches(&request) {
                return existing.into_receipt(ApplyStatus::Replayed);
            }
            return Err(StoreError::ReceiptCollision {
                receipt_key: request.receipt_key(),
            });
        }

        if let Some(existing_key) = load_receipt_key_by_revision(&transaction, request.revision())?
        {
            return Err(StoreError::driver(
                "sqlite-revision-receipt-collision",
                format!(
                    "revision {} is already bound to receipt key {existing_key}",
                    request.revision()
                ),
            ));
        }

        let current = load_snapshot(&transaction, self.limits)?.ok_or(StoreError::Uninitialized)?;
        if current.revision() != request.expected_revision() {
            return Err(StoreError::StaleRevision {
                expected: request.expected_revision(),
                actual: current.revision(),
            });
        }

        let changed = transaction
            .execute(
                "UPDATE value_snapshot \
                 SET revision = ?1, value = ?2, value_digest = ?3 \
                 WHERE singleton = 1 AND revision = ?4",
                params![
                    to_sql_revision(request.revision())?,
                    request.value().bytes(),
                    request.value().digest().to_string(),
                    to_sql_revision(request.expected_revision())?,
                ],
            )
            .map_err(|error| driver_error("sqlite-snapshot-update", error))?;
        if changed != 1 {
            let actual = load_snapshot(&transaction, self.limits)?
                .map(|snapshot| snapshot.revision())
                .ok_or(StoreError::Uninitialized)?;
            return Err(StoreError::StaleRevision {
                expected: request.expected_revision(),
                actual,
            });
        }

        transaction
            .execute(
                "INSERT INTO value_receipts \
                 (receipt_key, expected_revision, revision, value, value_digest, receipt) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    request.receipt_key().to_string(),
                    to_sql_revision(request.expected_revision())?,
                    to_sql_revision(request.revision())?,
                    request.value().bytes(),
                    request.value().digest().to_string(),
                    request.receipt().bytes(),
                ],
            )
            .map_err(|error| driver_error("sqlite-receipt-insert", error))?;

        transaction
            .commit()
            .map_err(|error| driver_error("sqlite-transaction-commit", error))?;
        CommitReceipt::new(
            ApplyStatus::Applied,
            request.revision(),
            request.receipt_key(),
            request.receipt().clone(),
        )
    }

    fn receipt(&self, receipt_key: Digest) -> Result<Option<CommitReceipt>, StoreError> {
        let connection = self.lock_connection()?;
        load_commit_by_key(&connection, receipt_key, self.limits)?
            .map(|commit| commit.into_receipt(ApplyStatus::Replayed))
            .transpose()
    }
}

fn configure_connection(connection: &Connection) -> Result<(), StoreError> {
    connection
        .busy_timeout(DEFAULT_BUSY_TIMEOUT)
        .map_err(|error| driver_error("sqlite-busy-timeout", error))?;
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;\n\
             PRAGMA journal_mode = WAL;\n\
             PRAGMA synchronous = FULL;\n\
             PRAGMA temp_store = MEMORY;",
        )
        .map_err(|error| driver_error("sqlite-configure", error))?;
    Ok(())
}

fn schema_version(connection: &Connection) -> Result<i64, StoreError> {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| driver_error("sqlite-schema-version-read", error))
}

fn verify_connection(connection: &Connection, limits: StoreLimits) -> Result<(), StoreError> {
    let version = schema_version(connection)?;
    if version != SCHEMA_VERSION {
        return Err(StoreError::driver(
            "sqlite-schema-version",
            format!("schema version {version} is unsupported; expected {SCHEMA_VERSION}"),
        ));
    }

    let quick_check: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|error| driver_error("sqlite-quick-check", error))?;
    if quick_check != "ok" {
        return Err(StoreError::driver(
            "sqlite-quick-check",
            format!("database quick_check returned {quick_check}"),
        ));
    }

    let snapshot = load_snapshot(connection, limits)?;
    let commits = load_all_commits(connection, limits)?;
    match snapshot {
        None if commits.is_empty() => Ok(()),
        None => Err(StoreError::driver(
            "sqlite-corrupt-orphan-receipts",
            format!("found {} receipts without a snapshot", commits.len()),
        )),
        Some(snapshot) => {
            for commit in &commits {
                if commit.revision > snapshot.revision() {
                    return Err(StoreError::driver(
                        "sqlite-corrupt-future-receipt",
                        format!(
                            "receipt revision {} exceeds snapshot revision {}",
                            commit.revision,
                            snapshot.revision()
                        ),
                    ));
                }
                if commit.revision == snapshot.revision() && &commit.value != snapshot.value() {
                    return Err(StoreError::driver(
                        "sqlite-corrupt-current-receipt",
                        "current receipt value does not match the current snapshot",
                    ));
                }
            }
            Ok(())
        }
    }
}

fn normalize_snapshot(snapshot: &Snapshot, limits: StoreLimits) -> Result<Snapshot, StoreError> {
    let value = CanonicalValue::verify(
        snapshot.value().bytes().to_vec(),
        snapshot.value().digest(),
        &Sha256Verifier,
        limits,
    )?;
    Snapshot::new(snapshot.revision(), value)
}

fn normalize_compare_and_swap(
    request: &CompareAndSwap,
    limits: StoreLimits,
) -> Result<CompareAndSwap, StoreError> {
    let value = CanonicalValue::verify(
        request.value().bytes().to_vec(),
        request.value().digest(),
        &Sha256Verifier,
        limits,
    )?;
    let receipt = OpaqueReceipt::new(request.receipt().bytes().to_vec(), limits)?;
    CompareAndSwap::new(
        request.expected_revision(),
        request.revision(),
        value,
        request.receipt_key(),
        receipt,
    )
}

fn load_snapshot(
    connection: &Connection,
    limits: StoreLimits,
) -> Result<Option<Snapshot>, StoreError> {
    let raw = connection
        .query_row(
            "SELECT revision, value, value_digest \
             FROM value_snapshot WHERE singleton = 1",
            [],
            |row| {
                Ok(RawSnapshot {
                    revision: row.get(0)?,
                    value: row.get(1)?,
                    value_digest: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(|error| driver_error("sqlite-snapshot-load", error))?;
    raw.map(|snapshot| snapshot.decode(limits)).transpose()
}

fn load_commit_by_key(
    connection: &Connection,
    receipt_key: Digest,
    limits: StoreLimits,
) -> Result<Option<StoredCommit>, StoreError> {
    let raw = connection
        .query_row(
            "SELECT receipt_key, expected_revision, revision, value, value_digest, receipt \
             FROM value_receipts WHERE receipt_key = ?1",
            params![receipt_key.to_string()],
            raw_commit_from_row,
        )
        .optional()
        .map_err(|error| driver_error("sqlite-receipt-load", error))?;
    raw.map(|commit| commit.decode(limits)).transpose()
}

fn load_receipt_key_by_revision(
    connection: &Connection,
    revision: u64,
) -> Result<Option<String>, StoreError> {
    connection
        .query_row(
            "SELECT receipt_key FROM value_receipts WHERE revision = ?1",
            params![to_sql_revision(revision)?],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| driver_error("sqlite-revision-receipt-load", error))
}

fn load_all_commits(
    connection: &Connection,
    limits: StoreLimits,
) -> Result<Vec<StoredCommit>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT receipt_key, expected_revision, revision, value, value_digest, receipt \
             FROM value_receipts ORDER BY revision ASC",
        )
        .map_err(|error| driver_error("sqlite-receipts-prepare", error))?;
    let mut rows = statement
        .query([])
        .map_err(|error| driver_error("sqlite-receipts-query", error))?;
    let mut commits = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|error| driver_error("sqlite-receipts-next", error))?
    {
        commits.push(
            raw_commit_from_row(row)
                .map_err(|error| driver_error("sqlite-receipts-decode", error))?
                .decode(limits)?,
        );
    }
    Ok(commits)
}

fn raw_commit_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawCommit> {
    Ok(RawCommit {
        receipt_key: row.get(0)?,
        expected_revision: row.get(1)?,
        revision: row.get(2)?,
        value: row.get(3)?,
        value_digest: row.get(4)?,
        receipt: row.get(5)?,
    })
}

fn to_sql_revision(revision: u64) -> Result<i64, StoreError> {
    i64::try_from(revision).map_err(|_| StoreError::RevisionOutOfRange { revision })
}

fn from_sql_revision(revision: i64) -> Result<u64, StoreError> {
    u64::try_from(revision).map_err(|_| {
        StoreError::driver(
            "sqlite-corrupt-revision",
            format!("stored revision {revision} is negative"),
        )
    })
}

fn driver_error(code: &'static str, error: rusqlite::Error) -> StoreError {
    StoreError::driver(code, error.to_string())
}

struct RawSnapshot {
    revision: i64,
    value: Vec<u8>,
    value_digest: String,
}

impl RawSnapshot {
    fn decode(self, limits: StoreLimits) -> Result<Snapshot, StoreError> {
        let revision = from_sql_revision(self.revision)?;
        let digest = Digest::parse(&self.value_digest)?;
        let value = CanonicalValue::verify(self.value, digest, &Sha256Verifier, limits)?;
        Snapshot::new(revision, value)
    }
}

struct RawCommit {
    receipt_key: String,
    expected_revision: i64,
    revision: i64,
    value: Vec<u8>,
    value_digest: String,
    receipt: Vec<u8>,
}

impl RawCommit {
    fn decode(self, limits: StoreLimits) -> Result<StoredCommit, StoreError> {
        let receipt_key = Digest::parse(&self.receipt_key)?;
        let expected_revision = from_sql_revision(self.expected_revision)?;
        let revision = from_sql_revision(self.revision)?;
        if expected_revision == MAX_REVISION || revision != expected_revision.saturating_add(1) {
            return Err(StoreError::driver(
                "sqlite-corrupt-revision-step",
                format!(
                    "stored receipt revision {revision} is not one greater than {expected_revision}"
                ),
            ));
        }
        let value_digest = Digest::parse(&self.value_digest)?;
        let value = CanonicalValue::verify(self.value, value_digest, &Sha256Verifier, limits)?;
        let receipt = OpaqueReceipt::new(self.receipt, limits)?;
        Ok(StoredCommit {
            receipt_key,
            expected_revision,
            revision,
            value,
            receipt,
        })
    }
}

struct StoredCommit {
    receipt_key: Digest,
    expected_revision: u64,
    revision: u64,
    value: CanonicalValue,
    receipt: OpaqueReceipt,
}

impl StoredCommit {
    fn matches(&self, request: &CompareAndSwap) -> bool {
        self.expected_revision == request.expected_revision()
            && self.revision == request.revision()
            && &self.value == request.value()
            && self.receipt_key == request.receipt_key()
            && &self.receipt == request.receipt()
    }

    fn into_receipt(self, status: ApplyStatus) -> Result<CommitReceipt, StoreError> {
        CommitReceipt::new(status, self.revision, self.receipt_key, self.receipt)
    }
}

#[cfg(test)]
mod tests;
