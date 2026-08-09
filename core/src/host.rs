use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use hara_wasm::Runtime;
use hara_wasm::{core::Value, hta};
use p256::ecdsa::{Signature as P256Signature, VerifyingKey as P256VerifyingKey};
use sha2::{Digest, Sha256};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn install(runtime: &mut Runtime) {
    runtime.register_resource("hoplite.host", super::app::HOST_SOURCE);
    runtime.install_native_host_handler(Rc::new(dispatch));
}

pub(crate) fn dispatch(service: String, method: String, args: Vec<Value>) -> Result<Value, String> {
    if service != "hoplite.host" {
        return Err(format!("unknown Hoplite host service {service:?}"));
    }
    match method.as_str() {
        "random-bytes" => random_bytes(&args),
        "hash" => hash(&args),
        "canonical-value-digest" => canonical_value_digest(&args),
        "base64url-decode" => base64url_decode(&args),
        "hex-decode" => hex_decode(&args),
        "hex-encode" => hex_encode(&args),
        "p256-jwk-sec1" => p256_jwk_sec1(&args),
        "verify-signature" => verify_signature(&args),
        "now" if args.is_empty() => now().map(Value::Number),
        "secret" => Err("hoplite.host/secret requires an installed secret provider".into()),
        _ => Err(format!("unknown hoplite.host operation {method:?}")),
    }
}

fn base64url_decode(args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("base64url-decode requires one string".into());
    }
    let input = text_arg(args, 0, "base64url value")?;
    if input.is_empty() || input.len() > 1_398_102 || input.contains('=') {
        return Err(
            "base64url value must be non-empty, unpadded, and at most 1 MiB decoded".into(),
        );
    }
    let output = URL_SAFE_NO_PAD
        .decode(input)
        .map_err(|_| "base64url value is invalid".to_string())?;
    if output.len() > 1_048_576 {
        return Err("base64url value exceeds the 1 MiB decoded limit".into());
    }
    Ok(Value::Bytes(output))
}

fn hex_decode(args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("hex-decode requires one lowercase string".into());
    }
    let input = text_arg(args, 0, "hex value")?;
    if input.is_empty() || input.len() > 2_097_152 || input.len() % 2 != 0 {
        return Err("hex value must be non-empty, even-length, and at most 1 MiB decoded".into());
    }
    if !input
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("hex value must use canonical lowercase hexadecimal".into());
    }
    let output = input
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte: u8| {
                if byte <= b'9' {
                    byte - b'0'
                } else {
                    byte - b'a' + 10
                }
            };
            (digit(pair[0]) << 4) | digit(pair[1])
        })
        .collect();
    Ok(Value::Bytes(output))
}

fn hex_encode(args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("hex-encode requires one byte value".into());
    }
    let input = bytes_arg(args, 0, "hex value")?;
    if input.is_empty() || input.len() > 1_048_576 {
        return Err("hex value must contain between 1 byte and 1 MiB".into());
    }
    let mut output = String::with_capacity(input.len() * 2);
    for byte in input {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").map_err(|_| "cannot encode hex".to_string())?;
    }
    Ok(Value::String(output))
}

fn p256_jwk_sec1(args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("p256-jwk-sec1 requires one JSON string".into());
    }
    let input = text_arg(args, 0, "P-256 public JWK")?;
    if input.len() > 2048 {
        return Err("P-256 public JWK exceeds 2048 bytes".into());
    }
    let value: serde_json::Value =
        serde_json::from_str(input).map_err(|_| "P-256 public JWK is invalid JSON".to_string())?;
    let object = value
        .as_object()
        .ok_or_else(|| "P-256 public JWK must be an object".to_string())?;
    let allowed = ["crv", "ext", "key_ops", "kty", "x", "y"];
    if object.len() != allowed.len() || object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("P-256 public JWK fields are invalid".into());
    }
    if object.get("kty").and_then(|value| value.as_str()) != Some("EC")
        || object.get("crv").and_then(|value| value.as_str()) != Some("P-256")
        || object.get("ext").and_then(|value| value.as_bool()) != Some(true)
        || object.get("key_ops") != Some(&serde_json::json!(["verify"]))
    {
        return Err("P-256 public JWK parameters are invalid".into());
    }
    let coordinate = |name: &str| -> Result<Vec<u8>, String> {
        let encoded = object
            .get(name)
            .and_then(|value| value.as_str())
            .ok_or_else(|| format!("P-256 public JWK {name} coordinate is missing"))?;
        let decoded = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| format!("P-256 public JWK {name} coordinate is invalid"))?;
        if decoded.len() != 32 {
            return Err(format!(
                "P-256 public JWK {name} coordinate must contain 32 bytes"
            ));
        }
        Ok(decoded)
    };
    let mut output = Vec::with_capacity(65);
    output.push(4);
    output.extend(coordinate("x")?);
    output.extend(coordinate("y")?);
    P256VerifyingKey::from_sec1_bytes(&output)
        .map_err(|_| "P-256 public JWK point is invalid".to_string())?;
    Ok(Value::Bytes(output))
}

