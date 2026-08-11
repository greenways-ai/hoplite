#![forbid(unsafe_code)]

//! Production worker ingress for signed Hoplite application requests.

use hoplite_data_plane_abi::{
    ApplicationRequestExpectation, SignedDeviceRequest, SIGNED_DEVICE_PROFILE,
};
use hoplite_signed_device_ed25519::{FreshnessPolicy, KeyRecord, KeyWindow, Provider, SystemClock};
use hoplite_signed_device_replay::{
    authenticate_and_admit_application_request, ApplicationIngressError, ReplayError, ReplayStatus,
    SqliteReplayStore,
};
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const KEYS_PROFILE: &str = "hoplite-signed-device-keys/0-alpha";
pub const REQUEST_PROFILE: &str = SIGNED_DEVICE_PROFILE;
pub const PROJECTED_IDENTITY_PROFILE: &str = "hoplite-application-ingress/0-alpha";
pub const KEYS_PATH_ENV: &str = "HOPLITE_SIGNED_DEVICE_KEYS_PATH";
pub const REPLAY_PATH_ENV: &str = "HOPLITE_SIGNED_DEVICE_REPLAY_PATH";

pub const PROFILE_HEADER: &str = "x-hoplite-signature-profile";
pub const CONTENT_DIGEST_HEADER: &str = "content-digest";
pub const TIMESTAMP_HEADER: &str = "x-hoplite-timestamp";
pub const NONCE_HEADER: &str = "x-hoplite-nonce";
pub const IDEMPOTENCY_HEADER: &str = "idempotency-key";
pub const KEY_ID_HEADER: &str = "x-hoplite-key-id";
pub const SIGNATURE_HEADER: &str = "x-hoplite-signature";
pub const AUTHORITY_HEADER: &str = "host";

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_KEYS: usize = 1024;
const ZERO_DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutePolicy {
    operation: String,
    application: String,
    namespace: String,
    collection: String,
}

impl RoutePolicy {
    pub fn new(
        operation: impl Into<String>,
        application: impl Into<String>,
        namespace: impl Into<String>,
        collection: impl Into<String>,
    ) -> Result<Self, ConfigurationError> {
        let policy = Self {
            operation: operation.into(),
            application: application.into(),
            namespace: namespace.into(),
            collection: collection.into(),
        };
        let expectation = ApplicationRequestExpectation {
            method: "GET",
            target: "/",
            authority: "localhost",
            content_digest: ZERO_DIGEST,
            operation: &policy.operation,
            application: &policy.application,
            namespace: &policy.namespace,
            collection: &policy.collection,
        };
        expectation
            .validate()
            .map_err(|_| ConfigurationError::InvalidRoute)?;
        Ok(policy)
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
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProjectedIdentity {
    profile: &'static str,
    subject: String,
    realm: String,
    device_id: String,
    key_id: String,
    application_id: String,
    application_version: String,
    publisher: String,
    lock_digest: String,
    claims: BTreeMap<String, String>,
    operation: String,
    application: String,
    namespace: String,
    collection: String,
    content_digest: String,
    timestamp: i64,
    replay_status: &'static str,
    request_fingerprint: String,
    admitted_at: i64,
}

impl ProjectedIdentity {
    pub const fn profile(&self) -> &'static str {
        self.profile
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

    pub fn application_id(&self) -> &str {
        &self.application_id
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

    pub fn claims(&self) -> &BTreeMap<String, String> {
        &self.claims
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

    pub const fn timestamp(&self) -> i64 {
        self.timestamp
    }

    pub const fn replay_status(&self) -> &'static str {
        self.replay_status
    }

    pub fn request_fingerprint(&self) -> &str {
        &self.request_fingerprint
    }

    pub const fn admitted_at(&self) -> i64 {
        self.admitted_at
    }
}

impl fmt::Debug for ProjectedIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectedIdentity")
            .field("profile", &self.profile)
            .field("device_id", &self.device_id)
            .field("key_id", &self.key_id)
            .field("operation", &self.operation)
            .field("application", &self.application)
            .field("namespace", &self.namespace)
            .field("collection", &self.collection)
            .field("replay_status", &self.replay_status)
            .field("request_fingerprint", &self.request_fingerprint)
            .field("admitted_at", &self.admitted_at)
            .finish_non_exhaustive()
    }
}

pub struct WorkerIngress {
    provider: Provider<SystemClock>,
    replay: SqliteReplayStore,
}

impl WorkerIngress {
    pub fn open(
        keys_path: impl AsRef<Path>,
        replay_path: impl AsRef<Path>,
    ) -> Result<Self, ConfigurationError> {
        let (records, freshness) = load_key_configuration(keys_path.as_ref())?;
        let provider =
            Provider::system(records, freshness).map_err(|_| ConfigurationError::Keys)?;
        let replay =
            SqliteReplayStore::open(replay_path).map_err(|_| ConfigurationError::Replay)?;
        Ok(Self { provider, replay })
    }

