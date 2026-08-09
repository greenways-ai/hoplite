use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU64;

/// The pre-production profile retained only for migration documentation.
pub const LEGACY_SIGNED_DEVICE_PROFILE: &str = "hoplite-signed-device/1";
/// The closed production profile. Version 2 binds application coordinates and
/// an idempotency key in addition to the original HTTP request fields.
pub const SIGNED_DEVICE_PROFILE: &str = "hoplite-signed-device/2";
pub const APPLICATION_IDENTITY_PROFILE: &str = "hoplite-application-identity/1";
pub const VERIFIED_APPLICATION_REQUEST_PROFILE: &str =
    "hoplite-verified-application-request/1";

const MAX_METHOD_BYTES: usize = 32;
const MAX_TARGET_BYTES: usize = 8 * 1024;
const MAX_AUTHORITY_BYTES: usize = 255;
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_OPAQUE_TOKEN_BYTES: usize = 256;
const MAX_SIGNATURE_BYTES: usize = 512;
const MAX_PRINCIPAL_CLAIMS: usize = 64;
const MAX_CLAIM_VALUE_BYTES: usize = 1024;
const MAX_OPERATIONS: usize = 64;

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

/// One exact signed application-device request.
///
/// The signature itself is deliberately excluded from `signing_input`. Every
/// other field is line-delimited after validation has ruled out whitespace and
/// delimiter ambiguity in opaque and identifier fields.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SignedDeviceRequest<'a> {
    pub method: &'a str,
    pub target: &'a str,
    pub authority: &'a str,
    pub content_digest: &'a str,
    pub operation: &'a str,
    pub application: &'a str,
    pub namespace: &'a str,
    pub collection: &'a str,
    pub timestamp: i64,
    pub nonce: &'a str,
    pub idempotency_key: &'a str,
    pub key_id: &'a str,
    pub signature: &'a str,
}


impl fmt::Debug for SignedDeviceRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignedDeviceRequest")
            .field("method", &self.method)
            .field("target", &self.target)
            .field("authority", &self.authority)
            .field("content_digest", &self.content_digest)
            .field("operation", &self.operation)
            .field("application", &self.application)
            .field("namespace", &self.namespace)
            .field("collection", &self.collection)
            .field("timestamp", &self.timestamp)
            .field("nonce", &self.nonce)
            .field("idempotency_key", &self.idempotency_key)
            .field("key_id", &self.key_id)
            .field("signature", &"<redacted>")
            .finish()
    }
}