fn random_bytes(args: &[Value]) -> Result<Value, String> {
    let size = number_arg(args, 0, "random-bytes size")?;
    if args.len() != 1 || !(1..=4096).contains(&size) {
        return Err("random-bytes size must be between 1 and 4096".into());
    }
    let mut output = vec![0_u8; size as usize];
    getrandom::getrandom(&mut output)
        .map_err(|error| format!("secure randomness unavailable: {error}"))?;
    Ok(Value::Bytes(output))
}

fn hash(args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 || text_arg(args, 0, "hash algorithm")? != "sha256" {
        return Err("hash currently supports only sha256".into());
    }
    let input = data_arg(args, 1, "hash value")?;
    Ok(Value::Bytes(Sha256::digest(&input).to_vec()))
}

fn canonical_value_digest(args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("canonical-value-digest requires one portable value".into());
    }
    let frame = hta::encode(&args[0])
        .map_err(|error| format!("cannot encode canonical HTA value: {error}"))?;
    if frame.len() > 8 * 1024 * 1024 {
        return Err("canonical HTA value exceeds the 8 MiB limit".into());
    }
    Ok(Value::String(format!("sha256:{:x}", Sha256::digest(frame))))
}

fn verify_signature(args: &[Value]) -> Result<Value, String> {
    if args.len() != 4 {
        return Err(
            "verify-signature requires algorithm, public key, message, and signature".into(),
        );
    }
    let algorithm = text_arg(args, 0, "signature algorithm")?;
    let message = data_arg(args, 2, "signed message")?;
    match algorithm {
        "ed25519" => {
            let public_key = fixed_bytes::<32>(args, 1, "Ed25519 public key")?;
            let signature = fixed_bytes::<64>(args, 3, "Ed25519 signature")?;
            let key = VerifyingKey::from_bytes(&public_key)
                .map_err(|_| "Ed25519 public key is invalid".to_string())?;
            Ok(Value::Bool(
                key.verify(&message, &Signature::from_bytes(&signature))
                    .is_ok(),
            ))
        }
        "p256-sha256" => {
            let public_key = fixed_bytes::<65>(args, 1, "P-256 SEC1 public key")?;
            let signature = fixed_bytes::<64>(args, 3, "P-256 P1363 signature")?;
            let key = P256VerifyingKey::from_sec1_bytes(&public_key)
                .map_err(|_| "P-256 SEC1 public key is invalid".to_string())?;
            let signature = P256Signature::from_slice(&signature)
                .map_err(|_| "P-256 P1363 signature is invalid".to_string())?;
            Ok(Value::Bool(key.verify(&message, &signature).is_ok()))
        }
        _ => Err("verify-signature supports only ed25519 and p256-sha256".into()),
    }
}

fn now() -> Result<i64, String> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before the Unix epoch".to_string())?;
    i64::try_from(elapsed.as_secs()).map_err(|_| "system clock is out of range".into())
}

fn number_arg(args: &[Value], index: usize, label: &str) -> Result<i64, String> {
    match args.get(index) {
        Some(Value::Number(value)) => Ok(*value),
        _ => Err(format!("{label} must be an integer")),
    }
}

fn text_arg<'a>(args: &'a [Value], index: usize, label: &str) -> Result<&'a str, String> {
    match args.get(index) {
        Some(Value::String(value)) => Ok(value),
        _ => Err(format!("{label} must be a string")),
    }
}

fn bytes_arg<'a>(args: &'a [Value], index: usize, label: &str) -> Result<&'a [u8], String> {
    match args.get(index) {
        Some(Value::Bytes(value)) => Ok(value),
        _ => Err(format!("{label} must be bytes")),
    }
}

fn data_arg(args: &[Value], index: usize, label: &str) -> Result<Vec<u8>, String> {
    match args.get(index) {
        Some(Value::Bytes(value)) => Ok(value.clone()),
        Some(Value::ByteBuffer(value)) => Ok(value.borrow().clone()),
        Some(Value::String(value)) => Ok(value.as_bytes().to_vec()),
        _ => Err(format!("{label} must be bytes or a string")),
    }
}

fn fixed_bytes<const N: usize>(
    args: &[Value],
    index: usize,
    label: &str,
) -> Result<[u8; N], String> {
    bytes_arg(args, index, label)?
        .try_into()
        .map_err(|_| format!("{label} must contain {N} bytes"))
}

#[cfg(test)]
mod tests {
    use super::{dispatch, install};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use hara_wasm::core::Value;
    use hara_wasm::Runtime;
    use p256::ecdsa::{signature::Signer, Signature, SigningKey};
    use sha2::{Digest, Sha256};

