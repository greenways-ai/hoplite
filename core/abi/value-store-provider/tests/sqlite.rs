use hoplite_provider_hta::Document;
use hoplite_value_store::{Digest, DigestVerifier, StoreLimits, REQUEST_PROTOCOL};
use hoplite_value_store_provider::Provider;
use hoplite_value_store_sqlite::{Sha256Verifier, SqliteValueStore};
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

const MAGIC: &[u8; 4] = b"HTA1";
const NIL: u8 = 0;
const I64: u8 = 3;
const STRING: u8 = 4;
const KEYWORD: u8 = 6;
const VECTOR: u8 = 9;
const MAP: u8 = 11;

struct TempDatabase {
    root: PathBuf,
    path: PathBuf,
}

impl TempDatabase {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "hoplite-value-store-provider-{label}-{}-{nonce}",
            process::id()
        ));
        fs::create_dir_all(&root).expect("create temporary database directory");
        let path = root.join("state.sqlite3");
        Self { root, path }
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

type SqliteProvider = Provider<SqliteValueStore, Sha256Verifier>;

fn provider(path: &Path) -> SqliteProvider {
    let limits = StoreLimits::default();
    Provider::new(
        SqliteValueStore::open(path, limits).expect("open SQLite value store"),
        Sha256Verifier,
        limits,
    )
    .expect("construct hara.store adapter")
}

fn digest(bytes: &[u8]) -> String {
    Digest::from_bytes(Sha256Verifier.sha256(bytes)).to_string()
}

fn frame(bare: &[u8]) -> Vec<u8> {
    let mut output = MAGIC.to_vec();
    output.extend_from_slice(bare);
    output
}

fn bare_text(tag: u8, value: &str) -> Vec<u8> {
    let mut output = vec![tag];
    output.extend_from_slice(
        &u32::try_from(value.len())
            .expect("test text length")
            .to_be_bytes(),
    );
    output.extend_from_slice(value.as_bytes());
    output
}

fn bare_string(value: &str) -> Vec<u8> {
    bare_text(STRING, value)
}

fn bare_keyword(value: &str) -> Vec<u8> {
    bare_text(KEYWORD, value)
}

fn bare_i64(value: i64) -> Vec<u8> {
    let mut output = vec![I64];
    output.extend_from_slice(&value.to_be_bytes());
    output
}

fn bare_vector(values: &[Vec<u8>]) -> Vec<u8> {
    let mut output = vec![VECTOR];
    output.extend_from_slice(
        &u32::try_from(values.len())
            .expect("test vector length")
            .to_be_bytes(),
    );
    for value in values {
        output.extend_from_slice(value);
    }
    output
}

fn bare_map(entries: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
    let mut entries = entries
        .into_iter()
        .map(|(key, value)| (bare_keyword(key), value))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut output = vec![MAP];
    output.extend_from_slice(
        &u32::try_from(entries.len())
            .expect("test map length")
            .to_be_bytes(),
    );
    for (key, value) in entries {
        output.extend_from_slice(&key);
        output.extend_from_slice(&value);
    }
    output
}

fn arguments(request: Vec<u8>) -> Vec<u8> {
    frame(&bare_vector(&[request]))
}

fn request(entries: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
    bare_map(entries)
}

fn value(revision: i64, label: &str) -> Vec<u8> {
    frame(&bare_map(vec![
        ("metadata-revision", bare_i64(revision)),
        (
            "nested",
            bare_vector(&[
                bare_string(label),
                bare_map(vec![
                    ("kind", bare_keyword("fixture")),
                    ("revision", bare_i64(revision)),
                ]),
            ]),
        ),
    ]))
}

fn receipt(label: &str, revision: i64) -> Vec<u8> {
    frame(&bare_map(vec![
        ("label", bare_string(label)),
        ("revision", bare_i64(revision)),
    ]))
}

fn load_request() -> Vec<u8> {
    request(vec![
        ("operation", bare_string("load")),
        ("protocol", bare_string(REQUEST_PROTOCOL)),
    ])
}

fn initialize_request(value: &[u8], revision: i64) -> Vec<u8> {
    request(vec![
        ("operation", bare_string("initialize")),
        ("protocol", bare_string(REQUEST_PROTOCOL)),
        ("revision", bare_i64(revision)),
        ("value", value[MAGIC.len()..].to_vec()),
        ("value-digest", bare_string(&digest(value))),
    ])
}

fn compare_request(
    expected_revision: i64,
    revision: i64,
    value: &[u8],
    receipt_key: &str,
    receipt: &[u8],
) -> Vec<u8> {
    request(vec![
        ("expected-revision", bare_i64(expected_revision)),
        ("operation", bare_string("compare-and-swap")),
        ("protocol", bare_string(REQUEST_PROTOCOL)),
        ("receipt", receipt[MAGIC.len()..].to_vec()),
        ("receipt-key", bare_string(receipt_key)),
        ("revision", bare_i64(revision)),
        ("value", value[MAGIC.len()..].to_vec()),
        ("value-digest", bare_string(&digest(value))),
    ])
}

fn receipt_request(receipt_key: &str) -> Vec<u8> {
    request(vec![
        ("operation", bare_string("receipt")),
        ("protocol", bare_string(REQUEST_PROTOCOL)),
        ("receipt-key", bare_string(receipt_key)),
    ])
}