impl SignedDeviceRequest<'_> {
    /// Validate every signed field and the encoded signature.
    pub fn validate(&self) -> Result<(), SignedDeviceError> {
        self.validate_signing_fields()?;
        if !valid_opaque_token(self.signature, 16, MAX_SIGNATURE_BYTES) {
            return Err(SignedDeviceError::InvalidSignature);
        }
        Ok(())
    }

    /// Build the canonical signing bytes before a signature exists.
    ///
    /// The signature field is intentionally not inspected here. This allows a
    /// signer to construct a request with an empty signature, obtain the exact
    /// domain-separated bytes, and then attach the encoded signature.
    pub fn signing_input(&self) -> Result<String, SignedDeviceError> {
        self.validate_signing_fields()?;
        Ok(format!(
            "{SIGNED_DEVICE_PROFILE}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            self.method,
            self.target,
            self.authority,
            self.content_digest,
            self.operation,
            self.application,
            self.namespace,
            self.collection,
            self.timestamp,
            self.nonce,
            self.idempotency_key,
            self.key_id
        ))
    }

    fn validate_signing_fields(&self) -> Result<(), SignedDeviceError> {
        if self.method.is_empty()
            || self.method.len() > MAX_METHOD_BYTES
            || !self
                .method
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte == b'-')
        {
            return Err(SignedDeviceError::InvalidMethod);
        }
        if self.target.len() > MAX_TARGET_BYTES
            || !self.target.starts_with('/')
            || self.target.starts_with("//")
            || self.target.contains("://")
            || self.target.contains('\\')
            || self.target.contains('\0')
            || self.target.contains('#')
            || !self.target.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(SignedDeviceError::InvalidTarget);
        }
        if self.authority.is_empty()
            || self.authority.len() > MAX_AUTHORITY_BYTES
            || self.authority.contains('/')
            || self.authority.contains('@')
            || self.authority.contains('\\')
            || self
                .authority
                .bytes()
                .any(|byte| !byte.is_ascii_graphic())
        {
            return Err(SignedDeviceError::InvalidAuthority);
        }
        if !valid_sha256(self.content_digest) {
            return Err(SignedDeviceError::InvalidContentDigest);
        }
        if !valid_identifier(self.operation) {
            return Err(SignedDeviceError::InvalidOperation);
        }
        if !valid_identifier(self.application) {
            return Err(SignedDeviceError::InvalidApplication);
        }
        if !valid_identifier(self.namespace) {
            return Err(SignedDeviceError::InvalidNamespace);
        }
        if !valid_identifier(self.collection) {
            return Err(SignedDeviceError::InvalidCollection);
        }
        if self.timestamp <= 0 {
            return Err(SignedDeviceError::InvalidTimestamp);
        }
        if !valid_opaque_token(self.nonce, 16, MAX_OPAQUE_TOKEN_BYTES) {
            return Err(SignedDeviceError::InvalidNonce);
        }
        if !valid_opaque_token(self.idempotency_key, 16, MAX_OPAQUE_TOKEN_BYTES) {
            return Err(SignedDeviceError::InvalidIdempotencyKey);
        }
        if !valid_opaque_token(self.key_id, 1, MAX_OPAQUE_TOKEN_BYTES) {
            return Err(SignedDeviceError::InvalidKeyId);
        }
        Ok(())
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

fn valid_identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_IDENTIFIER_BYTES
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().copied().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b':' | b'/' | b'-')
        })
}

fn valid_opaque_token(value: &str, minimum: usize, maximum: usize) -> bool {
    (minimum..=maximum).contains(&value.len())
        && value.bytes().all(|byte| byte.is_ascii_graphic())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignedDeviceError {
    InvalidMethod,
    InvalidTarget,
    InvalidAuthority,
    InvalidContentDigest,
    InvalidOperation,
    InvalidApplication,
    InvalidNamespace,
    InvalidCollection,
    InvalidTimestamp,
    InvalidNonce,
    InvalidIdempotencyKey,
    InvalidKeyId,
    InvalidSignature,
    UnknownKey,
    StaleTimestamp,
    FutureTimestamp,
    RevokedKey,
    KeyNotYetValid,
    KeyExpired,
    ClockUnavailable,
    VerificationFailed,
    Provider,
}

impl SignedDeviceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidMethod => "hoplite.signed-device/invalid-method",
            Self::InvalidTarget => "hoplite.signed-device/invalid-target",
            Self::InvalidAuthority => "hoplite.signed-device/invalid-authority",
            Self::InvalidContentDigest => "hoplite.signed-device/invalid-content-digest",
            Self::InvalidOperation => "hoplite.signed-device/invalid-operation",
            Self::InvalidApplication => "hoplite.signed-device/invalid-application",
            Self::InvalidNamespace => "hoplite.signed-device/invalid-namespace",
            Self::InvalidCollection => "hoplite.signed-device/invalid-collection",
            Self::InvalidTimestamp => "hoplite.signed-device/invalid-timestamp",
            Self::InvalidNonce => "hoplite.signed-device/invalid-nonce",
            Self::InvalidIdempotencyKey => {
                "hoplite.signed-device/invalid-idempotency-key"
            }
            Self::InvalidKeyId => "hoplite.signed-device/invalid-key-id",
            Self::InvalidSignature => "hoplite.signed-device/invalid-signature",
            Self::UnknownKey => "hoplite.signed-device/unknown-key",
            Self::StaleTimestamp => "hoplite.signed-device/stale-timestamp",
            Self::FutureTimestamp => "hoplite.signed-device/future-timestamp",
            Self::RevokedKey => "hoplite.signed-device/revoked-key",
            Self::KeyNotYetValid => "hoplite.signed-device/key-not-yet-valid",
            Self::KeyExpired => "hoplite.signed-device/key-expired",
            Self::ClockUnavailable => "hoplite.signed-device/clock-unavailable",
            Self::VerificationFailed => "hoplite.signed-device/verification-failed",
            Self::Provider => "hoplite.signed-device/provider-failed",
        }
    }
}

