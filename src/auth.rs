use crate::auth_policy::{AuthPolicy, ChallengeDecision, RefreshDecision};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};
use std::fs;
use std::ops::{Deref, DerefMut};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const BOOTSTRAP_TTL_SECONDS: i64 = 900;
const CHALLENGE_TTL_SECONDS: i64 = 300;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Principal {
    pub id: String,
    pub realm: String,
    pub session_id: String,
    pub device_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub session_id: String,
    pub access_expires_at: i64,
    pub refresh_expires_at: i64,
}

pub struct Store {
    path: PathBuf,
    connection: Connection,
}

pub struct Service {
    store: Store,
    policy: AuthPolicy,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        let in_memory = path == Path::new(":memory:");
        if !in_memory {
            let parent = path
                .parent()
                .ok_or("authentication store path has no parent")?;
            fs::create_dir_all(parent).map_err(io)?;
            set_private_directory(parent)?;
        }
        let connection = Connection::open(&path)
            .map_err(|error| format!("cannot open auth store {}: {error}", path.display()))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA foreign_keys=ON;
                 CREATE TABLE IF NOT EXISTS users (
                   id TEXT PRIMARY KEY,
                   realm TEXT NOT NULL,
                   created_at INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS devices (
                   id TEXT PRIMARY KEY,
                   user_id TEXT NOT NULL REFERENCES users(id),
                   public_key BLOB NOT NULL UNIQUE,
                   revoked_at INTEGER
                 );
                 CREATE TABLE IF NOT EXISTS bootstrap_tokens (
                   token_hash BLOB PRIMARY KEY,
                   expires_at INTEGER NOT NULL,
                   used_at INTEGER
                 );
                 CREATE TABLE IF NOT EXISTS challenges (
                   id TEXT PRIMARY KEY,
                   realm TEXT NOT NULL,
                   public_key BLOB NOT NULL,
                   nonce BLOB NOT NULL,
                   expires_at INTEGER NOT NULL,
                   used_at INTEGER
                 );
                 CREATE TABLE IF NOT EXISTS sessions (
                   id TEXT PRIMARY KEY,
                   user_id TEXT NOT NULL REFERENCES users(id),
                   device_id TEXT NOT NULL REFERENCES devices(id),
                   realm TEXT NOT NULL,
                   access_hash BLOB NOT NULL UNIQUE,
                   access_expires_at INTEGER NOT NULL,
                   refresh_expires_at INTEGER NOT NULL,
                   revoked_at INTEGER
                 );
                 CREATE TABLE IF NOT EXISTS refresh_tokens (
                   token_hash BLOB PRIMARY KEY,
                   session_id TEXT NOT NULL REFERENCES sessions(id),
                   issued_at INTEGER NOT NULL,
                   used_at INTEGER
                 );
                 CREATE TABLE IF NOT EXISTS audit_events (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   occurred_at INTEGER NOT NULL,
                   kind TEXT NOT NULL,
                   realm TEXT,
                   subject_id TEXT,
                   detail TEXT NOT NULL
                 );",
            )
            .map_err(db)?;
        if !in_memory {
            set_private_file(&path)?;
        }
        Ok(Self { path, connection })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn initialize(&mut self) -> Result<Option<String>, String> {
        let now = now()?;
        let existing: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM users WHERE realm = 'management'",
                [],
                |row| row.get(0),
            )
            .map_err(db)?;
        if existing > 0 {
            return Ok(None);
        }
        self.connection
            .execute("DELETE FROM bootstrap_tokens WHERE used_at IS NULL", [])
            .map_err(db)?;
        let token = random_token::<32>()?;
        self.connection
            .execute(
                "INSERT INTO bootstrap_tokens(token_hash, expires_at) VALUES (?1, ?2)",
                params![token_hash(&token), now + BOOTSTRAP_TTL_SECONDS],
            )
            .map_err(db)?;
        self.audit(
            "auth.bootstrap.created",
            Some("management"),
            None,
            "single-use",
        )?;
        Ok(Some(token))
    }

    pub fn enroll_management_device(
        &mut self,
        bootstrap_token: &str,
        public_key_hex: &str,
    ) -> Result<Principal, String> {
        let now = now()?;
        let public_key = decode_fixed::<32>(public_key_hex, "Ed25519 public key")?;
        VerifyingKey::from_bytes(&public_key)
            .map_err(|_| "invalid Ed25519 public key".to_string())?;
        let transaction = self.connection.transaction().map_err(db)?;
        let expires_at = transaction
            .query_row(
                "SELECT expires_at FROM bootstrap_tokens
                 WHERE token_hash = ?1 AND used_at IS NULL",
                params![token_hash(bootstrap_token)],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(db)?
            .ok_or("invalid or already-used bootstrap token")?;
        if expires_at < now {
            return Err("bootstrap token has expired; run `hoplite auth init` again".into());
        }
        let user_id = format!("usr_{}", random_token::<12>()?);
        let device_id = format!("dev_{}", random_token::<12>()?);
        transaction
            .execute(
                "INSERT INTO users(id, realm, created_at) VALUES (?1, 'management', ?2)",
                params![user_id, now],
            )
            .map_err(db)?;
        transaction
            .execute(
                "INSERT INTO devices(id, user_id, public_key) VALUES (?1, ?2, ?3)",
                params![device_id, user_id, public_key.as_slice()],
            )
            .map_err(db)?;
        transaction
            .execute(
                "UPDATE bootstrap_tokens SET used_at = ?1 WHERE token_hash = ?2",
                params![now, token_hash(bootstrap_token)],
            )
            .map_err(db)?;
        insert_audit(
            &transaction,
            now,
            "auth.management.enrolled",
            Some("management"),
            Some(&user_id),
            &device_id,
        )?;
        transaction.commit().map_err(db)?;
        Ok(Principal {
            id: user_id,
            realm: "management".into(),
            session_id: String::new(),
            device_id,
        })
    }

    pub fn create_challenge(
        &mut self,
        realm: &str,
        public_key_hex: &str,
    ) -> Result<(String, String), String> {
        validate_realm(realm)?;
        let now = now()?;
        let public_key = decode_fixed::<32>(public_key_hex, "Ed25519 public key")?;
        VerifyingKey::from_bytes(&public_key)
            .map_err(|_| "invalid Ed25519 public key".to_string())?;
        let id = format!("chl_{}", random_token::<12>()?);
        let nonce = random_bytes::<32>()?;
        self.connection
            .execute(
                "INSERT INTO challenges(id, realm, public_key, nonce, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    realm,
                    public_key.as_slice(),
                    nonce.as_slice(),
                    now + CHALLENGE_TTL_SECONDS
                ],
            )
            .map_err(db)?;
        Ok((id, encode_hex(&nonce)))
    }

    pub fn exchange_challenge(
        &mut self,
        policy: &mut AuthPolicy,
        challenge_id: &str,
        signature_hex: &str,
        access_ttl_seconds: u32,
        refresh_ttl_seconds: u32,
    ) -> Result<SessionTokens, String> {
        let now = now()?;
        let signature_bytes = decode_fixed::<64>(signature_hex, "Ed25519 signature")?;
        let transaction = self.connection.transaction().map_err(db)?;
        let challenge = transaction
            .query_row(
                "SELECT realm, public_key, nonce, expires_at FROM challenges
                 WHERE id = ?1 AND used_at IS NULL",
                params![challenge_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(db)?
            .ok_or("invalid or already-used authentication challenge")?;
        match policy.consume_challenge(challenge_id, challenge.3, None, now)? {
            ChallengeDecision::Accept => {}
            ChallengeDecision::Used => {
                return Err("invalid or already-used authentication challenge".into())
            }
            ChallengeDecision::Expired => return Err("authentication challenge has expired".into()),
        }
        let public_key: [u8; 32] = challenge
            .1
            .try_into()
            .map_err(|_| "invalid stored public key")?;
        let verifying_key = VerifyingKey::from_bytes(&public_key)
            .map_err(|_| "invalid stored Ed25519 public key".to_string())?;
        verifying_key
            .verify(&challenge.2, &Signature::from_bytes(&signature_bytes))
            .map_err(|_| "authentication signature is invalid".to_string())?;
        let identity = transaction
            .query_row(
                "SELECT users.id, devices.id FROM devices
                 JOIN users ON users.id = devices.user_id
                 WHERE devices.public_key = ?1 AND devices.revoked_at IS NULL AND users.realm = ?2",
                params![public_key.as_slice(), challenge.0],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(db)?
            .ok_or("this key is not enrolled in the requested realm")?;
        transaction
            .execute(
                "UPDATE challenges SET used_at = ?1 WHERE id = ?2",
                params![now, challenge_id],
            )
            .map_err(db)?;
        let tokens = issue_session(
            &transaction,
            now,
            &challenge.0,
            &identity.0,
            &identity.1,
            access_ttl_seconds,
            refresh_ttl_seconds,
        )?;
        transaction.commit().map_err(db)?;
        Ok(tokens)
    }

    pub fn authenticate(&self, realm: &str, access_token: &str) -> Result<Principal, String> {
        let now = now()?;
        self.connection
            .query_row(
                "SELECT users.id, sessions.realm, sessions.id, devices.id
                 FROM sessions
                 JOIN users ON users.id = sessions.user_id
                 JOIN devices ON devices.id = sessions.device_id
                 WHERE sessions.access_hash = ?1 AND sessions.realm = ?2
                   AND sessions.access_expires_at >= ?3
                   AND sessions.revoked_at IS NULL AND devices.revoked_at IS NULL",
                params![token_hash(access_token), realm, now],
                |row| {
                    Ok(Principal {
                        id: row.get(0)?,
                        realm: row.get(1)?,
                        session_id: row.get(2)?,
                        device_id: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(db)?
            .ok_or("invalid, expired, or revoked access token".into())
    }

    pub fn rotate_refresh_token(
        &mut self,
        policy: &mut AuthPolicy,
        refresh_token: &str,
        access_ttl_seconds: u32,
        refresh_ttl_seconds: u32,
        reuse_interval_seconds: u32,
    ) -> Result<SessionTokens, String> {
        let now = now()?;
        let transaction = self.connection.transaction().map_err(db)?;
        let current = transaction
            .query_row(
                "SELECT refresh_tokens.session_id, refresh_tokens.used_at,
                        sessions.user_id, sessions.device_id, sessions.realm,
                        sessions.refresh_expires_at, sessions.revoked_at
                 FROM refresh_tokens
                 JOIN sessions ON sessions.id = refresh_tokens.session_id
                 WHERE refresh_tokens.token_hash = ?1",
                params![token_hash(refresh_token)],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(db)?
            .ok_or("invalid refresh token")?;
        match policy.rotate_refresh(
            &current.0,
            &token_hash(refresh_token),
            current.1,
            current.5,
            current.6,
            now,
            reuse_interval_seconds,
        )? {
            RefreshDecision::Reuse => {
                transaction
                    .execute(
                        "UPDATE sessions SET revoked_at = ?1 WHERE id = ?2",
                        params![now, current.0],
                    )
                    .map_err(db)?;
                insert_audit(
                    &transaction,
                    now,
                    "auth.refresh.reuse-detected",
                    Some(&current.4),
                    Some(&current.2),
                    &current.0,
                )?;
                transaction.commit().map_err(db)?;
                return Err("refresh token reuse detected; session revoked".into());
            }
            RefreshDecision::Retry => {
                return Err(
                    "refresh token was already rotated; retry with the replacement token".into(),
                )
            }
            RefreshDecision::Revoked | RefreshDecision::Expired => {
                return Err("expired or revoked refresh token".into())
            }
            RefreshDecision::Accept => {}
        }
        transaction
            .execute(
                "UPDATE refresh_tokens SET used_at = ?1 WHERE token_hash = ?2",
                params![now, token_hash(refresh_token)],
            )
            .map_err(db)?;
        let access_token = format!("hpa_{}", random_token::<32>()?);
        let replacement_refresh = format!("hpr_{}", random_token::<32>()?);
        let access_expires_at = now + i64::from(access_ttl_seconds);
        let refresh_expires_at = now + i64::from(refresh_ttl_seconds);
        transaction
            .execute(
                "UPDATE sessions
                 SET access_hash = ?1, access_expires_at = ?2, refresh_expires_at = ?3
                 WHERE id = ?4",
                params![
                    token_hash(&access_token),
                    access_expires_at,
                    refresh_expires_at,
                    current.0
                ],
            )
            .map_err(db)?;
        transaction
            .execute(
                "INSERT INTO refresh_tokens(token_hash, session_id, issued_at) VALUES (?1, ?2, ?3)",
                params![token_hash(&replacement_refresh), current.0, now],
            )
            .map_err(db)?;
        insert_audit(
            &transaction,
            now,
            "auth.refresh.rotated",
            Some(&current.4),
            Some(&current.2),
            &current.0,
        )?;
        transaction.commit().map_err(db)?;
        Ok(SessionTokens {
            access_token,
            refresh_token: replacement_refresh,
            session_id: current.0,
            access_expires_at,
            refresh_expires_at,
        })
    }

    pub fn revoke_session(&mut self, session_id: &str) -> Result<bool, String> {
        let changed = self
            .connection
            .execute(
                "UPDATE sessions SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
                params![now()?, session_id],
            )
            .map_err(db)?;
        Ok(changed == 1)
    }

    fn audit(
        &self,
        kind: &str,
        realm: Option<&str>,
        subject: Option<&str>,
        detail: &str,
    ) -> Result<(), String> {
        insert_audit(&self.connection, now()?, kind, realm, subject, detail)
    }
}

impl Service {
    pub fn open_for(
        path: impl AsRef<Path>,
        composition: &crate::platform::AuthComposition,
    ) -> Result<Self, String> {
        if composition.store_package != crate::platform::SQLITE_STORE_PACKAGE
            || composition.store_export != crate::platform::STORE_EXPORT
        {
            return Err(format!(
                "authentication store adapter {} :{} is resolved but not installed",
                composition.store_package, composition.store_export
            ));
        }
        crate::store_adapter::validate(composition)?;
        Ok(Self {
            store: Store::open(path)?,
            policy: AuthPolicy::new(composition)?,
        })
    }

    pub fn exchange_challenge(
        &mut self,
        challenge_id: &str,
        signature_hex: &str,
        access_ttl_seconds: u32,
        refresh_ttl_seconds: u32,
    ) -> Result<SessionTokens, String> {
        self.store.exchange_challenge(
            &mut self.policy,
            challenge_id,
            signature_hex,
            access_ttl_seconds,
            refresh_ttl_seconds,
        )
    }

    pub fn rotate_refresh_token(
        &mut self,
        refresh_token: &str,
        access_ttl_seconds: u32,
        refresh_ttl_seconds: u32,
        reuse_interval_seconds: u32,
    ) -> Result<SessionTokens, String> {
        self.store.rotate_refresh_token(
            &mut self.policy,
            refresh_token,
            access_ttl_seconds,
            refresh_ttl_seconds,
            reuse_interval_seconds,
        )
    }
}

impl Deref for Service {
    type Target = Store;

    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

impl DerefMut for Service {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.store
    }
}

fn issue_session(
    transaction: &Transaction<'_>,
    now: i64,
    realm: &str,
    user_id: &str,
    device_id: &str,
    access_ttl_seconds: u32,
    refresh_ttl_seconds: u32,
) -> Result<SessionTokens, String> {
    let session_id = format!("ses_{}", random_token::<12>()?);
    let access_token = format!("hpa_{}", random_token::<32>()?);
    let refresh_token = format!("hpr_{}", random_token::<32>()?);
    let access_expires_at = now + i64::from(access_ttl_seconds);
    let refresh_expires_at = now + i64::from(refresh_ttl_seconds);
    transaction
        .execute(
            "INSERT INTO sessions
             (id, user_id, device_id, realm, access_hash, access_expires_at, refresh_expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                session_id,
                user_id,
                device_id,
                realm,
                token_hash(&access_token),
                access_expires_at,
                refresh_expires_at
            ],
        )
        .map_err(db)?;
    transaction
        .execute(
            "INSERT INTO refresh_tokens(token_hash, session_id, issued_at) VALUES (?1, ?2, ?3)",
            params![token_hash(&refresh_token), session_id, now],
        )
        .map_err(db)?;
    insert_audit(
        transaction,
        now,
        "auth.session.created",
        Some(realm),
        Some(user_id),
        &session_id,
    )?;
    Ok(SessionTokens {
        access_token,
        refresh_token,
        session_id,
        access_expires_at,
        refresh_expires_at,
    })
}

fn insert_audit(
    connection: &Connection,
    occurred_at: i64,
    kind: &str,
    realm: Option<&str>,
    subject: Option<&str>,
    detail: &str,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO audit_events(occurred_at, kind, realm, subject_id, detail)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![occurred_at, kind, realm, subject, detail],
        )
        .map_err(db)?;
    Ok(())
}

fn validate_realm(realm: &str) -> Result<(), String> {
    if realm.is_empty() || realm.chars().any(|character| character.is_whitespace()) {
        Err("authentication realm must be a non-empty identifier".into())
    } else {
        Ok(())
    }
}

fn random_token<const N: usize>() -> Result<String, String> {
    random_bytes::<N>().map(|bytes| encode_hex(&bytes))
}

fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0_u8; N];
    getrandom::getrandom(&mut bytes)
        .map_err(|error| format!("secure randomness unavailable: {error}"))?;
    Ok(bytes)
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn decode_fixed<const N: usize>(value: &str, label: &str) -> Result<[u8; N], String> {
    if value.len() != N * 2 {
        return Err(format!("{label} must be {} hexadecimal characters", N * 2));
    }
    let mut output = [0_u8; N];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let text =
            std::str::from_utf8(chunk).map_err(|_| format!("{label} must be hexadecimal"))?;
        output[index] =
            u8::from_str_radix(text, 16).map_err(|_| format!("{label} must be hexadecimal"))?;
    }
    Ok(output)
}

fn now() -> Result<i64, String> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before the Unix epoch".to_string())?;
    i64::try_from(duration.as_secs()).map_err(|_| "system clock is out of range".into())
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), String> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io)
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<(), String> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(io)
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn db(error: rusqlite::Error) -> String {
    format!("authentication store error: {error}")
}

fn io(error: std::io::Error) -> String {
    format!("authentication store I/O error: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn store() -> Service {
        Service::open_for(
            ":memory:",
            &crate::platform::Config::default()
                .auth_composition()
                .unwrap(),
        )
        .unwrap()
    }

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7_u8; 32])
    }

    #[test]
    fn bootstrap_is_single_use_and_enrolls_management_key() {
        let mut store = store();
        let token = store.initialize().unwrap().unwrap();
        let key = signing_key();
        let public_key = encode_hex(key.verifying_key().as_bytes());
        let principal = store.enroll_management_device(&token, &public_key).unwrap();
        assert_eq!(principal.realm, "management");
        assert!(store
            .enroll_management_device(&token, &public_key)
            .unwrap_err()
            .contains("already-used"));
        assert_eq!(store.initialize().unwrap(), None);
    }

    #[test]
    fn signed_challenge_issues_and_revokes_a_realm_bound_session() {
        let mut store = store();
        let token = store.initialize().unwrap().unwrap();
        let key = signing_key();
        let public_key = encode_hex(key.verifying_key().as_bytes());
        store.enroll_management_device(&token, &public_key).unwrap();
        let (challenge_id, nonce) = store.create_challenge("management", &public_key).unwrap();
        let nonce = decode_fixed::<32>(&nonce, "nonce").unwrap();
        let signature = encode_hex(&key.sign(&nonce).to_bytes());
        let tokens = store
            .exchange_challenge(&challenge_id, &signature, 900, 3600)
            .unwrap();
        let principal = store
            .authenticate("management", &tokens.access_token)
            .unwrap();
        assert_eq!(principal.session_id, tokens.session_id);
        assert!(store
            .authenticate("application", &tokens.access_token)
            .is_err());
        assert!(store.revoke_session(&tokens.session_id).unwrap());
        assert!(store
            .authenticate("management", &tokens.access_token)
            .is_err());
    }

    #[test]
    fn challenge_is_single_use_and_signature_is_verified() {
        let mut store = store();
        let token = store.initialize().unwrap().unwrap();
        let key = signing_key();
        let public_key = encode_hex(key.verifying_key().as_bytes());
        store.enroll_management_device(&token, &public_key).unwrap();
        let (challenge_id, nonce) = store.create_challenge("management", &public_key).unwrap();
        let nonce = decode_fixed::<32>(&nonce, "nonce").unwrap();
        let invalid = encode_hex(&SigningKey::from_bytes(&[8_u8; 32]).sign(&nonce).to_bytes());
        assert!(store
            .exchange_challenge(&challenge_id, &invalid, 900, 3600)
            .unwrap_err()
            .contains("invalid"));
        let valid = encode_hex(&key.sign(&nonce).to_bytes());
        store
            .exchange_challenge(&challenge_id, &valid, 900, 3600)
            .unwrap();
        assert!(store
            .exchange_challenge(&challenge_id, &valid, 900, 3600)
            .unwrap_err()
            .contains("already-used"));
    }

    #[test]
    fn refresh_rotation_invalidates_old_access_and_never_returns_the_same_token() {
        let mut store = store();
        let token = store.initialize().unwrap().unwrap();
        let key = signing_key();
        let public_key = encode_hex(key.verifying_key().as_bytes());
        store.enroll_management_device(&token, &public_key).unwrap();
        let (challenge_id, nonce) = store.create_challenge("management", &public_key).unwrap();
        let nonce = decode_fixed::<32>(&nonce, "nonce").unwrap();
        let first = store
            .exchange_challenge(
                &challenge_id,
                &encode_hex(&key.sign(&nonce).to_bytes()),
                900,
                3600,
            )
            .unwrap();
        let second = store
            .rotate_refresh_token(&first.refresh_token, 900, 3600, 10)
            .unwrap();
        assert_ne!(first.access_token, second.access_token);
        assert_ne!(first.refresh_token, second.refresh_token);
        assert!(store
            .authenticate("management", &first.access_token)
            .is_err());
        assert!(store
            .authenticate("management", &second.access_token)
            .is_ok());
        assert!(store
            .rotate_refresh_token(&first.refresh_token, 900, 3600, 10)
            .unwrap_err()
            .contains("already rotated"));
    }
}