fn result_text(result: &[u8], field: &str) -> String {
    let document = Document::parse(result).expect("parse result");
    document
        .root()
        .map_get(field)
        .expect("read result field")
        .expect("result field exists")
        .as_text()
        .expect("result field is text")
        .to_owned()
}

fn result_frame(result: &[u8], field: &str) -> Vec<u8> {
    let document = Document::parse(result).expect("parse result");
    document
        .root()
        .map_get(field)
        .expect("read result field")
        .expect("result field exists")
        .standalone_frame()
}

#[test]
fn preserves_exact_values_receipts_and_replay_across_restart() {
    let database = TempDatabase::new("restart");
    let initial = value(0, "initial");

    {
        let store = provider(&database.path);
        assert_eq!(
            store
                .execute("load", &arguments(load_request()))
                .expect("absent load"),
            frame(&[NIL])
        );
        let initialized = store
            .execute("initialize", &arguments(initialize_request(&initial, 0)))
            .expect("initialize");
        assert_eq!(result_text(&initialized, "operation"), "initialize");
        assert_eq!(result_frame(&initialized, "value"), initial);
    }

    let next = value(1, "next");
    let opaque_receipt = receipt("commit-one", 1);
    let receipt_key = digest(b"commit-one");
    let compare = compare_request(0, 1, &next, &receipt_key, &opaque_receipt);

    {
        let store = provider(&database.path);
        let loaded = store
            .execute("load", &arguments(load_request()))
            .expect("load after reopen");
        assert_eq!(result_frame(&loaded, "value"), initial);

        let applied = store
            .execute("compare-and-swap", &arguments(compare.clone()))
            .expect("apply compare-and-swap");
        assert_eq!(result_text(&applied, "status"), "applied");
        assert_eq!(result_frame(&applied, "receipt"), opaque_receipt);
    }

    {
        let store = provider(&database.path);
        let replayed = store
            .execute("compare-and-swap", &arguments(compare))
            .expect("replay after restart");
        assert_eq!(result_text(&replayed, "status"), "replayed");
        assert_eq!(result_frame(&replayed, "receipt"), opaque_receipt);

        let found = store
            .execute("receipt", &arguments(receipt_request(&receipt_key)))
            .expect("receipt lookup after restart");
        assert_eq!(result_text(&found, "status"), "replayed");
        assert_eq!(result_frame(&found, "receipt"), opaque_receipt);

        let loaded = store
            .execute("load", &arguments(load_request()))
            .expect("load committed value");
        assert_eq!(result_frame(&loaded, "value"), next);
    }
}

#[test]
fn preserves_stale_writer_and_receipt_collision_laws_across_connections() {
    let database = TempDatabase::new("writers");
    let first = provider(&database.path);
    let second = provider(&database.path);
    let initial = value(0, "initial");
    first
        .execute("initialize", &arguments(initialize_request(&initial, 0)))
        .expect("initialize");

    let committed = value(1, "committed");
    let committed_receipt = receipt("winner", 1);
    let committed_key = digest(b"winner");
    first
        .execute(
            "compare-and-swap",
            &arguments(compare_request(
                0,
                1,
                &committed,
                &committed_key,
                &committed_receipt,
            )),
        )
        .expect("first writer wins");

    let stale = value(1, "stale");
    let stale_error = second
        .execute(
            "compare-and-swap",
            &arguments(compare_request(
                0,
                1,
                &stale,
                &digest(b"stale"),
                &receipt("stale", 1),
            )),
        )
        .expect_err("second writer must be stale");
    assert_eq!(stale_error.code(), "store-stale-revision");

    let collision = value(2, "collision");
    let collision_error = second
        .execute(
            "compare-and-swap",
            &arguments(compare_request(
                1,
                2,
                &collision,
                &committed_key,
                &receipt("different", 2),
            )),
        )
        .expect_err("receipt key must bind exact committed input");
    assert_eq!(collision_error.code(), "store-receipt-collision");
}

#[test]
fn rejects_open_requests_and_bad_digests_before_mutation() {
    let database = TempDatabase::new("validation");
    let store = provider(&database.path);

    let open_load = request(vec![
        ("extra", bare_string("forbidden")),
        ("operation", bare_string("load")),
        ("protocol", bare_string(REQUEST_PROTOCOL)),
    ]);
    let open_error = store
        .execute("load", &arguments(open_load))
        .expect_err("open request must fail");
    assert_eq!(open_error.code(), "store-request-invalid");

    let initial = value(0, "initial");
    let bad_digest = request(vec![
        ("operation", bare_string("initialize")),
        ("protocol", bare_string(REQUEST_PROTOCOL)),
        ("revision", bare_i64(0)),
        ("value", initial[MAGIC.len()..].to_vec()),
        ("value-digest", bare_string(&digest(b"different"))),
    ]);
    let digest_error = store
        .execute("initialize", &arguments(bad_digest))
        .expect_err("bad digest must fail");
    assert_eq!(digest_error.code(), "store-digest-mismatch");

    assert_eq!(
        store
            .execute("load", &arguments(load_request()))
            .expect("store remains absent"),
        frame(&[NIL])
    );
}
