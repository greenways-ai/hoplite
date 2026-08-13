use hoplite_data_plane_abi::*;
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
    assert_eq!(
        response.plan().content_range.as_deref(),
        Some("bytes 3-7/10")
    );
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

#[test]
fn resource_handles_are_opaque_and_non_zero() {
    assert!(ResourceHandle::new(0).is_err());
    assert_eq!(ResourceHandle::new(42).unwrap().get(), 42);
}
