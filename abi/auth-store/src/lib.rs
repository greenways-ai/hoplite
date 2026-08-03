//! Canonical, language-neutral HTA contract for Hoplite authentication stores.

use hara_wasm::core::{self, Value};
use hara_wasm::lang::data::Vector as PVector;
use sha2::{Digest, Sha256};

pub const ABI_ID: &str = "hoplite/auth-store";
pub const ABI_VERSION: &str = "1.0.0";
pub const TRANSPORT: &str = "hta.v1";
pub const NATIVE_ABI: &str = "hoplite-auth-store/1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Query,
    Transact,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Transact => "transact",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Operation {
    pub name: &'static str,
    pub mode: Mode,
    pub input: &'static str,
    pub output: &'static str,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub id: String,
    pub operation: Operation,
    pub payload: Value,
}

pub const OPERATIONS: [Operation; 9] = [
    define_operation(
        "auth/audit-append",
        Mode::Transact,
        "auth/AuditAppend",
        "auth/AuditEvent",
    ),
    define_operation(
        "auth/challenge-consume",
        Mode::Transact,
        "auth/ChallengeConsume",
        "auth/Challenge",
    ),
    define_operation(
        "auth/challenge-put",
        Mode::Transact,
        "auth/Challenge",
        "auth/Challenge",
    ),
    define_operation(
        "auth/device-put",
        Mode::Transact,
        "auth/Device",
        "auth/Device",
    ),
    define_operation(
        "auth/refresh-rotate",
        Mode::Transact,
        "auth/RefreshRotate",
        "auth/Session",
    ),
    define_operation(
        "auth/session-put",
        Mode::Transact,
        "auth/Session",
        "auth/Session",
    ),
    define_operation(
        "auth/session-revoke",
        Mode::Transact,
        "auth/SessionRevoke",
        "auth/Session",
    ),
    define_operation("auth/user-create", Mode::Transact, "auth/User", "auth/User"),
    define_operation(
        "auth/user-find",
        Mode::Query,
        "auth/UserFind",
        "auth/UserMaybe",
    ),
];

const fn define_operation(
    name: &'static str,
    mode: Mode,
    input: &'static str,
    output: &'static str,
) -> Operation {
    Operation {
        name,
        mode,
        input,
        output,
    }
}

pub fn contract() -> Result<Vec<u8>, String> {
    hara_wasm::hta::encode(&value())
}

pub fn operation(name: &str) -> Option<&'static Operation> {
    OPERATIONS.iter().find(|operation| operation.name == name)
}

pub fn encode_request(id: &str, operation_name: &str, payload: Value) -> Result<Vec<u8>, String> {
    validate_id(id, "request")?;
    operation(operation_name)
        .ok_or_else(|| format!("auth-store/operation-unknown: {operation_name}"))?;
    require_map(&payload, "request payload")?;
    hara_wasm::hta::encode(&request_value(id, operation_name, payload))
}

pub fn decode_request(bytes: &[u8]) -> Result<Request, String> {
    let value = hara_wasm::hta::decode(bytes)?;
    let id = string_field(&value, "request/id")?;
    validate_id(&id, "request")?;
    let operation_name = keyword_field(&value, "request/operation")?;
    let operation = operation(&operation_name)
        .copied()
        .ok_or_else(|| format!("auth-store/operation-unknown: {operation_name}"))?;
    let payload = field(&value, "request/payload")
        .ok_or("auth-store/request-malformed: missing :request/payload")?;
    require_map(&payload, "request payload")?;
    Ok(Request {
        id,
        operation,
        payload,
    })
}

pub fn encode_transaction(id: &str, operations: Vec<(&str, Value)>) -> Result<Vec<u8>, String> {
    validate_id(id, "transaction")?;
    if operations.is_empty() {
        return Err("auth-store/transaction-empty".into());
    }
    let requests = operations
        .into_iter()
        .enumerate()
        .map(|(index, (name, payload))| {
            let operation =
                operation(name).ok_or_else(|| format!("auth-store/operation-unknown: {name}"))?;
            if operation.mode != Mode::Transact {
                return Err(format!("auth-store/transaction-query: {name}"));
            }
            require_map(&payload, "transaction payload")?;
            Ok(request_value(&format!("{id}/{index}"), name, payload))
        })
        .collect::<Result<Vec<_>, String>>()?;
    hara_wasm::hta::encode(&map(vec![
        (keyword("transaction/id"), Value::String(id.into())),
        (
            keyword("transaction/operations"),
            Value::Vector(PVector::from(requests)),
        ),
    ]))
}

pub fn sha256() -> Result<String, String> {
    Ok(format!("sha256:{:x}", Sha256::digest(contract()?)))
}

