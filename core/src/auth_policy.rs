use hara_wasm::core::{self, Value};
use hara_wasm::Runtime;

pub struct AuthPolicy {
    runtime: Runtime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChallengeDecision {
    Accept,
    Used,
    Expired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshDecision {
    Accept,
    Retry,
    Reuse,
    Revoked,
    Expired,
}

impl AuthPolicy {
    pub fn new(composition: &crate::platform::AuthComposition) -> Result<Self, String> {
        if composition.policy_export != crate::platform::CORE_AUTH_EXPORT {
            return Err(format!(
                "authentication policy {} exports unsupported :{}",
                composition.policy_package, composition.policy_export
            ));
        }
        let mut runtime = Runtime::new();
        runtime.register_resource("hoplite.core", super::app::CORE_SOURCE);
        if !composition.explicit {
            runtime.register_resource("hoplite.auth", super::app::AUTH_SOURCE);
        } else {
            let root = crate::package::installed_root_locked(
                &composition.policy_package,
                &composition.policy_version,
                composition.policy_archive_sha256.as_deref(),
            )?;
            let project = hara_wasm::project::read(&root)?;
            hara_wasm::project::register_sources(&project, &mut runtime)?;
        }
        let namespace = composition.policy_export.replace('/', ".");
        runtime.eval_native_value(&format!(
            "(ns hoplite.auth.engine (:require [{namespace} :as auth])) :ready"
        ))?;
        Ok(Self { runtime })
    }

    pub fn consume_challenge(
        &mut self,
        id: &str,
        expires_at: i64,
        used_at: Option<i64>,
        now: i64,
    ) -> Result<ChallengeDecision, String> {
        let source = format!(
            "(auth/consume-challenge {{:challenge/id {} :challenge/expires-at {expires_at} :challenge/used-at {}}} {now})",
            literal(id),
            optional_number(used_at)
        );
        let value = self.runtime.eval_native_value(&source)?;
        match keyword_field(&value, "auth/result").as_deref() {
            Some("accepted") => Ok(ChallengeDecision::Accept),
            Some("rejected") => match keyword_field(&value, "auth/reason").as_deref() {
                Some("challenge-used") => Ok(ChallengeDecision::Used),
                Some("challenge-expired") => Ok(ChallengeDecision::Expired),
                reason => Err(format!("unsupported HAL challenge rejection {reason:?}")),
            },
            result => Err(format!("unsupported HAL challenge result {result:?}")),
        }
    }

    pub fn rotate_refresh(
        &mut self,
        session_id: &str,
        refresh_hash: &[u8],
        used_at: Option<i64>,
        refresh_expires_at: i64,
        revoked_at: Option<i64>,
        now: i64,
        reuse_interval_seconds: u32,
    ) -> Result<RefreshDecision, String> {
        let source = format!(
            "(auth/rotate-refresh
              {{:refresh/hash {} :refresh/used-at {}}}
              {{:session/id {} :session/refresh-expires-at {refresh_expires_at} :session/revoked-at {}}}
              {{:refresh/hash \"replacement\" :refresh/access-hash \"replacement\" :refresh/access-expires-at {now} :refresh/expires-at {refresh_expires_at}}}
              {now} {reuse_interval_seconds})",
            literal(&encode_hex(refresh_hash)),
            optional_number(used_at),
            literal(session_id),
            optional_number(revoked_at)
        );
        let value = self.runtime.eval_native_value(&source)?;
        match (
            keyword_field(&value, "auth/result").as_deref(),
            keyword_field(&value, "auth/reason").as_deref(),
        ) {
            (Some("accepted"), _) => Ok(RefreshDecision::Accept),
            (Some("retry"), Some("refresh-already-rotated")) => Ok(RefreshDecision::Retry),
            (Some("rejected"), Some("refresh-reuse")) => Ok(RefreshDecision::Reuse),
            (Some("rejected"), Some("session-revoked")) => Ok(RefreshDecision::Revoked),
            (Some("rejected"), Some("refresh-expired")) => Ok(RefreshDecision::Expired),
            result => Err(format!("unsupported HAL refresh result {result:?}")),
        }
    }
}

fn keyword_field(value: &Value, name: &str) -> Option<String> {
    core::map_entries(value)?.iter().find_map(|(key, value)| {
        matches!(key, Value::Keyword(keyword) if keyword.as_str() == name)
            .then(|| match value {
                Value::Keyword(keyword) => Some(keyword.as_str().to_owned()),
                _ => None,
            })
            .flatten()
    })
}

fn literal(value: &str) -> String {
    serde_json::to_string(value).expect("strings always encode as JSON literals")
}

fn optional_number(value: Option<i64>) -> String {
    value.map_or_else(|| "nil".into(), |value| value.to_string())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions_are_evaluated_by_hal() {
        let mut policy = AuthPolicy::new(
            &crate::platform::Config::default()
                .auth_composition()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            policy
                .consume_challenge("challenge", 200, None, 100)
                .unwrap(),
            ChallengeDecision::Accept
        );
        assert_eq!(
            policy
                .consume_challenge("challenge", 90, None, 100)
                .unwrap(),
            ChallengeDecision::Expired
        );
        assert_eq!(
            policy
                .rotate_refresh("session", &[1; 32], Some(80), 200, None, 100, 10)
                .unwrap(),
            RefreshDecision::Reuse
        );
    }
}
