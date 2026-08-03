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
}
