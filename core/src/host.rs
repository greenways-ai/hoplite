use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use hara_wasm::core::Value;
use hara_wasm::Runtime;
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
        "verify-signature" => verify_signature(&args),
        "now" if args.is_empty() => now().map(Value::Number),
        "secret" => Err("hoplite.host/secret requires an installed secret provider".into()),
        _ => Err(format!("unknown hoplite.host operation {method:?}")),
    }
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

fn verify_signature(args: &[Value]) -> Result<Value, String> {
    if args.len() != 4 || text_arg(args, 0, "signature algorithm")? != "ed25519" {
        return Err("verify-signature currently supports only ed25519".into());
    }
    let public_key = fixed_bytes::<32>(args, 1, "Ed25519 public key")?;
    let message = data_arg(args, 2, "signed message")?;
    let signature = fixed_bytes::<64>(args, 3, "Ed25519 signature")?;
    let key = VerifyingKey::from_bytes(&public_key)
        .map_err(|_| "Ed25519 public key is invalid".to_string())?;
    Ok(Value::Bool(
        key.verify(&message, &Signature::from_bytes(&signature))
            .is_ok(),
    ))
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
    use super::install;
    use hara_wasm::Runtime;

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
                  (number? (host/now))]",
            )
            .expect("host calls evaluate");
        assert_eq!(value.display(), "[32 32 true]");
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
}