    #[test]
    fn hal_host_capabilities_use_the_native_boundary() {
        let mut runtime = Runtime::new();
        install(&mut runtime);
        let value = runtime
            .eval_native_value(
                "(ns host-test (:require [hoplite.host :as host]\n\
                                         [std.foundation.string :as str]))\n\
                 [(count (host/random-bytes 32))\n\
                  (count (host/hash \"sha256\" (str/encode-utf8 \"hoplite\")))\n\
                  (count (host/canonical-value-digest {:ready true}))\n\
                  (number? (host/now))]",
            )
            .expect("host calls evaluate");
        assert_eq!(value.display(), "[32 32 71 true]");
    }

    #[test]
    fn hashes_the_exact_canonical_hta_value() {
        let value = Value::Map(
            [
                (Value::Keyword("revision".into()), Value::Number(3)),
                (Value::Keyword("ready".into()), Value::Bool(true)),
            ]
            .into_iter()
            .collect(),
        );
        let expected = format!(
            "sha256:{:x}",
            Sha256::digest(hara_wasm::hta::encode(&value).expect("fixture encodes"))
        );
        assert_eq!(
            dispatch(
                "hoplite.host".into(),
                "canonical-value-digest".into(),
                vec![value],
            )
            .expect("canonical value digest evaluates"),
            Value::String(expected)
        );
        assert!(dispatch(
            "hoplite.host".into(),
            "canonical-value-digest".into(),
            Vec::new(),
        )
        .is_err());
    }

    #[test]
    fn secret_access_is_closed_without_a_provider() {
        let mut runtime = Runtime::new();
        install(&mut runtime);
        let error = runtime
            .eval_native("(ns host-secret (:require [hoplite.host :as host])) (host/secret \"auth/signing-key\")")
            .expect_err("secret access must fail closed");
        assert!(error.contains("requires an installed secret provider"));
    }

    #[test]
    fn verifies_browser_compatible_p256_p1363_signatures() {
        let signing_key = SigningKey::from_bytes((&[7_u8; 32]).into()).expect("fixed test key");
        let message = b"tahto.device-request/1\nsha256:fixture";
        let signature: Signature = signing_key.sign(message);
        let public_key = signing_key.verifying_key().to_encoded_point(false);
        let verified = dispatch(
            "hoplite.host".into(),
            "verify-signature".into(),
            vec![
                Value::String("p256-sha256".into()),
                Value::Bytes(public_key.as_bytes().to_vec()),
                Value::Bytes(message.to_vec()),
                Value::Bytes(signature.to_bytes().to_vec()),
            ],
        )
        .expect("P-256 verification evaluates");
        assert_eq!(verified, Value::Bool(true));

        let rejected = dispatch(
            "hoplite.host".into(),
            "verify-signature".into(),
            vec![
                Value::String("p256-sha256".into()),
                Value::Bytes(public_key.as_bytes().to_vec()),
                Value::Bytes(b"tampered".to_vec()),
                Value::Bytes(signature.to_bytes().to_vec()),
            ],
        )
        .expect("invalid signatures return false");
        assert_eq!(rejected, Value::Bool(false));
    }

    #[test]
    fn decodes_only_bounded_unpadded_base64url() {
        assert_eq!(
            dispatch(
                "hoplite.host".into(),
                "base64url-decode".into(),
                vec![Value::String("aGVsbG8td29ybGQ".into())],
            )
            .expect("canonical base64url decodes"),
            Value::Bytes(b"hello-world".to_vec())
        );
        assert!(dispatch(
            "hoplite.host".into(),
            "base64url-decode".into(),
            vec![Value::String("aGVsbG8=".into())],
        )
        .is_err());
    }

    #[test]
    fn converts_a_strict_public_p256_jwk_to_sec1() {
        let signing_key = SigningKey::from_bytes((&[9_u8; 32]).into()).expect("fixed test key");
        let point = signing_key.verifying_key().to_encoded_point(false);
        let jwk = serde_json::json!({
            "crv": "P-256",
            "ext": true,
            "key_ops": ["verify"],
            "kty": "EC",
            "x": URL_SAFE_NO_PAD.encode(point.x().expect("x coordinate")),
            "y": URL_SAFE_NO_PAD.encode(point.y().expect("y coordinate")),
        });
        assert_eq!(
            dispatch(
                "hoplite.host".into(),
                "p256-jwk-sec1".into(),
                vec![Value::String(jwk.to_string())],
            )
            .expect("strict JWK converts"),
            Value::Bytes(point.as_bytes().to_vec())
        );
    }

    #[test]
    fn decodes_only_canonical_bounded_hex() {
        assert_eq!(
            dispatch(
                "hoplite.host".into(),
                "hex-decode".into(),
                vec![Value::String("00a1ff".into())],
            )
            .expect("canonical hex decodes"),
            Value::Bytes(vec![0, 161, 255])
        );
        assert!(dispatch(
            "hoplite.host".into(),
            "hex-decode".into(),
            vec![Value::String("A1".into())],
        )
        .is_err());
        assert_eq!(
            dispatch(
                "hoplite.host".into(),
                "hex-encode".into(),
                vec![Value::Bytes(vec![0, 161, 255])],
            )
            .expect("bytes encode as lowercase hex"),
            Value::String("00a1ff".into())
        );
    }
}
