#[test]
fn reports_missing_and_tampered_objects_without_provider_details() {
    let root = TestRoot::new("integrity");
    let bootstrap = FilesystemBlobStore::open(root.path(), blob_limits()).unwrap();
    drop(bootstrap);
    let provider = provider(&root);
    let missing_digest = digest(b"missing");
    let missing = provider
        .execute(OPERATION, &request_arguments(missing_digest, 128))
        .unwrap();
    assert_failure(&missing, missing_digest, OBJECT_MISSING);

    let bytes = portable_value();
    let object_digest = install(&root, "value-a", &bytes);
    let (_, data_path) = object_paths(&root, object_digest);
    {
        let mut file = OpenOptions::new().write(true).open(data_path).unwrap();
        file.seek(SeekFrom::Start(4)).unwrap();
        file.write_all(&[0xff]).unwrap();
        file.sync_all().unwrap();
    }
    let provider = provider(&root);
    let tampered = provider
        .execute(OPERATION, &request_arguments(object_digest, bytes.len()))
        .unwrap();
    assert_failure(&tampered, object_digest, DIGEST_MISMATCH);
}

#[test]
fn classifies_malformed_noncanonical_and_runtime_only_hta() {
    let root = TestRoot::new("hta");
    let cases = [
        ("invalid-magic", b"NOPE".to_vec(), HTA_INVALID),
        (
            "truncated",
            vec![b'H', b'T', b'A', b'1', STRING, 0, 0, 0, 4, b'a'],
            HTA_INVALID,
        ),
        (
            "trailing",
            vec![b'H', b'T', b'A', b'1', NIL, NIL],
            HTA_INVALID,
        ),
        (
            "runtime",
            {
                let mut bytes = MAGIC.to_vec();
                bytes.extend_from_slice(&[SYMBOL, 0, 0, 0, 1, b'x']);
                bytes
            },
            VALUE_UNSUPPORTED,
        ),
        (
            "noncanonical",
            {
                let mut bytes = MAGIC.to_vec();
                bytes.push(MAP);
                bytes.extend_from_slice(&2_u32.to_be_bytes());
                bytes.extend_from_slice(&bare_keyword("aa"));
                bytes.push(NIL);
                bytes.extend_from_slice(&bare_keyword("z"));
                bytes.push(TRUE);
                bytes
            },
            HTA_NONCANONICAL,
        ),
    ];

    for (label, bytes, expected) in cases {
        let object_digest = install(&root, label, &bytes);
        let result = provider(&root)
            .execute(OPERATION, &request_arguments(object_digest, bytes.len()))
            .unwrap();
        assert_failure(&result, object_digest, expected);
    }
}

#[test]
fn detects_short_and_excess_object_reads_from_actual_bytes() {
    let root = TestRoot::new("length");
    let bytes = portable_value();
    let short_digest = install(&root, "short", &bytes);
    let (short_meta, _) = object_paths(&root, short_digest);
    rewrite_metadata_size(&short_meta, bytes.len() as u64 + 1);
    let short = provider(&root)
        .execute(
            OPERATION,
            &request_arguments(short_digest, bytes.len() + 1),
        )
        .unwrap();
    assert_failure(&short, short_digest, PROVIDER_FAILURE);

    let excess_digest = install(&root, "excess", &bytes);
    let (excess_meta, _) = object_paths(&root, excess_digest);
    rewrite_metadata_size(&excess_meta, bytes.len() as u64 - 1);
    let excess = provider(&root)
        .execute(
            OPERATION,
            &request_arguments(excess_digest, bytes.len()),
        )
        .unwrap();
    assert_failure(&excess, excess_digest, PROVIDER_FAILURE);
}
