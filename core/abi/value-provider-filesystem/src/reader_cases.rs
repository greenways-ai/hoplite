use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

struct StaticReader {
    object: VerifiedObject,
    calls: Arc<AtomicUsize>,
}

impl ImmutableObjectReader for StaticReader {
    fn read_verified(
        &self,
        digest: Digest,
        max_bytes: usize,
    ) -> Result<VerifiedObject, Failure> {
        self.calls.fetch_add(1, AtomicOrdering::SeqCst);
        if digest != self.object.digest() {
            return Err(Failure::Missing);
        }
        if self.object.byte_length() > max_bytes {
            return Err(Failure::Maximum);
        }
        Ok(self.object.clone())
    }
}

struct FailingReader(Failure);

impl ImmutableObjectReader for FailingReader {
    fn read_verified(
        &self,
        _digest: Digest,
        _max_bytes: usize,
    ) -> Result<VerifiedObject, Failure> {
        Err(self.0)
    }
}

#[test]
fn value_service_composes_with_an_injected_non_filesystem_reader() {
    let bytes = portable_value();
    let object_digest = digest(&bytes);
    let calls = Arc::new(AtomicUsize::new(0));
    let service = ValueService::new(
        StaticReader {
            object: VerifiedObject::new(object_digest, bytes.clone()),
            calls: calls.clone(),
        },
        value_limits(),
    )
    .unwrap();

    let result = service
        .execute(OPERATION, &request_arguments(object_digest, bytes.len()))
        .unwrap();
    let value = Document::parse(&result).unwrap();
    let value = value.root();
    assert!(value.require("verified").unwrap().as_bool().unwrap());
    assert_eq!(
        value.require("digest").unwrap().as_text().unwrap(),
        object_digest.to_string()
    );
    assert_eq!(value.require("value").unwrap().standalone_frame(), bytes);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
}

#[test]
fn value_service_normalizes_reader_failures_without_backend_details() {
    let object_digest = digest(b"reader-failure");
    let cases = [
        (Failure::Missing, OBJECT_MISSING),
        (Failure::Maximum, MAXIMUM_EXCEEDED),
        (Failure::Digest, DIGEST_MISMATCH),
        (Failure::Provider, PROVIDER_FAILURE),
    ];

    for (failure, expected_code) in cases {
        let service = ValueService::new(FailingReader(failure), value_limits()).unwrap();
        let result = service
            .execute(OPERATION, &request_arguments(object_digest, 128))
            .unwrap();
        assert_failure(&result, object_digest, expected_code);
    }
}
