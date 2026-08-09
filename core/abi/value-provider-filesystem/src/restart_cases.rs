#[test]
fn reopens_against_the_same_root_without_persisting_decoded_state() {
    let root = TestRoot::new("restart");
    let bytes = portable_value();
    let object_digest = install(&root, "value-a", &bytes);
    {
        let first = provider(&root);
        let result = first
            .execute(OPERATION, &request_arguments(object_digest, bytes.len()))
            .unwrap();
        assert!(Document::parse(&result)
            .unwrap()
            .root()
            .require("verified")
            .unwrap()
            .as_bool()
            .unwrap());
    }
    let reopened = provider(&root);
    let result = reopened
        .execute(OPERATION, &request_arguments(object_digest, bytes.len()))
        .unwrap();
    assert!(Document::parse(&result)
        .unwrap()
        .root()
        .require("verified")
        .unwrap()
        .as_bool()
        .unwrap());
}
