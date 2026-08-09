#![forbid(unsafe_code)]

//! Trusted Ed25519 verification for Hoplite signed application requests.
//!
//! This crate implements only key lookup, request freshness, key lifecycle and
//! exact signature verification. Durable nonce and idempotency admission are a
//! separate transaction boundary so a verified signature can never imply that
//! a nonce was durably consumed.

use ed25519_dalek::{Signature, VerifyingKey};
use hoplite_data_plane_abi::{
    SignedDeviceError, SignedDevicePrincipal, SignedDeviceProvider, SignedDeviceRequest,
};
use std::collections::{btree_map::Entry, BTreeMap};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

pub const PROVIDER_ID: &str = "hoplite.signed-device.ed25519/1";
const MAX_CONFIG_TEXT_BYTES: usize = 256;
const MAX_CLAIMS: usize = 64;
const MAX_CLAIM_NAME_BYTES: usize = 128;
const MAX_CLAIM_VALUE_BYTES: usize = 1024;

pub trait Clock {
    fn unix_seconds(&self) -> Result<i64, ClockError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn unix_seconds(&self) -> Result<i64, ClockError> {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ClockError::BeforeUnixEpoch)?;
        i64::try_from(duration.as_secs()).map_err(|_| ClockError::OutOfRange)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockError {
    BeforeUnixEpoch,
    OutOfRange,
}

impl fmt::Display for ClockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BeforeUnixEpoch => formatter.write_str("clock is before the Unix epoch"),
            Self::OutOfRange => formatter.write_str("clock exceeds the signed timestamp range"),
        }
    }
}

impl std::error::Error for ClockError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FreshnessPolicy {
    pub max_past_seconds: i64,
    pub max_future_seconds: i64,
}

impl FreshnessPolicy {
    pub fn new(
        max_past_seconds: i64,
        max_future_seconds: i64,
    ) -> Result<Self, ConfigurationError> {
        if max_past_seconds < 0 || max_future_seconds < 0 {
            return Err(ConfigurationError::InvalidFreshnessPolicy);
        }
        Ok(Self {
            max_past_seconds,
            max_future_seconds,
        })
    }
}

impl Default for FreshnessPolicy {
    fn default() -> Self {
        Self {
            max_past_seconds: 300,
            max_future_seconds: 30,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyWindow {
    pub not_before: Option<i64>,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl KeyWindow {
    pub fn validate(self) -> Result<Self, ConfigurationError> {
        for value in [self.not_before, self.expires_at, self.revoked_at]
            .into_iter()
            .flatten()
        {
            if value <= 0 {
                return Err(ConfigurationError::InvalidKeyWindow);
            }
        }
        if matches!(
            (self.not_before, self.expires_at),
            (Some(start), Some(end)) if start > end
        ) {
            return Err(ConfigurationError::InvalidKeyWindow);
        }
        Ok(self)
    }
}

#[derive(Clone)]
pub struct KeyRecord {
    key_id: String,
    subject: String,
    realm: String,
    device_id: String,
    verifying_key: VerifyingKey,
    window: KeyWindow,
    claims: BTreeMap<String, String>,
}

impl KeyRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key_id: impl Into<String>,
        subject: impl Into<String>,
        realm: impl Into<String>,
        device_id: impl Into<String>,
        public_key: [u8; 32],
        window: KeyWindow,
        claims: BTreeMap<String, String>,
    ) -> Result<Self, ConfigurationError> {
        let key_id = key_id.into();
        let subject = subject.into();
        let realm = realm.into();
        let device_id = device_id.into();
        for (field, value) in [
            ("key-id", key_id.as_str()),
            ("subject", subject.as_str()),
            ("device-id", device_id.as_str()),
        ] {
            if !valid_config_text(value) {
                return Err(ConfigurationError::InvalidField(field));
            }
        }
        if !matches!(realm.as_str(), "application" | "management") {
            return Err(ConfigurationError::InvalidRealm);
        }
        if claims.len() > MAX_CLAIMS
            || claims.iter().any(|(name, value)| {
                !valid_claim(name, MAX_CLAIM_NAME_BYTES)
                    || !valid_claim(value, MAX_CLAIM_VALUE_BYTES)
            })
        {
            return Err(ConfigurationError::InvalidClaims);
        }
        let verifying_key = VerifyingKey::from_bytes(&public_key)
            .map_err(|_| ConfigurationError::InvalidPublicKey)?;
        Ok(Self {
            key_id,
            subject,
            realm,
            device_id,
            verifying_key,
            window: window.validate()?,
            claims,
        })
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn realm(&self) -> &str {
        &self.realm
    }

    pub fn window(&self) -> KeyWindow {
        self.window
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigurationError {
    InvalidField(&'static str),
    InvalidRealm,
    InvalidPublicKey,
    InvalidClaims,
    InvalidFreshnessPolicy,
    InvalidKeyWindow,
    DuplicateKey,
    EmptyKeySet,
}

impl ConfigurationError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidField(_) => "signed-device-config-field-invalid",
            Self::InvalidRealm => "signed-device-config-realm-invalid",
            Self::InvalidPublicKey => "signed-device-config-public-key-invalid",
            Self::InvalidClaims => "signed-device-config-claims-invalid",
            Self::InvalidFreshnessPolicy => "signed-device-config-freshness-invalid",
            Self::InvalidKeyWindow => "signed-device-config-key-window-invalid",
            Self::DuplicateKey => "signed-device-config-key-duplicate",
            Self::EmptyKeySet => "signed-device-config-key-set-empty",
        }
    }
}

impl fmt::Display for ConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField(field) => write!(formatter, "invalid signed-device {field}"),
            Self::InvalidRealm => formatter.write_str("invalid signed-device realm"),
            Self::InvalidPublicKey => formatter.write_str("invalid Ed25519 public key"),
            Self::InvalidClaims => formatter.write_str("invalid signed-device claims"),
            Self::InvalidFreshnessPolicy => {
                formatter.write_str("invalid signed-device freshness policy")
            }
            Self::InvalidKeyWindow => formatter.write_str("invalid signed-device key window"),
            Self::DuplicateKey => formatter.write_str("duplicate signed-device key id"),
            Self::EmptyKeySet => formatter.write_str("signed-device key set is empty"),
        }
    }
}