    pub fn from_environment() -> Result<Self, ConfigurationError> {
        let (keys, replay) = environment_paths()?.ok_or(ConfigurationError::MissingEnvironment)?;
        Self::open(keys, replay)
    }

    pub fn authenticate(
        &mut self,
        method: &str,
        target: &str,
        headers: &[(String, String)],
        route: &RoutePolicy,
    ) -> Result<ProjectedIdentity, IngressError> {
        let wire = WireFields::parse(headers)?;
        let request = SignedDeviceRequest {
            method,
            target,
            authority: &wire.authority,
            content_digest: &wire.content_digest,
            operation: route.operation(),
            application: route.application(),
            namespace: route.namespace(),
            collection: route.collection(),
            timestamp: wire.timestamp,
            nonce: &wire.nonce,
            idempotency_key: &wire.idempotency_key,
            key_id: &wire.key_id,
            signature: &wire.signature,
        };
        let expectation = ApplicationRequestExpectation {
            method,
            target,
            authority: &wire.authority,
            content_digest: &wire.content_digest,
            operation: route.operation(),
            application: route.application(),
            namespace: route.namespace(),
            collection: route.collection(),
        };
        let admitted_at = now().map_err(|_| IngressError::Unavailable)?;
        let admitted = authenticate_and_admit_application_request(
            &mut self.provider,
            &self.replay,
            &request,
            &expectation,
            admitted_at,
        )
        .map_err(IngressError::Application)?;

        let verified = admitted.verified();
        let identity = verified.identity();
        let replay = admitted.replay();
        let evidence = replay.evidence();
        let replay_status = match replay.status() {
            ReplayStatus::Applied => "applied",
            ReplayStatus::Replayed => "replayed",
        };
        Ok(ProjectedIdentity {
            profile: PROJECTED_IDENTITY_PROFILE,
            subject: identity.subject.clone(),
            realm: identity.realm.clone(),
            device_id: identity.device_id.clone(),
            key_id: identity.key_id.clone(),
            application_id: identity.application_id.clone(),
            application_version: identity.application_version.clone(),
            publisher: identity.publisher.clone(),
            lock_digest: identity.lock_digest.clone(),
            claims: identity.claims.clone(),
            operation: verified.operation().to_owned(),
            application: verified.application().to_owned(),
            namespace: verified.namespace().to_owned(),
            collection: verified.collection().to_owned(),
            content_digest: verified.content_digest().to_owned(),
            timestamp: verified.timestamp(),
            replay_status,
            request_fingerprint: evidence.fingerprint().as_str().to_owned(),
            admitted_at: evidence.admitted_at(),
        })
    }
}

impl fmt::Debug for WorkerIngress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkerIngress")
            .field("key_count", &self.provider.key_count())
            .field(
                "persistent_replay",
                &(self.replay.path() != Path::new(":memory:")),
            )
            .finish_non_exhaustive()
    }
}

/// Validate trusted worker configuration and initialize the replay schema before
/// Nginx forks workers. Returns `false` when signed ingress is not configured.
pub fn preflight_environment() -> Result<bool, ConfigurationError> {
    let Some((keys, replay)) = environment_paths()? else {
        return Ok(false);
    };
    let _ = WorkerIngress::open(keys, replay)?;
    Ok(true)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigurationError {
    MissingEnvironment,
    PartialEnvironment,
    Read,
    TooLarge,
    Json,
    Profile,
    Shape,
    Keys,
    Replay,
    InvalidRoute,
}

impl ConfigurationError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::MissingEnvironment => "signed-device-worker/environment-missing",
            Self::PartialEnvironment => "signed-device-worker/environment-partial",
            Self::Read => "signed-device-worker/config-read-failed",
            Self::TooLarge => "signed-device-worker/config-too-large",
            Self::Json => "signed-device-worker/config-json-invalid",
            Self::Profile => "signed-device-worker/config-profile-invalid",
            Self::Shape => "signed-device-worker/config-shape-invalid",
            Self::Keys => "signed-device-worker/config-keys-invalid",
            Self::Replay => "signed-device-worker/replay-open-failed",
            Self::InvalidRoute => "signed-device-worker/route-invalid",
        }
    }
}

