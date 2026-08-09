use hoplite_data_plane_abi::*;
use std::collections::BTreeMap;
use std::io::Cursor;

fn limits() -> BodyLimits {
    BodyLimits {
        max_body_bytes: 8,
        max_chunk_bytes: 3,
        require_declared_length: false,
    }
}

#[test]
fn request_body_enforces_declared_and_observed_limits() {
    assert!(matches!(
        BoundedBody::new(Cursor::new(vec![0_u8; 9]), Some(9), limits()),
        Err(BodyError::LimitExceeded { .. })
    ));

    let mut body = BoundedBody::new(Cursor::new(b"123456789"), None, limits()).unwrap();
    let mut buffer = [0_u8; 16];
    assert_eq!(body.read_chunk(&mut buffer).unwrap(), 3);
    assert_eq!(body.read_chunk(&mut buffer).unwrap(), 3);
    assert_eq!(body.read_chunk(&mut buffer).unwrap(), 2);
    assert!(matches!(
        body.read_chunk(&mut buffer),
        Err(BodyError::LimitExceeded {
            limit: 8,
            attempted: 9
        })
    ));
}

#[test]
fn request_body_never_reads_more_than_the_chunk_limit() {
    let mut body = BoundedBody::new(Cursor::new(b"123456"), Some(6), limits()).unwrap();
    let mut buffer = [0_u8; 32];
    assert_eq!(body.read_chunk(&mut buffer).unwrap(), 3);
    assert_eq!(body.read_chunk(&mut buffer).unwrap(), 3);
    assert_eq!(body.read_chunk(&mut buffer).unwrap(), 0);
    body.finish().unwrap();
}

#[test]
fn declared_length_must_match_on_finish() {
    let mut body = BoundedBody::new(Cursor::new(b"123"), Some(4), limits()).unwrap();
    let mut buffer = [0_u8; 8];
    assert_eq!(body.read_chunk(&mut buffer).unwrap(), 3);
    assert_eq!(body.read_chunk(&mut buffer).unwrap(), 0);
    assert!(matches!(
        body.finish(),
        Err(BodyError::DeclaredLengthMismatch {
            declared: 4,
            observed: 3
        })
    ));
}

#[test]
fn single_ranges_cover_exact_open_and_suffix_forms() {
    assert_eq!(
        resolve_single_range("bytes=2-4", 10).unwrap(),
        ByteRange {
            start: 2,
            end_exclusive: 5
        }
    );
    assert_eq!(
        resolve_single_range("bytes=7-", 10).unwrap(),
        ByteRange {
            start: 7,
            end_exclusive: 10
        }
    );
    assert_eq!(
        resolve_single_range("bytes=-3", 10).unwrap(),
        ByteRange {
            start: 7,
            end_exclusive: 10
        }
    );
    assert!(matches!(
        resolve_single_range("bytes=0-1,4-5", 10),
        Err(RangeError::MultipleRanges)
    ));
    assert!(matches!(
        resolve_single_range("items=0-1", 10),
        Err(RangeError::UnsupportedUnit)
    ));
}

#[test]
fn response_streams_a_range_without_materializing_the_full_body() {
    let source = SliceBody::new(b"0123456789");
    let mut response = StreamResponse::new(source, Some("bytes=3-7")).unwrap();
    assert_eq!(response.plan().status, 206);
    assert_eq!(response.plan().content_length, 5);
    assert_eq!(response.plan().content_range.as_deref(), Some("bytes 3-7/10"));
    let mut output = [0_u8; 2];
    let mut collected = Vec::new();
    loop {
        let read = response.read_next(&mut output).unwrap();
        if read == 0 {
            break;
        }
        collected.extend_from_slice(&output[..read]);
    }
    response.finish().unwrap();
    assert_eq!(collected, b"34567");
}

fn request<'a>(target: &'a str, signature: &'a str) -> SignedDeviceRequest<'a> {
    SignedDeviceRequest {
        method: "PUT",
        target,
        authority: "tahto.local",
        content_digest:
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        operation: "object.upload",
        application: "app.example",
        namespace: "profile.primary",
        collection: "objects",
        timestamp: 1_786_080_000,
        nonce: "0123456789abcdef",
        idempotency_key: "request-00000001",
        key_id: "device.a",
        signature,
    }
}

