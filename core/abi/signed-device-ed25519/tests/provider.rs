use ed25519_dalek::{Signer, SigningKey};
use hoplite_data_plane_abi::{
    authenticate_application_request, ApplicationAuthenticationError,
    ApplicationRequestExpectation, SignedDeviceError, SignedDeviceProvider,
    SignedDeviceRequest, VERIFIED_APPLICATION_REQUEST_PROFILE,
};
use hoplite_signed_device_ed25519::{
    Clock, ClockError, ConfigurationError, FreshnessPolicy, KeyRecord, KeyWindow, Provider,
    PROVIDER_ID,
};
use std::collections::BTreeMap;

const NOW: i64 = 1_786_080_000;
const DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const LOCK_DIGEST: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[derive(Clone, Copy)]
struct FixedClock(i64);

impl Clock for FixedClock {
    fn unix_seconds(&self) -> Result<i64, ClockError> {
        Ok(self.0)
    }
}

#[derive(Clone, Copy)]
struct FailingClock;

impl Clock for FailingClock {
    fn unix_seconds(&self) -> Result<i64, ClockError> {
        Err(ClockError::OutOfRange)
    }
}

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn claims() -> BTreeMap<String, String> {
    BTreeMap::from([
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
        ("provider/private".into(), "must-not-project".into()),
    ])
}

fn record(
    key: &SigningKey,
    realm: &str,
    window: KeyWindow,
) -> Result<KeyRecord, ConfigurationError> {
    KeyRecord::new(
        "key.device-a",
        "profile.primary",
        realm,
        "device.a",
        key.verifying_key().to_bytes(),
        window,
        claims(),
    )
}

fn unsigned_request<'a>(timestamp: i64, key_id: &'a str) -> SignedDeviceRequest<'a> {
    SignedDeviceRequest {
        method: "PUT",
        target: "/tahto/v1/objects",
        authority: "tahto.local",
        content_digest: DIGEST,
        operation: "object.upload",
        application: "app.example",
        namespace: "profile.primary",
        collection: "objects",
        timestamp,
        nonce: "nonce-device-a-0001",
        idempotency_key: "request-device-a-0001",
        key_id,
        signature: "",
    }
}

fn sign<'a>(
    key: &SigningKey,
    timestamp: i64,
    key_id: &'a str,
    signature: &'a mut String,
) -> SignedDeviceRequest<'a> {
    let unsigned = unsigned_request(timestamp, key_id);
    let bytes = unsigned.signing_input().unwrap();
    *signature = encode_hex(&key.sign(bytes.as_bytes()).to_bytes());
    SignedDeviceRequest {
        signature: signature.as_str(),
        ..unsigned
    }
}

fn expectation() -> ApplicationRequestExpectation<'static> {
    ApplicationRequestExpectation {
        method: "PUT",
        target: "/tahto/v1/objects",
        authority: "tahto.local",
        content_digest: DIGEST,
        operation: "object.upload",
        application: "app.example",
        namespace: "profile.primary",
        collection: "objects",
    }
}

fn provider(
    key: &SigningKey,
    realm: &str,
    window: KeyWindow,
) -> Provider<FixedClock> {
    Provider::new(
        [record(key, realm, window).unwrap()],
        FreshnessPolicy::new(300, 30).unwrap(),
        FixedClock(NOW),
    )
    .unwrap()
}

#[test]
fn valid_signature_reaches_the_application_adapter_as_closed_evidence() {
    let key = signing_key(7);
    let mut provider = provider(&key, "application", KeyWindow::default());
    let mut signature = String::new();
    let request = sign(&key, NOW, "key.device-a", &mut signature);
    let verified =
        authenticate_application_request(&mut provider, &request, &expectation()).unwrap();

    assert_eq!(provider.key_count(), 1);
    assert_eq!(verified.profile(), VERIFIED_APPLICATION_REQUEST_PROFILE);
    assert_eq!(verified.identity().device_id, "device.a");
    assert_eq!(verified.operation(), "object.upload");
    assert_eq!(verified.content_digest(), DIGEST);
    assert!(!format!("{verified:?}").contains(PROVIDER_ID));
    assert!(!format!("{verified:?}").contains(&signature));
    assert!(!format!("{verified:?}").contains("must-not-project"));
}

#[test]
fn the_exact_v2_signing_bytes_are_verified() {
    let key = signing_key(7);
    let mut provider = provider(&key, "application", KeyWindow::default());
    let mut signature = String::new();
    let mut request = sign(&key, NOW, "key.device-a", &mut signature);
    request.operation = "object.read";
    assert_eq!(
        provider.authenticate(&request).unwrap_err(),
        SignedDeviceError::VerificationFailed
    );
}

#[test]
fn malformed_unknown_and_wrong_signatures_have_stable_failures() {
    let key = signing_key(7);
    let other = signing_key(8);
    let mut provider = provider(&key, "application", KeyWindow::default());

    let mut signature = String::new();
    let malformed = SignedDeviceRequest {
        signature: "not-hex-signature",
        ..unsigned_request(NOW, "key.device-a")
    };
    assert_eq!(
        provider.authenticate(&malformed).unwrap_err(),
        SignedDeviceError::InvalidSignature
    );

    let unknown = sign(&key, NOW, "key.unknown", &mut signature);
    assert_eq!(
        provider.authenticate(&unknown).unwrap_err(),
        SignedDeviceError::UnknownKey
    );

    let wrong = sign(&other, NOW, "key.device-a", &mut signature);
    assert_eq!(
        provider.authenticate(&wrong).unwrap_err(),
        SignedDeviceError::VerificationFailed
    );
}

