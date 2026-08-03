//! Dependency-free model for the `hoplite-auth-store/1` HTA ABI.

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
    pub code: &'static str,
    pub detail: String,
}

impl Error {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
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
}