fn expectation<'a>(target: &'a str) -> ApplicationRequestExpectation<'a> {
    ApplicationRequestExpectation {
        method: "PUT",
        target,
        authority: "tahto.local",
        content_digest:
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        operation: "object.upload",
        application: "app.example",
        namespace: "profile.primary",
        collection: "objects",
    }
}

#[test]
fn signed_device_v2_binds_application_coordinates_and_idempotency() {
    let mut unsigned = request("/tahto/v1/objects", "");
    let input = unsigned.signing_input().unwrap();
    assert!(matches!(
        unsigned.validate(),
        Err(SignedDeviceError::InvalidSignature)
    ));
    unsigned.signature = "0123456789abcdef";
    assert!(unsigned.validate().is_ok());
    assert_eq!(
        input,
        concat!(
            "hoplite-signed-device/2\n",
            "PUT\n",
            "/tahto/v1/objects\n",
            "tahto.local\n",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
            "object.upload\n",
            "app.example\n",
            "profile.primary\n",
            "objects\n",
            "1786080000\n",
            "0123456789abcdef\n",
            "request-00000001\n",
            "device.a"
        )
    );
}

#[test]
fn signed_request_debug_output_redacts_the_signature() {
    let request = request("/tahto/v1/objects", "secret-signature-1");
    let debug = format!("{request:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("secret-signature-1"));
}

#[test]
fn signed_device_input_rejects_absolute_or_path_like_authority() {
    assert!(request("/tahto/v1/objects", "0123456789abcdef")
        .signing_input()
        .is_ok());
    assert!(matches!(
        request("https://evil.example/object", "0123456789abcdef").validate(),
        Err(SignedDeviceError::InvalidTarget)
    ));
    let mut invalid = request("/tahto/v1/objects", "0123456789abcdef");
    invalid.authority = "user@tahto.local/path";
    assert!(matches!(
        invalid.validate(),
        Err(SignedDeviceError::InvalidAuthority)
    ));
}

#[test]
fn signed_device_input_rejects_unbound_or_ambiguous_application_fields() {
    let mut invalid = request("/tahto/v1/objects", "0123456789abcdef");
    invalid.operation = "Object Upload";
    assert!(matches!(
        invalid.validate(),
        Err(SignedDeviceError::InvalidOperation)
    ));
    invalid = request("/tahto/v1/objects", "0123456789abcdef");
    invalid.idempotency_key = "short";
    assert!(matches!(
        invalid.validate(),
        Err(SignedDeviceError::InvalidIdempotencyKey)
    ));
}

fn application_principal(realm: &str) -> SignedDevicePrincipal {
    SignedDevicePrincipal {
        subject: "profile.primary".into(),
        realm: realm.into(),
        device_id: "device.a".into(),
        key_id: "key.a".into(),
        provider: "auth/ed25519".into(),
        claims: BTreeMap::from([
            ("application/id".into(), "app.example".into()),
            ("application/version".into(), "1.0.0".into()),
            ("application/publisher".into(), "greenways.example".into()),
            (
                "application/lock-digest".into(),
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    .into(),
            ),
            ("application/namespace".into(), "profile.primary".into()),
            ("application/collection".into(), "objects".into()),
            (
                "application/operations".into(),
                "object.upload,object.read".into(),
            ),
            ("session/access-token".into(), "must-not-project".into()),
            ("management/admin".into(), "true".into()),
            ("provider/public-key".into(), "must-not-project".into()),
        ]),
    }
}

#[test]
fn application_projection_rejects_management_and_filters_private_claims() {
    assert!(matches!(
        ApplicationIdentity::project(&application_principal("management")),
        Err(ProjectionError::WrongRealm)
    ));
    let projected = ApplicationIdentity::project(&application_principal("application")).unwrap();
    assert_eq!(projected.application_id, "app.example");
    assert!(projected.claims.contains_key("application/namespace"));
    assert!(!projected.claims.contains_key("session/access-token"));
    assert!(!projected.claims.contains_key("management/admin"));
    assert!(!projected.claims.contains_key("provider/public-key"));
    assert!(!format!("{projected:?}").contains("auth/ed25519"));
    let principal_debug = format!("{:?}", application_principal("application"));
    assert!(!principal_debug.contains("auth/ed25519"));
    assert!(!principal_debug.contains("must-not-project"));
}

struct FixtureProvider {
    principal: SignedDevicePrincipal,
    calls: usize,
}

impl SignedDeviceProvider for FixtureProvider {
    fn authenticate(
        &mut self,
        request: &SignedDeviceRequest<'_>,
    ) -> Result<SignedDevicePrincipal, SignedDeviceError> {
        self.calls += 1;
        if request.signature != "valid-signature-1" {
            return Err(SignedDeviceError::VerificationFailed);
        }
        Ok(self.principal.clone())
    }
}

#[test]
fn application_authentication_returns_only_closed_verified_evidence() {
    let mut provider = FixtureProvider {
        principal: application_principal("application"),
        calls: 0,
    };
    let request = request("/tahto/v1/objects", "valid-signature-1");
    let verified = authenticate_application_request(
        &mut provider,
        &request,
        &expectation("/tahto/v1/objects"),
    )
    .unwrap();
    assert_eq!(provider.calls, 1);
    assert_eq!(verified.profile(), VERIFIED_APPLICATION_REQUEST_PROFILE);
    assert_eq!(verified.identity().device_id, "device.a");
    assert_eq!(verified.operation(), "object.upload");
    assert_eq!(verified.content_digest(), request.content_digest);
    assert_eq!(verified.nonce(), request.nonce);
    assert_eq!(verified.idempotency_key(), request.idempotency_key);
    let debug = format!("{verified:?}");
    assert!(!debug.contains("valid-signature-1"));
    assert!(!debug.contains("auth/ed25519"));
    assert!(!debug.contains("must-not-project"));
}

#[test]
fn trusted_request_mismatch_fails_before_signature_provider_access() {
    let mut provider = FixtureProvider {
        principal: application_principal("application"),
        calls: 0,
    };
    let request = request("/tahto/v1/objects", "valid-signature-1");
    let mut expected = expectation("/tahto/v1/objects");
    expected.content_digest =
        "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let error = authenticate_application_request(&mut provider, &request, &expected).unwrap_err();
    assert_eq!(provider.calls, 0);
    assert_eq!(
        error,
        ApplicationAuthenticationError::RequestMismatch(
            ApplicationRequestField::ContentDigest
        )
    );
    assert_eq!(error.code(), "hoplite.application-auth/request-mismatch");
}

#[test]
fn management_identity_and_unlisted_operations_fail_closed() {
    let request = request("/tahto/v1/objects", "valid-signature-1");
    let mut management = FixtureProvider {
        principal: application_principal("management"),
        calls: 0,
    };
    assert_eq!(
        authenticate_application_request(
            &mut management,
            &request,
            &expectation("/tahto/v1/objects")
        )
        .unwrap_err(),
        ApplicationAuthenticationError::WrongRealm
    );

    let mut principal = application_principal("application");
    principal.claims.insert(
        "application/operations".into(),
        "object.read".into(),
    );
    let mut provider = FixtureProvider {
        principal,
        calls: 0,
    };
    assert_eq!(
        authenticate_application_request(
            &mut provider,
            &request,
            &expectation("/tahto/v1/objects")
        )
        .unwrap_err(),
        ApplicationAuthenticationError::OperationNotAllowed
    );
}

#[test]
fn provider_details_are_collapsed_to_stable_rejection_codes() {
    struct FailingProvider;
    impl SignedDeviceProvider for FailingProvider {
        fn authenticate(
            &mut self,
            _request: &SignedDeviceRequest<'_>,
        ) -> Result<SignedDevicePrincipal, SignedDeviceError> {
            Err(SignedDeviceError::Provider)
        }
    }

    let request = request("/tahto/v1/objects", "valid-signature-1");
    let error = authenticate_application_request(
        &mut FailingProvider,
        &request,
        &expectation("/tahto/v1/objects"),
    )
    .unwrap_err();
    assert_eq!(
        error.code(),
        "hoplite.signed-device/provider-failed"
    );
    assert!(!error.to_string().contains("/private"));
}

#[test]
fn resource_handles_are_opaque_and_non_zero() {
    assert!(ResourceHandle::new(0).is_err());
    assert_eq!(ResourceHandle::new(42).unwrap().get(), 42);
}