impl fmt::Display for SignedDeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMethod => write!(formatter, "invalid signed-device method"),
            Self::InvalidTarget => write!(formatter, "invalid signed-device request target"),
            Self::InvalidAuthority => write!(formatter, "invalid signed-device authority"),
            Self::InvalidContentDigest => write!(formatter, "invalid signed-device content digest"),
            Self::InvalidOperation => write!(formatter, "invalid signed-device operation"),
            Self::InvalidApplication => write!(formatter, "invalid signed-device application"),
            Self::InvalidNamespace => write!(formatter, "invalid signed-device namespace"),
            Self::InvalidCollection => write!(formatter, "invalid signed-device collection"),
            Self::InvalidTimestamp => write!(formatter, "invalid signed-device timestamp"),
            Self::InvalidNonce => write!(formatter, "invalid signed-device nonce"),
            Self::InvalidIdempotencyKey => {
                write!(formatter, "invalid signed-device idempotency key")
            }
            Self::InvalidKeyId => write!(formatter, "invalid signed-device key id"),
            Self::InvalidSignature => write!(formatter, "invalid signed-device signature"),
            Self::UnknownKey => write!(formatter, "signed-device key is not configured"),
            Self::StaleTimestamp => write!(formatter, "signed-device request is stale"),
            Self::FutureTimestamp => write!(formatter, "signed-device request is from the future"),
            Self::RevokedKey => write!(formatter, "signed-device key is revoked"),
            Self::KeyNotYetValid => write!(formatter, "signed-device key is not yet valid"),
            Self::KeyExpired => write!(formatter, "signed-device key is expired"),
            Self::ClockUnavailable => write!(formatter, "signed-device clock is unavailable"),
            Self::VerificationFailed => write!(formatter, "signed-device verification failed"),
            Self::Provider => write!(formatter, "signed-device provider failed"),
        }
    }
}

impl std::error::Error for SignedDeviceError {}

#[derive(Clone, PartialEq, Eq)]
pub struct SignedDevicePrincipal {
    pub subject: String,
    pub realm: String,
    pub device_id: String,
    pub key_id: String,
    /// Internal provider identity. It is deliberately removed by
    /// `ApplicationIdentity::project` and never reaches an application value.
    pub provider: String,
    pub claims: BTreeMap<String, String>,
}

impl fmt::Debug for SignedDevicePrincipal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let claim_names = self.claims.keys().collect::<Vec<_>>();
        formatter
            .debug_struct("SignedDevicePrincipal")
            .field("subject", &self.subject)
            .field("realm", &self.realm)
            .field("device_id", &self.device_id)
            .field("key_id", &self.key_id)
            .field("claim_names", &claim_names)
            .finish_non_exhaustive()
    }
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
    pub application_id: String,
    pub application_version: String,
    pub publisher: String,
    pub lock_digest: String,
    pub claims: BTreeMap<String, String>,
}

