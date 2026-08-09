#[test]
fn verifies_one_closed_canonical_value_and_reuses_the_blob_layout() {
    let root = TestRoot::new("success");
    let bytes = portable_value();
    let object_digest = install(&root, "value-a", &bytes);
    let provider = provider(&root);
    let result = provider
        .execute(OPERATION, &request_arguments(object_digest, bytes.len()))
        .unwrap();

    let document = Document::parse(&result).unwrap();
    let value = document.root();
    assert_eq!(value.len().unwrap(), 7);
    assert_eq!(
        value.require("protocol").unwrap().as_text().unwrap(),
        RESULT_PROTOCOL
    );
    assert_eq!(
        value.require("operation").unwrap().as_text().unwrap(),
        OPERATION
    );
    assert!(value.require("verified").unwrap().as_bool().unwrap());
    assert_eq!(
        value.require("digest").unwrap().as_text().unwrap(),
        object_digest.to_string()
    );
    assert_eq!(
        value.require("byte-length").unwrap().as_i64().unwrap(),
        bytes.len() as i64
    );
    assert_eq!(
        value.require("profile").unwrap().as_text().unwrap(),
        PROFILE
    );
    assert_eq!(value.require("value").unwrap().standalone_frame(), bytes);
    assert!(value.map_get("path").unwrap().is_none());
    assert!(value.map_get("provider").unwrap().is_none());

    let blob = FilesystemBlobStore::open(root.path(), blob_limits()).unwrap();
    assert_eq!(
        read_blob_source(&blob, object_digest, bytes.len() as u64),
        bytes
    );
}

#[test]
fn enforces_exact_request_and_installed_maximum_boundaries() {
    let root = TestRoot::new("bounds");
    let bytes = portable_value();
    let object_digest = install(&root, "value-a", &bytes);
    let provider = provider(&root);

    let exact = provider
        .execute(OPERATION, &request_arguments(object_digest, bytes.len()))
        .unwrap();
    assert!(Document::parse(&exact)
        .unwrap()
        .root()
        .require("verified")
        .unwrap()
        .as_bool()
        .unwrap());

    let below = provider
        .execute(
            OPERATION,
            &request_arguments(object_digest, bytes.len() - 1),
        )
        .unwrap();
    assert_failure(&below, object_digest, MAXIMUM_EXCEEDED);

    let limited = FilesystemValueProvider::open(
        root.path(),
        Limits {
            max_frame_bytes: bytes.len() - 1,
            max_media_type_bytes: 128,
            io_chunk_bytes: 5,
        },
    )
    .unwrap();
    let configured = limited
        .execute(OPERATION, &request_arguments(object_digest, bytes.len()))
        .unwrap();
    assert_failure(&configured, object_digest, MAXIMUM_EXCEEDED);

    let overflow_digest = install(&root, "value-overflow", &bytes);
    let (_, overflow_data) = object_paths(&root, overflow_digest);
    {
        let mut file = OpenOptions::new().append(true).open(overflow_data).unwrap();
        file.write_all(&[0]).unwrap();
        file.sync_all().unwrap();
    }
    let during_io = provider
        .execute(
            OPERATION,
            &request_arguments(overflow_digest, bytes.len()),
        )
        .unwrap();
    assert_failure(&during_io, overflow_digest, MAXIMUM_EXCEEDED);

    let extra = request_arguments_with(vec![
        ("digest", bare_string(&object_digest.to_string())),
        ("max-bytes", bare_usize(bytes.len()).unwrap()),
        ("operation", bare_string(OPERATION)),
        ("path", bare_string("/tmp/forbidden")),
        ("protocol", bare_string(REQUEST_PROTOCOL)),
    ]);
    assert_eq!(
        provider.execute(OPERATION, &extra).unwrap_err().code(),
        "value-request-invalid"
    );
}
