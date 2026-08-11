use ed25519_dalek::{Signer, SigningKey};
use hoplite_data_plane_abi::{SignedDeviceRequest, SIGNED_DEVICE_PROFILE};
use hoplite_signed_device_worker::{
    ConfigurationError, IngressError, RoutePolicy, WorkerIngress, AUTHORITY_HEADER,
    CONTENT_DIGEST_HEADER, IDEMPOTENCY_HEADER, KEY_ID_HEADER, NONCE_HEADER, PROFILE_HEADER,
    PROJECTED_IDENTITY_PROFILE, SIGNATURE_HEADER, TIMESTAMP_HEADER,
};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const LOCK_DIGEST: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn temp_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "hoplite-signed-worker-{label}-{}-{}",
        std::process::id(),
        now()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn write_config(root: &std::path::Path, key: &SigningKey, revoked_at: Option<i64>) -> PathBuf {
    let path = root.join("keys.json");
    let revoked = revoked_at
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_owned());
    fs::write(
        &path,
        format!(
            r#"{{
  "profile": "hoplite-signed-device-keys/0-alpha",
  "freshness": {{
    "max-past-seconds": 300,
    "max-future-seconds": 30
  }},
  "keys": [{{
    "key-id": "key.device-a",
    "subject": "profile.primary",
    "realm": "application",
    "device-id": "device.a",
    "public-key": "{}",
    "claims": {{
      "application/id": "fixture.transfer",
      "application/version": "1.0.0",
      "application/publisher": "greenways.example",
      "application/lock-digest": "{}",
      "application/namespace": "world.shared",
      "application/collection": "objects",
      "application/operations": "object.upload,object.read",
      "device/label": "Device-A",
      "provider/private": "must-not-project"
    }},
    "not-before": null,
    "expires-at": null,
    "revoked-at": {}
  }}]
}}
"#,
            hex(key.verifying_key().as_bytes()),
            LOCK_DIGEST,
            revoked
        ),
    )
    .unwrap();
    path
}

fn route() -> RoutePolicy {
    RoutePolicy::new(
        "object.upload",
        "fixture.transfer",
        "world.shared",
        "objects",
    )
    .unwrap()
}

fn signed_headers(
    key: &SigningKey,
    timestamp: i64,
    nonce: &str,
    idempotency: &str,
) -> Vec<(String, String)> {
    let unsigned = SignedDeviceRequest {
        method: "PUT",
        target: "/objects?mode=install",
        authority: "fixture.local",
        content_digest: DIGEST,
        operation: "object.upload",
        application: "fixture.transfer",
        namespace: "world.shared",
        collection: "objects",
        timestamp,
        nonce,
        idempotency_key: idempotency,
        key_id: "key.device-a",
        signature: "",
    };
    let signature = hex(&key
        .sign(unsigned.signing_input().unwrap().as_bytes())
        .to_bytes());
    vec![
        (AUTHORITY_HEADER.into(), "fixture.local".into()),
        (PROFILE_HEADER.into(), SIGNED_DEVICE_PROFILE.into()),
        (CONTENT_DIGEST_HEADER.into(), DIGEST.into()),
        (TIMESTAMP_HEADER.into(), timestamp.to_string()),
        (NONCE_HEADER.into(), nonce.into()),
        (IDEMPOTENCY_HEADER.into(), idempotency.into()),
        (KEY_ID_HEADER.into(), "key.device-a".into()),
        (SIGNATURE_HEADER.into(), signature),
    ]
}

#[test]
fn valid_request_projects_only_closed_identity_and_replay_evidence() {
    let root = temp_root("valid");
    let key = SigningKey::from_bytes(&[7; 32]);
    let keys = write_config(&root, &key, None);
    let replay = root.join("replay.db");
    let timestamp = now();
    let headers = signed_headers(
        &key,
        timestamp,
        "nonce-device-a-0001",
        "request-device-a-0001",
    );
    let mut worker = WorkerIngress::open(keys, &replay).unwrap();
    let first = worker
        .authenticate("PUT", "/objects?mode=install", &headers, &route())
        .unwrap();
    let second = worker
        .authenticate("PUT", "/objects?mode=install", &headers, &route())
        .unwrap();

    assert_eq!(first.profile(), PROJECTED_IDENTITY_PROFILE);
    assert_eq!(first.device_id(), "device.a");
    assert_eq!(first.operation(), "object.upload");
    assert_eq!(first.replay_status(), "applied");
    assert_eq!(second.replay_status(), "replayed");
    assert_eq!(first.request_fingerprint(), second.request_fingerprint());
    assert_eq!(first.admitted_at(), second.admitted_at());
    assert_eq!(first.claims().get("device/label").unwrap(), "Device-A");
    assert!(!first.claims().contains_key("provider/private"));
    let debug = format!("{first:?}");
    assert!(!debug.contains("nonce-device-a"));
    assert!(!debug.contains("request-device-a"));
    assert!(!debug.contains("provider/private"));
    assert!(!debug.contains(replay.to_string_lossy().as_ref()));
}

#[test]
fn replay_survives_a_fresh_worker_process_boundary() {
    let root = temp_root("restart");
    let key = SigningKey::from_bytes(&[8; 32]);
    let keys = write_config(&root, &key, None);
    let replay = root.join("replay.db");
    let timestamp = now();
    let headers = signed_headers(
        &key,
        timestamp,
        "nonce-device-a-0002",
        "request-device-a-0002",
    );
    let first = WorkerIngress::open(&keys, &replay)
        .unwrap()
        .authenticate("PUT", "/objects?mode=install", &headers, &route())
        .unwrap();
    let second = WorkerIngress::open(&keys, &replay)
        .unwrap()
        .authenticate("PUT", "/objects?mode=install", &headers, &route())
        .unwrap();
    assert_eq!(first.replay_status(), "applied");
    assert_eq!(second.replay_status(), "replayed");
    assert_eq!(first.admitted_at(), second.admitted_at());
}

