use hara_wasm::core::Value;
use hara_wasm::lang::data::Vector as PVector;
use sha2::{Digest, Sha256};

pub fn contract() -> Result<Vec<u8>, String> {
    hara_wasm::hta::encode(&value())
}

pub fn sha256() -> Result<String, String> {
    Ok(format!("sha256:{:x}", Sha256::digest(contract()?)))
}

pub fn decode_request_payload(
    request: &hoplite_auth_store_abi::Request,
) -> Result<Value, hoplite_auth_store_abi::Error> {
    let value = hara_wasm::hta::decode(&request.payload_hta)
        .map_err(|error| hoplite_auth_store_abi::Error::new("payload-invalid", error))?;
    validate_record(request.operation.input, &value)?;
    Ok(value)
}

pub fn decode_response_result(
    request: &hoplite_auth_store_abi::Request,
    response: &hoplite_auth_store_abi::Response,
) -> Result<Option<Value>, hoplite_auth_store_abi::Error> {
    if response.id != request.id {
        return Err(hoplite_auth_store_abi::Error::new(
            "response-id-mismatch",
            format!("expected {}, got {}", request.id, response.id),
        ));
    }
    match (&response.result_hta, &response.error) {
        (Some(result), None) => {
            let value = hara_wasm::hta::decode(result)
                .map_err(|error| hoplite_auth_store_abi::Error::new("result-invalid", error))?;
            validate_record(request.operation.output, &value)?;
            Ok(Some(value))
        }
        (None, Some(_)) => Ok(None),
        _ => Err(hoplite_auth_store_abi::Error::new(
            "response-invalid",
            "response must contain exactly one of result or error",
        )),
    }
}

pub fn execute<A: hoplite_auth_store_abi::Adapter + ?Sized>(
    adapter: &mut A,
    request: hoplite_auth_store_abi::Request,
) -> Result<hoplite_auth_store_abi::Response, hoplite_auth_store_abi::Error> {
    decode_request_payload(&request)?;
    let response = adapter.execute(request.clone())?;
    decode_response_result(&request, &response)?;
    Ok(response)
}

pub fn transact<A: hoplite_auth_store_abi::Adapter + ?Sized>(
    adapter: &mut A,
    transaction: hoplite_auth_store_abi::Transaction,
) -> Result<hoplite_auth_store_abi::TransactionResponse, hoplite_auth_store_abi::Error> {
    for request in &transaction.operations {
        decode_request_payload(request)?;
    }
    let response = adapter.transact(transaction.clone())?;
    if response.id != transaction.id {
        return Err(hoplite_auth_store_abi::Error::new(
            "transaction-response-id-mismatch",
            format!("expected {}, got {}", transaction.id, response.id),
        ));
    }
    hoplite_auth_store_abi::TransactionResponse::new(&transaction, response.responses.clone())?;
    for (request, response) in transaction.operations.iter().zip(&response.responses) {
        decode_response_result(request, response)?;
    }
    Ok(response)
}

fn validate_record(record_name: &str, value: &Value) -> Result<(), hoplite_auth_store_abi::Error> {
    let record = hoplite_auth_store_abi::record_type(record_name)
        .ok_or_else(|| hoplite_auth_store_abi::Error::new("type-unknown", record_name))?;
    let entries = hara_wasm::core::map_entries(value).ok_or_else(|| {
        hoplite_auth_store_abi::Error::new(
            "payload-type",
            format!("{record_name} must be an HTA map"),
        )
    })?;
    let mut present = std::collections::BTreeMap::new();
    for (key, value) in entries {
        let Value::Keyword(key) = key else {
            return Err(hoplite_auth_store_abi::Error::new(
                "field-name",
                format!("{record_name} fields must use keywords"),
            ));
        };
        let name = key.as_str().to_owned();
        if !record.fields.iter().any(|field| field.name == name) {
            return Err(hoplite_auth_store_abi::Error::new(
                "field-unknown",
                format!("{record_name} does not define :{name}"),
            ));
        }
        if present.insert(name.clone(), value).is_some() {
            return Err(hoplite_auth_store_abi::Error::new(
                "field-duplicate",
                format!("{record_name} contains :{name} more than once"),
            ));
        }
    }
    for field in record.fields {
        match present.get(field.name) {
            None if field.required => {
                return Err(hoplite_auth_store_abi::Error::new(
                    "field-required",
                    format!("{record_name} requires :{}", field.name),
                ))
            }
            Some(value) if !value_matches(field.field_type, value)? => {
                return Err(hoplite_auth_store_abi::Error::new(
                    "field-type",
                    format!("{record_name} :{} must be {}", field.name, field.field_type),
                ))
            }
            _ => {}
        }
    }
    Ok(())
}