impl std::error::Error for ConfigurationError {}

pub struct Provider<C = SystemClock> {
    keys: BTreeMap<String, KeyRecord>,
    freshness: FreshnessPolicy,
    clock: C,
}

impl Provider<SystemClock> {
    pub fn system(
        records: impl IntoIterator<Item = KeyRecord>,
        freshness: FreshnessPolicy,
    ) -> Result<Self, ConfigurationError> {
        Self::new(records, freshness, SystemClock)
    }
}

impl<C> Provider<C>
where
    C: Clock,
{
    pub fn new(
        records: impl IntoIterator<Item = KeyRecord>,
        freshness: FreshnessPolicy,
        clock: C,
    ) -> Result<Self, ConfigurationError> {
        let freshness = FreshnessPolicy::new(
            freshness.max_past_seconds,
            freshness.max_future_seconds,
        )?;
        let mut keys = BTreeMap::new();
        for record in records {
            match keys.entry(record.key_id.clone()) {
                Entry::Vacant(slot) => {
                    slot.insert(record);
                }
                Entry::Occupied(_) => return Err(ConfigurationError::DuplicateKey),
            }
        }
        if keys.is_empty() {
            return Err(ConfigurationError::EmptyKeySet);
        }
        Ok(Self {
            keys,
            freshness,
            clock,
        })
    }

    pub fn key_count(&self) -> usize {
        self.keys.len()
    }

    pub fn freshness(&self) -> FreshnessPolicy {
        self.freshness
    }

    fn authenticate_at(
        &self,
        request: &SignedDeviceRequest<'_>,
        now: i64,
    ) -> Result<SignedDevicePrincipal, SignedDeviceError> {
        request.validate()?;
        if now <= 0 {
            return Err(SignedDeviceError::ClockUnavailable);
        }
        if request.timestamp < now.saturating_sub(self.freshness.max_past_seconds) {
            return Err(SignedDeviceError::StaleTimestamp);
        }
        if request.timestamp > now.saturating_add(self.freshness.max_future_seconds) {
            return Err(SignedDeviceError::FutureTimestamp);
        }

        let record = self
            .keys
            .get(request.key_id)
            .ok_or(SignedDeviceError::UnknownKey)?;
        if record
            .window
            .revoked_at
            .is_some_and(|revoked_at| revoked_at <= now)
        {
            return Err(SignedDeviceError::RevokedKey);
        }
        if record
            .window
            .not_before
            .is_some_and(|not_before| request.timestamp < not_before)
        {
            return Err(SignedDeviceError::KeyNotYetValid);
        }
        if record
            .window
            .expires_at
            .is_some_and(|expires_at| request.timestamp >= expires_at)
        {
            return Err(SignedDeviceError::KeyExpired);
        }

        let signature_bytes = decode_lower_hex::<64>(request.signature)
            .ok_or(SignedDeviceError::InvalidSignature)?;
        let signature = Signature::from_bytes(&signature_bytes);
        let signing_input = request.signing_input()?;
        record
            .verifying_key
            .verify_strict(signing_input.as_bytes(), &signature)
            .map_err(|_| SignedDeviceError::VerificationFailed)?;

        Ok(SignedDevicePrincipal {
            subject: record.subject.clone(),
            realm: record.realm.clone(),
            device_id: record.device_id.clone(),
            key_id: record.key_id.clone(),
            provider: PROVIDER_ID.to_owned(),
            claims: record.claims.clone(),
        })
    }
}

impl<C> SignedDeviceProvider for Provider<C>
where
    C: Clock,
{
    fn authenticate(
        &mut self,
        request: &SignedDeviceRequest<'_>,
    ) -> Result<SignedDevicePrincipal, SignedDeviceError> {
        let now = self
            .clock
            .unix_seconds()
            .map_err(|_| SignedDeviceError::ClockUnavailable)?;
        self.authenticate_at(request, now)
    }
}

fn valid_config_text(value: &str) -> bool {
    valid_claim(value, MAX_CONFIG_TEXT_BYTES)
}

fn valid_claim(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.bytes().all(|byte| byte.is_ascii_graphic())
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
        output[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Some(output)
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}
