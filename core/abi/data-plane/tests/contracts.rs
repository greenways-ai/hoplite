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

fn request<'a>(target: &'a str) -> SignedDeviceRequest<'a> {
    SignedDeviceRequest {
        method: "PUT",
        target,
        authority: "tahto.local",
        content_digest:
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        timestamp: 1_786_080_000,
        nonce: "0123456789abcdef",
        key_id: "device.a",
        signature: "0123456789abcdef",
    }
}

#[test]
fn signed_device_input_rejects_absolute_or_path_like_authority() {
    assert!(request("/tahto/v1/objects").signing_input().is_ok());
    assert!(matches!(
        request("https://evil.example/object").validate(),
        Err(SignedDeviceError::InvalidTarget)
    ));
    let mut invalid = request("/tahto/v1/objects");
    invalid.authority = "user@tahto.local/path";
    assert!(matches!(
        invalid.validate(),
        Err(SignedDeviceError::InvalidAuthority)
    ));
}

fn application_principal(realm: &str) -> SignedDevicePrincipal {
    SignedDevicePrincipal {
        subject: "profile.primary".into(),
        realm: realm.into(),
        device_id: "device.a".into(),
        key_id: "key.a".into(),
        provider: "auth/signed-device".into(),
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
            ("session/access-token".into(), "must-not-project".into()),
            ("management/admin".into(), "true".into()),
        ]),
    }
}

#[test]
fn application_projection_rejects_management_and_filters_private_claims() {
    assert!(matches!(
        ApplicationIdentity::project(&application_principal("management")),
        Err(ProjectionError::WrongRealm(_))
    ));
    let projected = ApplicationIdentity::project(&application_principal("application")).unwrap();
    assert_eq!(projected.application_id, "app.example");
    assert!(projected.claims.contains_key("application/namespace"));
    assert!(!projected.claims.contains_key("session/access-token"));
    assert!(!projected.claims.contains_key("management/admin"));
}

#[test]
fn resource_handles_are_opaque_and_non_zero() {
    assert!(ResourceHandle::new(0).is_err());
    assert_eq!(ResourceHandle::new(42).unwrap().get(), 42);
}