impl fmt::Display for ConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("signed-device worker configuration is invalid")
    }
}

impl std::error::Error for ConfigurationError {}

#[derive(Debug)]
pub enum IngressError {
    MissingHeader(&'static str),
    DuplicateHeader(&'static str),
    InvalidHeader(&'static str),
    UnsupportedProfile,
    Application(ApplicationIngressError),
    Unavailable,
}

impl IngressError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::MissingHeader(_) => "signed-device-worker/header-missing",
            Self::DuplicateHeader(_) => "signed-device-worker/header-duplicate",
            Self::InvalidHeader(_) => "signed-device-worker/header-invalid",
            Self::UnsupportedProfile => "signed-device-worker/profile-unsupported",
            Self::Application(error) => error.code(),
            Self::Unavailable => "signed-device-worker/unavailable",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Self::Application(ApplicationIngressError::Replay(
                ReplayError::IdempotencyCollision | ReplayError::NonceReused,
            )) => 409,
            Self::Application(ApplicationIngressError::Replay(_)) | Self::Unavailable => 503,
            _ => 401,
        }
    }
}

impl fmt::Display for IngressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status() {
            409 => formatter.write_str("signed request conflicts with prior admission"),
            503 => formatter.write_str("signed request admission is unavailable"),
            _ => formatter.write_str("signed request authentication failed"),
        }
    }
}

impl std::error::Error for IngressError {}

struct WireFields {
    authority: String,
    content_digest: String,
    timestamp: i64,
    nonce: String,
    idempotency_key: String,
    key_id: String,
    signature: String,
}

impl WireFields {
    fn parse(headers: &[(String, String)]) -> Result<Self, IngressError> {
        let profile = unique_header(headers, PROFILE_HEADER)?;
        if profile != SIGNED_DEVICE_PROFILE {
            return Err(IngressError::UnsupportedProfile);
        }
        let timestamp = unique_header(headers, TIMESTAMP_HEADER)?
            .parse::<i64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or(IngressError::InvalidHeader(TIMESTAMP_HEADER))?;
        Ok(Self {
            authority: unique_header(headers, AUTHORITY_HEADER)?.to_owned(),
            content_digest: unique_header(headers, CONTENT_DIGEST_HEADER)?.to_owned(),
            timestamp,
            nonce: unique_header(headers, NONCE_HEADER)?.to_owned(),
            idempotency_key: unique_header(headers, IDEMPOTENCY_HEADER)?.to_owned(),
            key_id: unique_header(headers, KEY_ID_HEADER)?.to_owned(),
            signature: unique_header(headers, SIGNATURE_HEADER)?.to_owned(),
        })
    }
}

fn unique_header<'a>(
    headers: &'a [(String, String)],
    name: &'static str,
) -> Result<&'a str, IngressError> {
    let mut values = headers
        .iter()
        .filter(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str());
    let value = values.next().ok_or(IngressError::MissingHeader(name))?;
    if values.next().is_some() {
        return Err(IngressError::DuplicateHeader(name));
    }
    if value.is_empty()
        || value.len() > 8192
        || value.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
    {
        return Err(IngressError::InvalidHeader(name));
    }
    Ok(value)
}

fn environment_paths() -> Result<Option<(PathBuf, PathBuf)>, ConfigurationError> {
    match (
        env::var_os(KEYS_PATH_ENV).map(PathBuf::from),
        env::var_os(REPLAY_PATH_ENV).map(PathBuf::from),
    ) {
        (None, None) => Ok(None),
        (Some(keys), Some(replay))
            if !keys.as_os_str().is_empty() && !replay.as_os_str().is_empty() =>
        {
            Ok(Some((keys, replay)))
        }
        _ => Err(ConfigurationError::PartialEnvironment),
    }
}