impl ApplicationIdentity {
    pub fn project(principal: &SignedDevicePrincipal) -> Result<Self, ProjectionError> {
        if principal.realm != "application" {
            return Err(ProjectionError::WrongRealm);
        }
        for (name, value) in [
            ("subject", principal.subject.as_str()),
            ("device-id", principal.device_id.as_str()),
            ("key-id", principal.key_id.as_str()),
        ] {
            if !valid_opaque_token(value, 1, MAX_OPAQUE_TOKEN_BYTES) {
                return Err(ProjectionError::InvalidPrincipalField(name));
            }
        }
        if principal.claims.len() > MAX_PRINCIPAL_CLAIMS {
            return Err(ProjectionError::InvalidPrincipalField("claims"));
        }

        let application_id = required_claim(principal, "application/id")?;
        let application_version = required_claim(principal, "application/version")?;
        let publisher = required_claim(principal, "application/publisher")?;
        let lock_digest = required_claim(principal, "application/lock-digest")?;
        let namespace = required_claim(principal, "application/namespace")?;
        let collection = required_claim(principal, "application/collection")?;
        let operations = required_claim(principal, "application/operations")?;

        if !valid_identifier(&application_id) {
            return Err(ProjectionError::InvalidClaim("application/id"));
        }
        if !valid_opaque_token(&application_version, 1, MAX_IDENTIFIER_BYTES) {
            return Err(ProjectionError::InvalidClaim("application/version"));
        }
        if !valid_identifier(&publisher) {
            return Err(ProjectionError::InvalidClaim("application/publisher"));
        }
        if !valid_sha256(&lock_digest) {
            return Err(ProjectionError::InvalidClaim("application/lock-digest"));
        }
        if !valid_identifier(&namespace) {
            return Err(ProjectionError::InvalidClaim("application/namespace"));
        }
        if !valid_identifier(&collection) {
            return Err(ProjectionError::InvalidClaim("application/collection"));
        }
        if !valid_operation_list(&operations) {
            return Err(ProjectionError::InvalidClaim("application/operations"));
        }

        let mut claims = BTreeMap::from([
            ("application/id".to_owned(), application_id.clone()),
            (
                "application/version".to_owned(),
                application_version.clone(),
            ),
            ("application/publisher".to_owned(), publisher.clone()),
            ("application/lock-digest".to_owned(), lock_digest.clone()),
            ("application/namespace".to_owned(), namespace),
            ("application/collection".to_owned(), collection),
            ("application/operations".to_owned(), operations),
        ]);
        if let Some(label) = principal.claims.get("device/label") {
            if !valid_opaque_token(label, 1, MAX_OPAQUE_TOKEN_BYTES) {
                return Err(ProjectionError::InvalidClaim("device/label"));
            }
            claims.insert("device/label".to_owned(), label.clone());
        }

        Ok(Self {
            profile: APPLICATION_IDENTITY_PROFILE,
            subject: principal.subject.clone(),
            realm: principal.realm.clone(),
            device_id: principal.device_id.clone(),
            key_id: principal.key_id.clone(),
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
    WrongRealm,
    MissingClaim(&'static str),
    InvalidClaim(&'static str),
    InvalidPrincipalField(&'static str),
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongRealm => write!(formatter, "principal is not in the application realm"),
            Self::MissingClaim(name) => write!(formatter, "missing application claim {name}"),
            Self::InvalidClaim(name) => write!(formatter, "invalid application claim {name}"),
            Self::InvalidPrincipalField(name) => {
                write!(formatter, "invalid application principal field {name}")
            }
        }
    }
}

impl std::error::Error for ProjectionError {}

/// Trusted request facts derived from the actual HTTP exchange and route.
/// They are compared to every signed field before the provider is invoked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApplicationRequestExpectation<'a> {
    pub method: &'a str,
    pub target: &'a str,
    pub authority: &'a str,
    pub content_digest: &'a str,
    pub operation: &'a str,
    pub application: &'a str,
    pub namespace: &'a str,
    pub collection: &'a str,
}

impl ApplicationRequestExpectation<'_> {
    pub fn validate(&self) -> Result<(), SignedDeviceError> {
        let request = SignedDeviceRequest {
            method: self.method,
            target: self.target,
            authority: self.authority,
            content_digest: self.content_digest,
            operation: self.operation,
            application: self.application,
            namespace: self.namespace,
            collection: self.collection,
            timestamp: 1,
            nonce: "expectation-nonce",
            idempotency_key: "expectation-key-1",
            key_id: "expectation-key",
            signature: "expectation-signature",
        };
        request.validate()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplicationRequestField {
    Method,
    Target,
    Authority,
    ContentDigest,
    Operation,
    Application,
    Namespace,
    Collection,
}

impl fmt::Display for ApplicationRequestField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Method => "method",
            Self::Target => "target",
            Self::Authority => "authority",
            Self::ContentDigest => "content digest",
            Self::Operation => "operation",
            Self::Application => "application",
            Self::Namespace => "namespace",
            Self::Collection => "collection",
        };
        formatter.write_str(name)
    }
}

/// Closed evidence safe to project into the Hara request value.
///
/// It intentionally excludes the signature, public key, provider identity,
/// filesystem/database configuration, bearer tokens and raw authentication
/// object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedApplicationRequest {
    profile: &'static str,
    identity: ApplicationIdentity,
    operation: String,
    application: String,
    namespace: String,
    collection: String,
    content_digest: String,
    timestamp: i64,
    nonce: String,
    idempotency_key: String,
}