fn value_matches(field_type: &str, value: &Value) -> Result<bool, hoplite_auth_store_abi::Error> {
    match field_type {
        "string" => Ok(matches!(value, Value::String(_))),
        "integer" => Ok(matches!(value, Value::Number(_))),
        "bytes" => Ok(matches!(value, Value::Bytes(_) | Value::ByteBuffer(_))),
        record_name if record_name.starts_with("auth/") => {
            validate_record(record_name, value)?;
            Ok(true)
        }
        _ => Err(hoplite_auth_store_abi::Error::new(
            "type-unknown",
            field_type,
        )),
    }
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
            keyword("abi/types"),
            map(hoplite_auth_store_abi::RECORDS
                .iter()
                .map(|record_type| {
                    (
                        keyword(record_type.name),
                        record(
                            record_type
                                .fields
                                .iter()
                                .map(|field| (field.name, field.field_type, field.required))
                                .collect(),
                        ),
                    )
                })
                .collect()),
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

    #[test]
    fn contract_contains_every_portable_record() {
        let types = super::map_entries(&value(), "abi/types").unwrap();
        assert_eq!(types.len(), hoplite_auth_store_abi::RECORDS.len());
        for record in hoplite_auth_store_abi::RECORDS {
            assert!(types.iter().any(
                |(name, _)| matches!(name, Value::Keyword(name) if name.as_str() == record.name)
            ));
        }
    }

    #[test]
    fn request_payloads_are_checked_against_the_operation_input() {
        let payload = map(vec![
            (keyword("user/id"), Value::String("usr_1".into())),
            (keyword("user/realm"), Value::String("management".into())),
            (keyword("user/created-at"), Value::Number(42)),
        ]);
        let request = hoplite_auth_store_abi::Request::new(
            "req-1",
            "auth/user-create",
            hara_wasm::hta::encode(&payload).unwrap(),
        )
        .unwrap();
        assert_eq!(
            decode_request_payload(&request).unwrap().display(),
            payload.display()
        );

        let incomplete = map(vec![(keyword("user/id"), Value::String("usr_1".into()))]);
        let request = hoplite_auth_store_abi::Request::new(
            "req-2",
            "auth/user-create",
            hara_wasm::hta::encode(&incomplete).unwrap(),
        )
        .unwrap();
        assert_eq!(
            decode_request_payload(&request).unwrap_err().code,
            "field-required"
        );
    }

    #[test]
    fn response_results_are_checked_against_the_operation_output() {
        let request = hoplite_auth_store_abi::Request::new(
            "req-1",
            "auth/user-find",
            hara_wasm::hta::encode(&map(vec![
                (keyword("user/realm"), Value::String("management".into())),
                (keyword("device/public-key"), Value::Bytes(vec![1; 32])),
            ]))
            .unwrap(),
        )
        .unwrap();
        let result = map(vec![(
            keyword("result/value"),
            map(vec![
                (keyword("user/id"), Value::String("usr_1".into())),
                (keyword("user/realm"), Value::String("management".into())),
                (keyword("user/created-at"), Value::Number(42)),
            ]),
        )]);
        let response = hoplite_auth_store_abi::Response::success(
            "req-1",
            hara_wasm::hta::encode(&result).unwrap(),
        )
        .unwrap();
        assert_eq!(
            decode_response_result(&request, &response)
                .unwrap()
                .unwrap()
                .display(),
            result.display()
        );
    }

    struct EchoAdapter {
        result: Vec<u8>,
        called: bool,
    }

    impl hoplite_auth_store_abi::Adapter for EchoAdapter {
        fn execute(
            &mut self,
            request: hoplite_auth_store_abi::Request,
        ) -> Result<hoplite_auth_store_abi::Response, hoplite_auth_store_abi::Error> {
            self.called = true;
            hoplite_auth_store_abi::Response::success(request.id, self.result.clone())
        }

        fn transact(
            &mut self,
            transaction: hoplite_auth_store_abi::Transaction,
        ) -> Result<hoplite_auth_store_abi::TransactionResponse, hoplite_auth_store_abi::Error>
        {
            self.called = true;
            let responses = transaction
                .operations
                .iter()
                .map(|request| {
                    hoplite_auth_store_abi::Response::success(
                        request.id.clone(),
                        self.result.clone(),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            hoplite_auth_store_abi::TransactionResponse::new(&transaction, responses)
        }
    }

    #[test]
    fn guarded_dispatch_validates_both_sides_of_an_adapter_call() {
        let request = hoplite_auth_store_abi::Request::new(
            "req-1",
            "auth/user-create",
            hara_wasm::hta::encode(&map(vec![
                (keyword("user/id"), Value::String("usr_1".into())),
                (keyword("user/realm"), Value::String("management".into())),
                (keyword("user/created-at"), Value::Number(42)),
            ]))
            .unwrap(),
        )
        .unwrap();
        let mut adapter = EchoAdapter {
            result: request.payload_hta.clone(),
            called: false,
        };
        assert_eq!(execute(&mut adapter, request).unwrap().id, "req-1");
        assert!(adapter.called);

        let invalid = hoplite_auth_store_abi::Request::new(
            "req-2",
            "auth/user-create",
            hara_wasm::hta::encode(&map(vec![])).unwrap(),
        )
        .unwrap();
        adapter.called = false;
        assert_eq!(
            execute(&mut adapter, invalid).unwrap_err().code,
            "field-required"
        );
        assert!(!adapter.called);

        let mutation = hoplite_auth_store_abi::Request::new(
            "req-3",
            "auth/user-create",
            adapter.result.clone(),
        )
        .unwrap();
        let transaction =
            hoplite_auth_store_abi::Transaction::new("txn-1", vec![mutation]).unwrap();
        assert_eq!(
            transact(&mut adapter, transaction).unwrap().responses.len(),
            1
        );
    }
}

#[cfg(test)]
fn map_entries(value: &Value, field_name: &str) -> Option<Vec<(Value, Value)>> {
    hara_wasm::core::map_entries(value)?
        .into_iter()
        .find_map(|(key, value)| {
            matches!(key, Value::Keyword(name) if name.as_str() == field_name)
                .then(|| hara_wasm::core::map_entries(&value))
                .flatten()
        })
}