fn load_key_configuration(
    path: &Path,
) -> Result<(Vec<KeyRecord>, FreshnessPolicy), ConfigurationError> {
    let metadata = fs::metadata(path).map_err(|_| ConfigurationError::Read)?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
        return Err(ConfigurationError::TooLarge);
    }
    let bytes = fs::read(path).map_err(|_| ConfigurationError::Read)?;
    let value: JsonValue = serde_json::from_slice(&bytes).map_err(|_| ConfigurationError::Json)?;
    let object = exact_object(&value, &["profile", "freshness", "keys"])?;
    if text(object, "profile")? != KEYS_PROFILE {
        return Err(ConfigurationError::Profile);
    }
    let freshness_object = exact_object(
        object.get("freshness").ok_or(ConfigurationError::Shape)?,
        &["max-past-seconds", "max-future-seconds"],
    )?;
    let freshness = FreshnessPolicy::new(
        integer(freshness_object, "max-past-seconds")?,
        integer(freshness_object, "max-future-seconds")?,
    )
    .map_err(|_| ConfigurationError::Keys)?;

    let keys = object
        .get("keys")
        .and_then(JsonValue::as_array)
        .ok_or(ConfigurationError::Shape)?;
    if keys.is_empty() || keys.len() > MAX_KEYS {
        return Err(ConfigurationError::Keys);
    }
    let records = keys.iter().map(parse_key).collect::<Result<Vec<_>, _>>()?;
    Ok((records, freshness))
}

fn parse_key(value: &JsonValue) -> Result<KeyRecord, ConfigurationError> {
    let object = exact_object(
        value,
        &[
            "key-id",
            "subject",
            "realm",
            "device-id",
            "public-key",
            "claims",
            "not-before",
            "expires-at",
            "revoked-at",
        ],
    )?;
    let claims_object = object
        .get("claims")
        .and_then(JsonValue::as_object)
        .ok_or(ConfigurationError::Shape)?;
    let claims = claims_object
        .iter()
        .map(|(name, value)| {
            value
                .as_str()
                .map(|value| (name.clone(), value.to_owned()))
                .ok_or(ConfigurationError::Shape)
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let public_key =
        decode_lower_hex::<32>(text(object, "public-key")?).ok_or(ConfigurationError::Keys)?;
    KeyRecord::new(
        text(object, "key-id")?,
        text(object, "subject")?,
        text(object, "realm")?,
        text(object, "device-id")?,
        public_key,
        KeyWindow {
            not_before: optional_integer(object, "not-before")?,
            expires_at: optional_integer(object, "expires-at")?,
            revoked_at: optional_integer(object, "revoked-at")?,
        },
        claims,
    )
    .map_err(|_| ConfigurationError::Keys)
}

fn exact_object<'a>(
    value: &'a JsonValue,
    allowed: &[&str],
) -> Result<&'a JsonMap<String, JsonValue>, ConfigurationError> {
    let object = value.as_object().ok_or(ConfigurationError::Shape)?;
    if object.keys().any(|name| !allowed.contains(&name.as_str()))
        || allowed.iter().any(|name| !object.contains_key(*name))
    {
        return Err(ConfigurationError::Shape);
    }
    Ok(object)
}

fn text<'a>(
    object: &'a JsonMap<String, JsonValue>,
    name: &str,
) -> Result<&'a str, ConfigurationError> {
    object
        .get(name)
        .and_then(JsonValue::as_str)
        .ok_or(ConfigurationError::Shape)
}

fn integer(object: &JsonMap<String, JsonValue>, name: &str) -> Result<i64, ConfigurationError> {
    object
        .get(name)
        .and_then(JsonValue::as_i64)
        .ok_or(ConfigurationError::Shape)
}

fn optional_integer(
    object: &JsonMap<String, JsonValue>,
    name: &str,
) -> Result<Option<i64>, ConfigurationError> {
    match object.get(name) {
        Some(JsonValue::Null) => Ok(None),
        Some(value) => value.as_i64().map(Some).ok_or(ConfigurationError::Shape),
        None => Err(ConfigurationError::Shape),
    }
}

fn decode_lower_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return None;
    }
    let mut output = [0_u8; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(output)
}

fn nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn now() -> Result<i64, ()> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ())?;
    i64::try_from(duration.as_secs()).map_err(|_| ())
}