#[test]
fn freshness_and_key_lifecycle_are_host_enforced() {
    let key = signing_key(7);
    let mut signature = String::new();
    let mut provider = provider(&key, "application", KeyWindow::default());
    assert_eq!(
        provider
            .authenticate(&sign(&key, NOW - 301, "key.device-a", &mut signature))
            .unwrap_err(),
        SignedDeviceError::StaleTimestamp
    );
    assert_eq!(
        provider
            .authenticate(&sign(&key, NOW + 31, "key.device-a", &mut signature))
            .unwrap_err(),
        SignedDeviceError::FutureTimestamp
    );

    let mut not_yet = provider(
        &key,
        "application",
        KeyWindow {
            not_before: Some(NOW + 1),
            ..KeyWindow::default()
        },
    );
    assert_eq!(
        not_yet
            .authenticate(&sign(&key, NOW, "key.device-a", &mut signature))
            .unwrap_err(),
        SignedDeviceError::KeyNotYetValid
    );

    let mut expired = provider(
        &key,
        "application",
        KeyWindow {
            expires_at: Some(NOW - 1),
            ..KeyWindow::default()
        },
    );
    assert_eq!(
        expired
            .authenticate(&sign(&key, NOW, "key.device-a", &mut signature))
            .unwrap_err(),
        SignedDeviceError::KeyExpired
    );

    let mut revoked = provider(
        &key,
        "application",
        KeyWindow {
            revoked_at: Some(NOW),
            ..KeyWindow::default()
        },
    );
    assert_eq!(
        revoked
            .authenticate(&sign(&key, NOW, "key.device-a", &mut signature))
            .unwrap_err(),
        SignedDeviceError::RevokedKey
    );
}

#[test]
fn a_rotated_key_can_replace_an_expired_key_for_the_same_device() {
    let old_key = signing_key(7);
    let new_key = signing_key(8);
    let old_record = KeyRecord::new(
        "key.device-a.old",
        "profile.primary",
        "application",
        "device.a",
        old_key.verifying_key().to_bytes(),
        KeyWindow {
            expires_at: Some(NOW),
            ..KeyWindow::default()
        },
        claims(),
    )
    .unwrap();
    let new_record = KeyRecord::new(
        "key.device-a.new",
        "profile.primary",
        "application",
        "device.a",
        new_key.verifying_key().to_bytes(),
        KeyWindow {
            not_before: Some(NOW),
            ..KeyWindow::default()
        },
        claims(),
    )
    .unwrap();
    let mut provider = Provider::new(
        [old_record, new_record],
        FreshnessPolicy::default(),
        FixedClock(NOW),
    )
    .unwrap();
    let mut signature = String::new();
    assert_eq!(
        provider
            .authenticate(&sign(
                &old_key,
                NOW,
                "key.device-a.old",
                &mut signature,
            ))
            .unwrap_err(),
        SignedDeviceError::KeyExpired
    );
    let principal = provider
        .authenticate(&sign(
            &new_key,
            NOW,
            "key.device-a.new",
            &mut signature,
        ))
        .unwrap();
    assert_eq!(principal.device_id, "device.a");
    assert_eq!(principal.key_id, "key.device-a.new");
}

#[test]
fn management_keys_cannot_project_into_application_handlers() {
    let key = signing_key(7);
    let mut provider = provider(&key, "management", KeyWindow::default());
    let mut signature = String::new();
    let request = sign(&key, NOW, "key.device-a", &mut signature);
    assert_eq!(
        authenticate_application_request(&mut provider, &request, &expectation()).unwrap_err(),
        ApplicationAuthenticationError::WrongRealm
    );
}

#[test]
fn clock_failures_collapse_to_a_stable_error() {
    let key = signing_key(7);
    let mut provider = Provider::new(
        [record(&key, "application", KeyWindow::default()).unwrap()],
        FreshnessPolicy::default(),
        FailingClock,
    )
    .unwrap();
    let mut signature = String::new();
    let request = sign(&key, NOW, "key.device-a", &mut signature);
    assert_eq!(
        provider.authenticate(&request).unwrap_err(),
        SignedDeviceError::ClockUnavailable
    );
}

#[test]
fn duplicate_and_empty_key_configurations_fail_at_startup() {
    let key = signing_key(7);
    let first = record(&key, "application", KeyWindow::default()).unwrap();
    let second = record(&key, "application", KeyWindow::default()).unwrap();
    assert!(matches!(
        Provider::new(
            [first, second],
            FreshnessPolicy::default(),
            FixedClock(NOW)
        ),
        Err(ConfigurationError::DuplicateKey)
    ));
    assert!(matches!(
        Provider::new(
            Vec::<KeyRecord>::new(),
            FreshnessPolicy::default(),
            FixedClock(NOW)
        ),
        Err(ConfigurationError::EmptyKeySet)
    ));

    let mut invalid_claims = claims();
    invalid_claims.insert("provider/path".into(), "line\nbreak".into());
    assert!(matches!(
        KeyRecord::new(
            "key.invalid",
            "profile.primary",
            "application",
            "device.a",
            key.verifying_key().to_bytes(),
            KeyWindow::default(),
            invalid_claims,
        ),
        Err(ConfigurationError::InvalidClaims)
    ));
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