fn value() -> Value {
    map(vec![
        (keyword("abi/id"), keyword(ABI_ID)),
        (keyword("abi/version"), Value::String(ABI_VERSION.into())),
        (keyword("abi/transport"), keyword(TRANSPORT)),
        (keyword("abi/native"), Value::String(NATIVE_ABI.into())),
        (
            keyword("abi/request"),
            record(vec![
                ("request/id", "string", true),
                ("request/operation", "keyword", true),
                ("request/payload", "map", true),
            ]),
        ),
        (
            keyword("abi/response"),
            record(vec![
                ("response/id", "string", true),
                ("response/result", "any", false),
                ("response/error", "map", false),
            ]),
        ),
        (
            keyword("abi/operations"),
            map(OPERATIONS
                .iter()
                .map(|operation| {
                    (
                        keyword(operation.name),
                        map(vec![
                            (keyword("operation/mode"), keyword(operation.mode.name())),
                            (keyword("operation/input"), keyword(operation.input)),
                            (keyword("operation/output"), keyword(operation.output)),
                        ]),
                    )
                })
                .collect()),
        ),
    ])
}

fn request_value(id: &str, operation: &str, payload: Value) -> Value {
    map(vec![
        (keyword("request/id"), Value::String(id.into())),
        (keyword("request/operation"), keyword(operation)),
        (keyword("request/payload"), payload),
    ])
}

fn field(value: &Value, name: &str) -> Option<Value> {
    core::map_entries(value)?
        .into_iter()
        .find_map(|(key, value)| {
            matches!(&key, Value::Keyword(keyword) if keyword.as_str() == name).then_some(value)
        })
}

fn string_field(value: &Value, name: &str) -> Result<String, String> {
    match field(value, name) {
        Some(Value::String(value)) => Ok(value),
        value => Err(format!(
            "auth-store/request-malformed: :{name} must be a string, got {value:?}"
        )),
    }
}

fn keyword_field(value: &Value, name: &str) -> Result<String, String> {
    match field(value, name) {
        Some(Value::Keyword(value)) => Ok(value.as_str().into()),
        value => Err(format!(
            "auth-store/request-malformed: :{name} must be a keyword, got {value:?}"
        )),
    }
}

fn require_map(value: &Value, label: &str) -> Result<(), String> {
    core::map_entries(value)
        .map(|_| ())
        .ok_or_else(|| format!("auth-store/request-malformed: {label} must be a map"))
}

fn validate_id(id: &str, label: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 128 || id.chars().any(char::is_whitespace) {
        Err(format!(
            "auth-store/{label}-id: must be 1-128 non-whitespace characters"
        ))
    } else {
        Ok(())
    }
}

fn record(fields: Vec<(&str, &str, bool)>) -> Value {
    map(vec![
        (keyword("type/kind"), keyword("record")),
        (
            keyword("type/fields"),
            Value::Vector(PVector::from(
                fields
                    .into_iter()
                    .map(|(name, field_type, required)| {
                        map(vec![
                            (keyword("field/name"), keyword(name)),
                            (keyword("field/type"), keyword(field_type)),
                            (keyword("field/required"), Value::Bool(required)),
                        ])
                    })
                    .collect::<Vec<_>>(),
            )),
        ),
    ])
}

fn keyword(name: &str) -> Value {
    Value::Keyword(name.into())
}

fn map(entries: Vec<(Value, Value)>) -> Value {
    Value::Map(entries.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn contract_is_canonical_hta_with_unique_operations() {
        let first = contract().unwrap();
        let second = contract().unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with(b"HTA1"));
        assert_eq!(
            hara_wasm::hta::decode(&first).unwrap().display(),
            value().display()
        );
        assert_eq!(
            OPERATIONS
                .iter()
                .map(|operation| operation.name)
                .collect::<BTreeSet<_>>()
                .len(),
            OPERATIONS.len()
        );
        assert!(sha256().unwrap().starts_with("sha256:"));
    }

    #[test]
    fn only_reads_are_queries() {
        assert_eq!(
            OPERATIONS
                .iter()
                .filter(|operation| operation.mode == Mode::Query)
                .map(|operation| operation.name)
                .collect::<Vec<_>>(),
            vec!["auth/user-find"]
        );
    }

    #[test]
    fn request_round_trips_through_hta_and_rejects_unknown_operations() {
        let payload = map(vec![(
            keyword("auth/realm"),
            Value::String("management".into()),
        )]);
        let encoded = encode_request("req-1", "auth/user-find", payload).unwrap();
        let request = decode_request(&encoded).unwrap();
        assert_eq!(request.id, "req-1");
        assert_eq!(request.operation.name, "auth/user-find");
        assert_eq!(request.operation.mode, Mode::Query);
        assert!(encode_request("req-2", "auth/not-real", map(vec![])).is_err());
    }

    #[test]
    fn transaction_batches_only_accept_mutations() {
        let payload = map(vec![(keyword("auth/id"), Value::String("usr-1".into()))]);
        assert!(
            encode_transaction("txn-1", vec![("auth/user-create", payload.clone())])
                .unwrap()
                .starts_with(b"HTA1")
        );
        assert!(
            encode_transaction("txn-2", vec![("auth/user-find", payload)])
                .unwrap_err()
                .contains("transaction-query")
        );
    }
}