impl VerifiedApplicationRequest {
    pub const fn profile(&self) -> &'static str {
        self.profile
    }

    pub const fn identity(&self) -> &ApplicationIdentity {
        &self.identity
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

    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplicationAuthenticationError {
    InvalidRequest(&'static str),
    InvalidExpectation,
    RequestMismatch(ApplicationRequestField),
    AuthenticationRejected(&'static str),
    WrongRealm,
    MissingClaim(&'static str),
    InvalidIdentity(&'static str),
    OperationNotAllowed,
}

impl ApplicationAuthenticationError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(code) => code,
            Self::InvalidExpectation => "hoplite.application-auth/expectation-invalid",
            Self::RequestMismatch(_) => "hoplite.application-auth/request-mismatch",
            Self::AuthenticationRejected(code) => code,
            Self::WrongRealm => "hoplite.application-auth/wrong-realm",
            Self::MissingClaim(_) => "hoplite.application-auth/claim-missing",
            Self::InvalidIdentity(_) => "hoplite.application-auth/identity-mismatch",
            Self::OperationNotAllowed => "hoplite.application-auth/operation-not-allowed",
        }
    }
}

impl fmt::Display for ApplicationAuthenticationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(_) => write!(formatter, "signed application request is invalid"),
            Self::InvalidExpectation => {
                write!(formatter, "trusted application request expectation is invalid")
            }
            Self::RequestMismatch(field) => {
                write!(formatter, "signed application request {field} does not match")
            }
            Self::AuthenticationRejected(_) => {
                write!(formatter, "signed application request was rejected")
            }
            Self::WrongRealm => write!(formatter, "application identity realm is invalid"),
            Self::MissingClaim(name) => write!(formatter, "application claim {name} is missing"),
            Self::InvalidIdentity(name) => {
                write!(formatter, "application identity {name} does not match")
            }
            Self::OperationNotAllowed => {
                write!(formatter, "application operation is not allowed")
            }
        }
    }
}

impl std::error::Error for ApplicationAuthenticationError {}

