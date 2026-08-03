use hara_wasm::core::Value;
use hara_wasm::lang::data::Vector as PVector;
use sha2::{Digest, Sha256};

pub fn contract() -> Result<Vec<u8>, String> {
    hara_wasm::hta::encode(&value())
}

pub fn sha256() -> Result<String, String> {
    Ok(format!("sha256:{:x}", Sha256::digest(contract()?)))
}

fn value() -> Value {
    map(vec![
        (keyword("abi/id"), keyword(hoplite_auth_store_abi::ABI_ID)),
        (
            keyword("abi/version"),
            Value::String(hoplite_auth_store_abi::ABI_VERSION.into()),
        ),
        (
            keyword("abi/transport"),
            keyword(hoplite_auth_store_abi::TRANSPORT),
        ),
        (
            keyword("abi/native"),
            Value::String(hoplite_auth_store_abi::NATIVE_ABI.into()),
        ),
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
            map(hoplite_auth_store_abi::OPERATIONS
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

    #[test]
    fn contract_is_canonical_hta() {
        let first = contract().unwrap();
        assert_eq!(first, contract().unwrap());
        assert!(first.starts_with(b"HTA1"));
        assert_eq!(
            hara_wasm::hta::decode(&first).unwrap().display(),
            value().display()
        );
        assert!(sha256().unwrap().starts_with("sha256:"));
    }
}
