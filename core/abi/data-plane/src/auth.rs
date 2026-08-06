use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU64;

pub const SIGNED_DEVICE_PROFILE: &str = "hoplite-signed-device/1";
pub const APPLICATION_IDENTITY_PROFILE: &str = "hoplite-application-identity/1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceHandle(NonZeroU64);

impl ResourceHandle {
    pub fn new(value: u64) -> Result<Self, ResourceHandleError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(ResourceHandleError::Zero)
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceHandleError {
    Zero,
}

impl fmt::Display for ResourceHandleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "resource handles are server-assigned non-zero integers")
    }
}

impl std::error::Error for ResourceHandleError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedDeviceRequest<'a> {
    pub method: &'a str,
    pub target: &'a str,
    pub authority: &'a str,
    pub content_digest: &'a str,
    pub timestamp: i64,
    pub nonce: &'a str,
    pub key_id: &'a str,
    pub signature: &'a str,
}

impl SignedDeviceRequest<'_> {
    pub fn validate(&self) -> Result<(), SignedDeviceError> {
        if self.method.is_empty()
            || !self
                .method
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte == b'-')
        {
            return Err(SignedDeviceError::InvalidMethod);
        }
        if !self.target.starts_with('/')
            || self.target.contains("://")
            || self.target.contains('\\')
            || self.target.contains('\0')
            || self.target.bytes().any(|byte| byte.is_ascii_whitespace())
        {
            return Err(SignedDeviceError::InvalidTarget);
        }
        if self.authority.is_empty()
            || self.authority.contains('/')
            || self.authority.contains('@')
            || self
                .authority
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte == 0)
        {
            return Err(SignedDeviceError::InvalidAuthority);
        }
        if !valid_sha256(self.content_digest) {
            return Err(SignedDeviceError::InvalidContentDigest);
        }
        if self.nonce.len() < 16 {
            return Err(SignedDeviceError::InvalidNonce);
        }
        if self.key_id.is_empty() || self.key_id.bytes().any(|byte| byte.is_ascii_whitespace()) {
            return Err(SignedDeviceError::InvalidKeyId);
        }
        if self.signature.len() < 16 {
            return Err(SignedDeviceError::InvalidSignature);
        }
        Ok(())
    }

    pub fn signing_input(&self) -> Result<String, SignedDeviceError> {
        self.validate()?;
        Ok(format!(
            "{SIGNED_DEVICE_PROFILE}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            self.method,
            self.target,
            self.authority,
            self.content_digest,
            self.timestamp,
            self.nonce,
            self.key_id
        ))
    }
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignedDeviceError {
    InvalidMethod,
    InvalidTarget,
    InvalidAuthority,
    InvalidContentDigest,
    InvalidNonce,
    InvalidKeyId,
    InvalidSignature,
    VerificationFailed,
    Provider(String),
}

impl fmt::Display for SignedDeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMethod => write!(formatter, "invalid signed-device method"),
            Self::InvalidTarget => write!(formatter, "invalid signed-device request target"),
            Self::InvalidAuthority => write!(formatter, "invalid signed-device authority"),
            Self::InvalidContentDigest => write!(formatter, "invalid signed-device content digest"),
            Self::InvalidNonce => write!(formatter, "invalid signed-device nonce"),
            Self::InvalidKeyId => write!(formatter, "invalid signed-device key id"),
            Self::InvalidSignature => write!(formatter, "invalid signed-device signature"),
            Self::VerificationFailed => write!(formatter, "signed-device verification failed"),
            Self::Provider(message) => write!(formatter, "signed-device provider failed: {message}"),
        }
    }
}

impl std::error::Error for SignedDeviceError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedDevicePrincipal {
    pub subject: String,
    pub realm: String,
    pub device_id: String,
    pub key_id: String,
    pub provider: String,
    pub claims: BTreeMap<String, String>,
}

pub trait SignedDeviceProvider {
    fn authenticate(
        &mut self,
        request: &SignedDeviceRequest<'_>,
    ) -> Result<SignedDevicePrincipal, SignedDeviceError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationIdentity {
    pub profile: &'static str,
    pub subject: String,
    pub realm: String,
    pub device_id: String,
    pub key_id: String,
    pub provider: String,
    pub application_id: String,
    pub application_version: String,
    pub publisher: String,
    pub lock_digest: String,
    pub claims: BTreeMap<String, String>,
}

impl ApplicationIdentity {
    pub fn project(principal: &SignedDevicePrincipal) -> Result<Self, ProjectionError> {
        if principal.realm != "application" {
            return Err(ProjectionError::WrongRealm(principal.realm.clone()));
        }
        let application_id = required_claim(principal, "application/id")?;
        let application_version = required_claim(principal, "application/version")?;
        let publisher = required_claim(principal, "application/publisher")?;
        let lock_digest = required_claim(principal, "application/lock-digest")?;
        if !valid_sha256(&lock_digest) {
            return Err(ProjectionError::InvalidLockDigest);
        }

        const ALLOWED: &[&str] = &[
            "application/id",
            "application/version",
            "application/publisher",
            "application/lock-digest",
            "application/namespace",
            "application/collection",
            "application/operations",
            "device/label",
        ];
        let claims = principal
            .claims
            .iter()
            .filter(|(key, _)| ALLOWED.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();

        Ok(Self {
            profile: APPLICATION_IDENTITY_PROFILE,
            subject: principal.subject.clone(),
            realm: principal.realm.clone(),
            device_id: principal.device_id.clone(),
            key_id: principal.key_id.clone(),
            provider: principal.provider.clone(),
            application_id,
            application_version,
            publisher,
            lock_digest,
            claims,
        })
    }
}

fn required_claim(
    principal: &SignedDevicePrincipal,
    name: &'static str,
) -> Result<String, ProjectionError> {
    principal
        .claims
        .get(name)
        .filter(|value| !value.is_empty())
        .cloned()
        .ok_or(ProjectionError::MissingClaim(name))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectionError {
    WrongRealm(String),
    MissingClaim(&'static str),
    InvalidLockDigest,
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongRealm(realm) => write!(
                formatter,
                "cannot project realm {realm:?} into an application identity"
            ),
            Self::MissingClaim(name) => write!(formatter, "missing application claim {name}"),
            Self::InvalidLockDigest => write!(formatter, "invalid application lock digest"),
        }
    }
}

impl std::error::Error for ProjectionError {}