pub fn authenticate_application_request<P: SignedDeviceProvider>(
    provider: &mut P,
    request: &SignedDeviceRequest<'_>,
    expectation: &ApplicationRequestExpectation<'_>,
) -> Result<VerifiedApplicationRequest, ApplicationAuthenticationError> {
    request
        .validate()
        .map_err(|error| ApplicationAuthenticationError::InvalidRequest(error.code()))?;
    expectation
        .validate()
        .map_err(|_| ApplicationAuthenticationError::InvalidExpectation)?;
    if let Some(field) = first_mismatch(request, expectation) {
        return Err(ApplicationAuthenticationError::RequestMismatch(field));
    }

    let principal = provider.authenticate(request).map_err(|error| {
        ApplicationAuthenticationError::AuthenticationRejected(error.code())
    })?;
    let identity = ApplicationIdentity::project(&principal).map_err(|error| match error {
        ProjectionError::WrongRealm => ApplicationAuthenticationError::WrongRealm,
        ProjectionError::MissingClaim(name) => {
            ApplicationAuthenticationError::MissingClaim(name)
        }
        ProjectionError::InvalidClaim(name)
        | ProjectionError::InvalidPrincipalField(name) => {
            ApplicationAuthenticationError::InvalidIdentity(name)
        }
    })?;

    if identity.application_id != request.application {
        return Err(ApplicationAuthenticationError::InvalidIdentity(
            "application/id",
        ));
    }
    require_claim_match(
        &identity,
        "application/namespace",
        request.namespace,
    )?;
    require_claim_match(
        &identity,
        "application/collection",
        request.collection,
    )?;
    let operations = identity
        .claims
        .get("application/operations")
        .ok_or(ApplicationAuthenticationError::MissingClaim(
            "application/operations",
        ))?;
    if !operation_allowed(operations, request.operation) {
        return Err(ApplicationAuthenticationError::OperationNotAllowed);
    }

    Ok(VerifiedApplicationRequest {
        profile: VERIFIED_APPLICATION_REQUEST_PROFILE,
        identity,
        operation: request.operation.to_owned(),
        application: request.application.to_owned(),
        namespace: request.namespace.to_owned(),
        collection: request.collection.to_owned(),
        content_digest: request.content_digest.to_owned(),
        timestamp: request.timestamp,
        nonce: request.nonce.to_owned(),
        idempotency_key: request.idempotency_key.to_owned(),
    })
}

fn first_mismatch(
    request: &SignedDeviceRequest<'_>,
    expectation: &ApplicationRequestExpectation<'_>,
) -> Option<ApplicationRequestField> {
    [
        (request.method == expectation.method, ApplicationRequestField::Method),
        (request.target == expectation.target, ApplicationRequestField::Target),
        (
            request.authority.eq_ignore_ascii_case(expectation.authority),
            ApplicationRequestField::Authority,
        ),
        (
            request.content_digest == expectation.content_digest,
            ApplicationRequestField::ContentDigest,
        ),
        (
            request.operation == expectation.operation,
            ApplicationRequestField::Operation,
        ),
        (
            request.application == expectation.application,
            ApplicationRequestField::Application,
        ),
        (
            request.namespace == expectation.namespace,
            ApplicationRequestField::Namespace,
        ),
        (
            request.collection == expectation.collection,
            ApplicationRequestField::Collection,
        ),
    ]
    .into_iter()
    .find_map(|(matches, field)| (!matches).then_some(field))
}

fn require_claim_match(
    identity: &ApplicationIdentity,
    name: &'static str,
    expected: &str,
) -> Result<(), ApplicationAuthenticationError> {
    match identity.claims.get(name) {
        None => Err(ApplicationAuthenticationError::MissingClaim(name)),
        Some(value) if value == expected => Ok(()),
        Some(_) => Err(ApplicationAuthenticationError::InvalidIdentity(name)),
    }
}

fn valid_operation_list(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_CLAIM_VALUE_BYTES {
        return false;
    }
    let mut count = 0;
    let mut seen = Vec::new();
    for operation in value.split(',').map(str::trim) {
        count += 1;
        if count > MAX_OPERATIONS
            || !valid_identifier(operation)
            || seen.contains(&operation)
        {
            return false;
        }
        seen.push(operation);
    }
    count > 0
}

fn operation_allowed(value: &str, expected: &str) -> bool {
    valid_operation_list(value)
        && value
            .split(',')
            .map(str::trim)
            .any(|operation| operation == expected)
}
