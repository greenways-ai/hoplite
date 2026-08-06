//! Dependency-free model for the `hoplite-auth-store/1` HTA ABI.

use std::collections::BTreeMap;

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
    pub const fn name(self) -> &'static str {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: &'static str,
    pub field_type: &'static str,
    pub required: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub name: &'static str,
    pub fields: &'static [Field],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    String(String),
    Integer(i64),
    Bytes(Vec<u8>),
    Record(RecordValue),
}

pub type RecordValue = BTreeMap<String, Value>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeRequest {
    pub id: String,
    pub operation: Operation,
    pub payload: RecordValue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeTransaction {
    pub id: String,
    pub operations: Vec<NativeRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeResponse {
    pub id: String,
    pub result: Option<RecordValue>,
    pub error: Option<Error>,
}

impl NativeResponse {
    pub fn success(id: impl Into<String>, result: RecordValue) -> Result<Self, Error> {
        let id = id.into();
        validate_id(&id, "response")?;
        Ok(Self {
            id,
            result: Some(result),
            error: None,
        })
    }

    pub fn failure(id: impl Into<String>, error: Error) -> Result<Self, Error> {
        let id = id.into();
        validate_id(&id, "response")?;
        Ok(Self {
            id,
            result: None,
            error: Some(error),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeTransactionResponse {
    pub id: String,
    pub responses: Vec<NativeResponse>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: String,
    pub operation: Operation,
    pub payload_hta: Vec<u8>,
}

impl Request {
    pub fn new(
        id: impl Into<String>,
        operation_name: &str,
        payload_hta: Vec<u8>,
    ) -> Result<Self, Error> {
        let id = id.into();
        validate_id(&id, "request")?;
        let operation = operation(operation_name)
            .copied()
            .ok_or_else(|| Error::new("operation-unknown", operation_name))?;
        validate_hta(&payload_hta)?;
        Ok(Self {
            id,
            operation,
            payload_hta,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    pub id: String,
    pub operations: Vec<Request>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub id: String,
    pub result_hta: Option<Vec<u8>>,
    pub error: Option<Error>,
}

impl Response {
    pub fn success(id: impl Into<String>, result_hta: Vec<u8>) -> Result<Self, Error> {
        let id = id.into();
        validate_id(&id, "response")?;
        validate_hta(&result_hta)?;
        Ok(Self {
            id,
            result_hta: Some(result_hta),
            error: None,
        })
    }

    pub fn failure(id: impl Into<String>, error: Error) -> Result<Self, Error> {
        let id = id.into();
        validate_id(&id, "response")?;
        Ok(Self {
            id,
            result_hta: None,
            error: Some(error),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransactionResponse {
    pub id: String,
    pub responses: Vec<Response>,
}

impl TransactionResponse {
    pub fn new(transaction: &Transaction, responses: Vec<Response>) -> Result<Self, Error> {
        if responses.len() != transaction.operations.len() {
            return Err(Error::new(
                "transaction-response-count",
                format!(
                    "expected {} responses, got {}",
                    transaction.operations.len(),
                    responses.len()
                ),
            ));
        }
        for (request, response) in transaction.operations.iter().zip(&responses) {
            if request.id != response.id {
                return Err(Error::new(
                    "response-id-mismatch",
                    format!("expected {}, got {}", request.id, response.id),
                ));
            }
        }
        Ok(Self {
            id: transaction.id.clone(),
            responses,
        })
    }
}

pub trait Adapter {
    fn execute(&mut self, request: NativeRequest) -> Result<NativeResponse, Error>;

    fn transact(
        &mut self,
        transaction: NativeTransaction,
    ) -> Result<NativeTransactionResponse, Error>;
}

impl Transaction {
    pub fn new(id: impl Into<String>, operations: Vec<Request>) -> Result<Self, Error> {
        let id = id.into();
        validate_id(&id, "transaction")?;
        if operations.is_empty() {
            return Err(Error::new(
                "transaction-empty",
                "at least one operation is required",
            ));
        }
        if let Some(request) = operations
            .iter()
            .find(|request| request.operation.mode != Mode::Transact)
        {
            return Err(Error::new("transaction-query", request.operation.name));
        }
        Ok(Self { id, operations })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub code: String,
    pub detail: String,
}

impl Error {
    pub fn new(code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
        }
    }
}

pub const OPERATIONS: [Operation; 9] = [
    op(
        "auth/audit-append",
        Mode::Transact,
        "auth/AuditAppend",
        "auth/AuditEvent",
    ),
    op(
        "auth/challenge-consume",
        Mode::Transact,
        "auth/ChallengeConsume",
        "auth/Challenge",
    ),
    op(
        "auth/challenge-put",
        Mode::Transact,
        "auth/Challenge",
        "auth/Challenge",
    ),
    op(
        "auth/device-put",
        Mode::Transact,
        "auth/Device",
        "auth/Device",
    ),
    op(
        "auth/refresh-rotate",
        Mode::Transact,
        "auth/RefreshRotate",
        "auth/Session",
    ),
    op(
        "auth/session-put",
        Mode::Transact,
        "auth/Session",
        "auth/Session",
    ),
    op(
        "auth/session-revoke",
        Mode::Transact,
        "auth/SessionRevoke",
        "auth/Session",
    ),
    op("auth/user-create", Mode::Transact, "auth/User", "auth/User"),
    op(
        "auth/user-find",
        Mode::Query,
        "auth/UserFind",
        "auth/UserMaybe",
    ),
];

pub const RECORDS: [Record; 11] = [
    record("auth/AuditAppend", AUDIT_APPEND_FIELDS),
    record("auth/AuditEvent", AUDIT_EVENT_FIELDS),
    record("auth/Challenge", CHALLENGE_FIELDS),
    record("auth/ChallengeConsume", CHALLENGE_CONSUME_FIELDS),
    record("auth/Device", DEVICE_FIELDS),
    record("auth/RefreshRotate", REFRESH_ROTATE_FIELDS),
    record("auth/Session", SESSION_FIELDS),
    record("auth/SessionRevoke", SESSION_REVOKE_FIELDS),
    record("auth/User", USER_FIELDS),
    record("auth/UserFind", USER_FIND_FIELDS),
    record("auth/UserMaybe", USER_MAYBE_FIELDS),
];

const AUDIT_APPEND_FIELDS: &[Field] = &[
    field("audit/occurred-at", "integer", true),
    field("audit/kind", "string", true),
    field("audit/realm", "string", false),
    field("audit/subject-id", "string", false),
    field("audit/detail", "string", true),
];
const AUDIT_EVENT_FIELDS: &[Field] = &[
    field("audit/id", "integer", true),
    field("audit/occurred-at", "integer", true),
    field("audit/kind", "string", true),
    field("audit/realm", "string", false),
    field("audit/subject-id", "string", false),
    field("audit/detail", "string", true),
];
const CHALLENGE_FIELDS: &[Field] = &[
    field("challenge/id", "string", true),
    field("challenge/realm", "string", true),
    field("challenge/public-key", "bytes", true),
    field("challenge/nonce", "bytes", true),
    field("challenge/expires-at", "integer", true),
    field("challenge/used-at", "integer", false),
];
const CHALLENGE_CONSUME_FIELDS: &[Field] = &[
    field("challenge/id", "string", true),
    field("challenge/used-at", "integer", true),
];
const DEVICE_FIELDS: &[Field] = &[
    field("device/id", "string", true),
    field("device/user-id", "string", true),
    field("device/public-key", "bytes", true),
    field("device/revoked-at", "integer", false),
];
const REFRESH_ROTATE_FIELDS: &[Field] = &[
    field("refresh/session-id", "string", true),
    field("refresh/token-hash", "bytes", true),
    field("refresh/replacement-hash", "bytes", true),
    field("refresh/issued-at", "integer", true),
    field("refresh/access-hash", "bytes", true),
    field("refresh/access-expires-at", "integer", true),
    field("refresh/expires-at", "integer", true),
];
const SESSION_FIELDS: &[Field] = &[
    field("session/id", "string", true),
    field("session/user-id", "string", true),
    field("session/device-id", "string", true),
    field("session/realm", "string", true),
    field("session/access-hash", "bytes", true),
    field("session/access-expires-at", "integer", true),
    field("session/refresh-expires-at", "integer", true),
    field("session/revoked-at", "integer", false),
];
const SESSION_REVOKE_FIELDS: &[Field] = &[
    field("session/id", "string", true),
    field("session/revoked-at", "integer", true),
];
const USER_FIELDS: &[Field] = &[
    field("user/id", "string", true),
    field("user/realm", "string", true),
    field("user/created-at", "integer", true),
];
const USER_FIND_FIELDS: &[Field] = &[
    field("user/realm", "string", true),
    field("device/public-key", "bytes", true),
];
const USER_MAYBE_FIELDS: &[Field] = &[field("result/value", "auth/User", false)];

const fn field(name: &'static str, field_type: &'static str, required: bool) -> Field {
    Field {
        name,
        field_type,
        required,
    }
}

const fn record(name: &'static str, fields: &'static [Field]) -> Record {
    Record { name, fields }
}

const fn op(
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

pub fn operation(name: &str) -> Option<&'static Operation> {
    OPERATIONS.iter().find(|operation| operation.name == name)
}

pub fn record_type(name: &str) -> Option<&'static Record> {
    RECORDS.iter().find(|record| record.name == name)
}

fn validate_id(id: &str, label: &'static str) -> Result<(), Error> {
    if id.is_empty() || id.len() > 128 || id.chars().any(char::is_whitespace) {
        Err(Error::new(
            "identifier-invalid",
            format!("{label} id must be 1-128 non-whitespace characters"),
        ))
    } else {
        Ok(())
    }
}

fn validate_hta(payload: &[u8]) -> Result<(), Error> {
    if payload.starts_with(b"HTA1") {
        Ok(())
    } else {
        Err(Error::new(
            "payload-not-hta",
            "payload must be an HTA1 frame",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn operations_are_unique_and_only_reads_are_queries() {
        assert_eq!(
            OPERATIONS
                .iter()
                .map(|operation| operation.name)
                .collect::<BTreeSet<_>>()
                .len(),
            OPERATIONS.len()
        );
        assert_eq!(
            OPERATIONS
                .iter()
                .filter(|operation| operation.mode == Mode::Query)
                .map(|operation| operation.name)
                .collect::<Vec<_>>(),
            vec!["auth/user-find"]
        );
        for operation in OPERATIONS {
            assert!(
                record_type(operation.input).is_some(),
                "{}",
                operation.input
            );
            assert!(
                record_type(operation.output).is_some(),
                "{}",
                operation.output
            );
        }
    }

    #[test]
    fn record_and_field_names_are_unique() {
        assert_eq!(
            RECORDS
                .iter()
                .map(|record| record.name)
                .collect::<BTreeSet<_>>()
                .len(),
            RECORDS.len()
        );
        for record in RECORDS {
            assert!(!record.fields.is_empty(), "{}", record.name);
            assert_eq!(
                record
                    .fields
                    .iter()
                    .map(|field| field.name)
                    .collect::<BTreeSet<_>>()
                    .len(),
                record.fields.len(),
                "{}",
                record.name
            );
        }
    }

    #[test]
    fn requests_and_transactions_enforce_the_wire_boundary() {
        let mutation = Request::new("req-1", "auth/user-create", b"HTA1payload".to_vec()).unwrap();
        assert_eq!(
            Transaction::new("txn-1", vec![mutation])
                .unwrap()
                .operations
                .len(),
            1
        );

        let query = Request::new("req-2", "auth/user-find", b"HTA1payload".to_vec()).unwrap();
        assert_eq!(
            Transaction::new("txn-2", vec![query]).unwrap_err().code,
            "transaction-query"
        );
        assert_eq!(
            Request::new("req-3", "auth/user-find", b"json".to_vec())
                .unwrap_err()
                .code,
            "payload-not-hta"
        );
    }

    #[test]
    fn responses_preserve_request_and_transaction_identity() {
        let request = Request::new("req-1", "auth/user-create", b"HTA1input".to_vec()).unwrap();
        let transaction = Transaction::new("txn-1", vec![request]).unwrap();
        let response = Response::success("req-1", b"HTA1output".to_vec()).unwrap();
        assert_eq!(
            TransactionResponse::new(&transaction, vec![response])
                .unwrap()
                .id,
            "txn-1"
        );
        let mismatch = Response::failure("req-2", Error::new("conflict", "duplicate")).unwrap();
        assert_eq!(
            TransactionResponse::new(&transaction, vec![mismatch])
                .unwrap_err()
                .code,
            "response-id-mismatch"
        );
    }
}