#[test]
fn wire_headers_are_exact_bounded_and_case_insensitive() {
    let root = temp_root("wire");
    let key = SigningKey::from_bytes(&[9; 32]);
    let keys = write_config(&root, &key, None);
    let replay = root.join("replay.db");
    let timestamp = now();
    let mut headers = signed_headers(
        &key,
        timestamp,
        "nonce-device-a-0003",
        "request-device-a-0003",
    );
    headers[0].0 = "Host".into();
    headers[1].0 = "X-Hoplite-Signature-Profile".into();
    let mut worker = WorkerIngress::open(keys, replay).unwrap();
    assert!(worker
        .authenticate("PUT", "/objects?mode=install", &headers, &route())
        .is_ok());

    let missing = headers
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case(NONCE_HEADER))
        .cloned()
        .collect::<Vec<_>>();
    let error = worker
        .authenticate("PUT", "/objects?mode=install", &missing, &route())
        .unwrap_err();
    assert_eq!(error.status(), 401);
    assert_eq!(error.code(), "signed-device-worker/header-missing");

    let mut duplicate = headers.clone();
    duplicate.push(("HOST".into(), "fixture.local".into()));
    assert_eq!(
        worker
            .authenticate("PUT", "/objects?mode=install", &duplicate, &route())
            .unwrap_err()
            .code(),
        "signed-device-worker/header-duplicate"
    );
}

#[test]
fn route_and_actual_exchange_are_part_of_the_signature() {
    let root = temp_root("binding");
    let key = SigningKey::from_bytes(&[10; 32]);
    let keys = write_config(&root, &key, None);
    let replay = root.join("replay.db");
    let timestamp = now();
    let headers = signed_headers(
        &key,
        timestamp,
        "nonce-device-a-0004",
        "request-device-a-0004",
    );
    let mut worker = WorkerIngress::open(keys, replay).unwrap();
    for (method, target, policy) in [
        ("POST", "/objects?mode=install", route()),
        ("PUT", "/objects?mode=other", route()),
        (
            "PUT",
            "/objects?mode=install",
            RoutePolicy::new("object.read", "fixture.transfer", "world.shared", "objects").unwrap(),
        ),
    ] {
        let error = worker
            .authenticate(method, target, &headers, &policy)
            .unwrap_err();
        assert_eq!(error.status(), 401);
    }
}

#[test]
fn nonce_reuse_and_idempotency_collision_have_stable_conflict_status() {
    let root = temp_root("conflict");
    let key = SigningKey::from_bytes(&[11; 32]);
    let keys = write_config(&root, &key, None);
    let replay = root.join("replay.db");
    let timestamp = now();
    let mut worker = WorkerIngress::open(keys, replay).unwrap();
    let first = signed_headers(
        &key,
        timestamp,
        "nonce-device-a-0005",
        "request-device-a-0005",
    );
    worker
        .authenticate("PUT", "/objects?mode=install", &first, &route())
        .unwrap();

    let nonce_reuse = signed_headers(
        &key,
        timestamp,
        "nonce-device-a-0005",
        "request-device-a-0006",
    );
    let error = worker
        .authenticate("PUT", "/objects?mode=install", &nonce_reuse, &route())
        .unwrap_err();
    assert_eq!(error.status(), 409);

    let collision = signed_headers(
        &key,
        timestamp,
        "nonce-device-a-0006",
        "request-device-a-0005",
    );
    let error = worker
        .authenticate("PUT", "/objects?mode=install", &collision, &route())
        .unwrap_err();
    assert_eq!(error.status(), 409);
}

#[test]
fn revoked_keys_fail_before_replay_mutation() {
    let root = temp_root("revoked");
    let key = SigningKey::from_bytes(&[12; 32]);
    let timestamp = now();
    let keys = write_config(&root, &key, Some(timestamp));
    let replay = root.join("replay.db");
    let headers = signed_headers(
        &key,
        timestamp,
        "nonce-device-a-0007",
        "request-device-a-0007",
    );
    let mut worker = WorkerIngress::open(keys, &replay).unwrap();
    let error = worker
        .authenticate("PUT", "/objects?mode=install", &headers, &route())
        .unwrap_err();
    assert_eq!(error.status(), 401);
    assert_eq!(error.code(), "hoplite.signed-device/revoked-key");

    let key = SigningKey::from_bytes(&[13; 32]);
    let keys = write_config(&root, &key, None);
    let headers = signed_headers(&key, now(), "nonce-device-a-0007", "request-device-a-0007");
    let result = WorkerIngress::open(keys, replay)
        .unwrap()
        .authenticate("PUT", "/objects?mode=install", &headers, &route())
        .unwrap();
    assert_eq!(result.replay_status(), "applied");
}

#[test]
fn configuration_is_closed_and_rejects_private_or_partial_authority() {
    let root = temp_root("config");
    let key = SigningKey::from_bytes(&[14; 32]);
    let path = write_config(&root, &key, None);
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["keys"][0]["private-key"] = serde_json::Value::String("secret".into());
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        WorkerIngress::open(path, root.join("replay.db"))
            .unwrap_err()
            .code(),
        ConfigurationError::Shape.code()
    );
}

#[test]
fn errors_and_debug_output_do_not_leak_signed_authority() {
    let error = IngressError::InvalidHeader(SIGNATURE_HEADER);
    let debug = format!("{error:?}");
    let display = error.to_string();
    assert!(!debug.contains("secret-signature"));
    assert!(!display.contains(SIGNATURE_HEADER));
}
