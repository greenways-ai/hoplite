use serde_json::{Map, Value};
use std::collections::BTreeSet;

#[path = "provider_lock/object_backend.rs"]
mod object_backend;
pub use object_backend::{
    validate as validate_object_backend_lock, Expected as ObjectBackendExpected,
    ValidatedLock as ValidatedObjectBackendLock, FORMAT as OBJECT_BACKEND_FORMAT,
};

#[path = "provider_lock/provider_set.rs"]
mod provider_set;
pub use provider_set::{
    validate as validate_provider_set_lock, Expected as ProviderSetExpected,
    ValidatedSet as ValidatedProviderSet, FORMAT as PROVIDER_SET_FORMAT,
};

pub const FORMAT: &str = "hoplite.provider-lock/v1";
const MAX_LOCK_BYTES: usize = 16 * 1024;
const MAX_TEXT_BYTES: usize = 256;
const MAX_JSON_DEPTH: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expected<'a> {
    pub provider: &'a str,
    pub version: &'a str,
    pub repository: &'a str,
    pub tag: &'a str,
    pub asset: &'a str,
    pub media_type: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedLock {
    provider: String,
    version: String,
    source_revision: String,
    repository: String,
    tag: String,
    asset: String,
    media_type: String,
    artifact_digest: String,
}

impl ValidatedLock {
    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn source_revision(&self) -> &str {
        &self.source_revision
    }

    pub fn repository(&self) -> &str {
        &self.repository
    }

    pub fn tag(&self) -> &str {
        &self.tag
    }

    pub fn asset(&self) -> &str {
        &self.asset
    }

    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    pub fn artifact_digest(&self) -> &str {
        &self.artifact_digest
    }
}

pub fn validate(
    source: &[u8],
    expected: Expected<'_>,
    published_manifest_digest: &str,
) -> Result<ValidatedLock, String> {
    if source.is_empty() {
        return Err("provider lock is empty".into());
    }
    if source.len() > MAX_LOCK_BYTES {
        return Err(format!(
            "provider lock is {} bytes with limit {}",
            source.len(),
            MAX_LOCK_BYTES
        ));
    }
    validate_expected(expected)?;
    validate_digest(
        published_manifest_digest,
        "published provider manifest digest",
    )?;
    JsonScanner::new(source).validate()?;

    let value: Value = serde_json::from_slice(source)
        .map_err(|error| format!("invalid provider lock JSON: {error}"))?;
    let root = exact_object(
        &value,
        "provider lock",
        &[
            "artifact",
            "format",
            "provider",
            "release",
            "source_revision",
            "version",
        ],
    )?;
    require_exact(root, "format", FORMAT, "provider lock")?;
    require_exact(root, "provider", expected.provider, "provider lock")?;
    require_exact(root, "version", expected.version, "provider lock")?;

    let source_revision = require_text(root, "source_revision", "provider lock")?;
    validate_revision(source_revision)?;

    let release = exact_object(
        field(root, "release", "provider lock")?,
        "provider release",
        &["repository", "tag"],
    )?;
    require_exact(
        release,
        "repository",
        expected.repository,
        "provider release",
    )?;
    require_exact(release, "tag", expected.tag, "provider release")?;

    let artifact = exact_object(
        field(root, "artifact", "provider lock")?,
        "provider artifact lock",
        &["digest", "media_type", "name"],
    )?;
    require_exact(artifact, "name", expected.asset, "provider artifact lock")?;
    require_exact(
        artifact,
        "media_type",
        expected.media_type,
        "provider artifact lock",
    )?;
    let artifact_digest = require_text(artifact, "digest", "provider artifact lock")?;
    validate_digest(artifact_digest, "provider artifact digest")?;
    if artifact_digest != published_manifest_digest {
        return Err(format!(
            "provider lock digest does not match the published manifest: lock {artifact_digest:?}, manifest {published_manifest_digest:?}"
        ));
    }

    Ok(ValidatedLock {
        provider: expected.provider.to_owned(),
        version: expected.version.to_owned(),
        source_revision: source_revision.to_owned(),
        repository: expected.repository.to_owned(),
        tag: expected.tag.to_owned(),
        asset: expected.asset.to_owned(),
        media_type: expected.media_type.to_owned(),
        artifact_digest: artifact_digest.to_owned(),
    })
}

fn validate_expected(expected: Expected<'_>) -> Result<(), String> {
    for (name, value) in [
        ("expected provider", expected.provider),
        ("expected version", expected.version),
        ("expected repository", expected.repository),
        ("expected release tag", expected.tag),
        ("expected asset", expected.asset),
        ("expected media type", expected.media_type),
    ] {
        validate_text(name, value)?;
    }
    Ok(())
}

fn validate_text(name: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{name} is empty"));
    }
    if value.len() > MAX_TEXT_BYTES {
        return Err(format!("{name} exceeds {MAX_TEXT_BYTES} bytes"));
    }
    if value.bytes().any(|byte| {
        byte.is_ascii_control() || byte.is_ascii_whitespace() || matches!(byte, b'=' | b'\'' | b'"')
    }) {
        return Err(format!("{name} contains forbidden characters"));
    }
    Ok(())
}

fn validate_revision(value: &str) -> Result<(), String> {
    if value.len() != 40
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err("provider lock source_revision must be 40 lowercase hexadecimal digits".into());
    }
    Ok(())
}

fn validate_digest(value: &str, context: &str) -> Result<(), String> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(format!("{context} must use sha256:"));
    };
    if hex.len() != 64
        || hex
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(format!(
            "{context} must contain 64 lowercase hexadecimal digits"
        ));
    }
    Ok(())
}

fn exact_object<'a>(
    value: &'a Value,
    context: &str,
    expected: &[&str],
) -> Result<&'a Map<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{context} must be an object"))?;
    let present = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    if present != expected {
        let missing = expected.difference(&present).copied().collect::<Vec<_>>();
        let unknown = present.difference(&expected).copied().collect::<Vec<_>>();
        return Err(format!(
            "{context} fields are not exact; missing {missing:?}, unknown {unknown:?}"
        ));
    }
    Ok(object)
}

fn field<'a>(
    object: &'a Map<String, Value>,
    name: &str,
    context: &str,
) -> Result<&'a Value, String> {
    object
        .get(name)
        .ok_or_else(|| format!("{context} is missing field {name}"))
}

fn require_text<'a>(
    object: &'a Map<String, Value>,
    name: &str,
    context: &str,
) -> Result<&'a str, String> {
    let value = field(object, name, context)?;
    let text = value
        .as_str()
        .ok_or_else(|| format!("{context} field {name} must be text"))?;
    validate_text(&format!("{context} field {name}"), text)?;
    Ok(text)
}

fn require_exact(
    object: &Map<String, Value>,
    name: &str,
    expected: &str,
    context: &str,
) -> Result<(), String> {
    let value = require_text(object, name, context)?;
    if value != expected {
        return Err(format!(
            "{context} field {name} is incompatible: expected {expected:?}, found {value:?}"
        ));
    }
    Ok(())
}

struct JsonScanner<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> JsonScanner<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn validate(mut self) -> Result<(), String> {
        self.skip_whitespace();
        self.scan_value(0)?;
        self.skip_whitespace();
        if self.cursor != self.bytes.len() {
            return Err("provider lock contains trailing JSON bytes".into());
        }
        Ok(())
    }

    fn scan_value(&mut self, depth: usize) -> Result<(), String> {
        if depth > MAX_JSON_DEPTH {
            return Err("provider lock exceeds the JSON depth limit".into());
        }
        self.skip_whitespace();
        match self.peek() {
            Some(b'{') => self.scan_object(depth + 1),
            Some(b'[') => self.scan_array(depth + 1),
            Some(b'"') => self.scan_string().map(|_| ()),
            Some(b't') => self.scan_literal(b"true"),
            Some(b'f') => self.scan_literal(b"false"),
            Some(b'n') => self.scan_literal(b"null"),
            Some(b'-' | b'0'..=b'9') => self.scan_number(),
            Some(byte) => Err(format!(
                "provider lock contains an unsupported JSON byte 0x{byte:02x}"
            )),
            None => Err("provider lock is truncated".into()),
        }
    }

    fn scan_object(&mut self, depth: usize) -> Result<(), String> {
        self.expect(b'{')?;
        self.skip_whitespace();
        if self.consume(b'}') {
            return Ok(());
        }
        let mut keys = BTreeSet::new();
        loop {
            self.skip_whitespace();
            let key = self.scan_string()?;
            if key.len() > MAX_TEXT_BYTES {
                return Err("provider lock JSON key exceeds its byte limit".into());
            }
            if !keys.insert(key.clone()) {
                return Err(format!("provider lock contains duplicate key {key:?}"));
            }
            self.skip_whitespace();
            self.expect(b':')?;
            self.scan_value(depth)?;
            self.skip_whitespace();
            if self.consume(b'}') {
                return Ok(());
            }
            self.expect(b',')?;
        }
    }

    fn scan_array(&mut self, depth: usize) -> Result<(), String> {
        self.expect(b'[')?;
        self.skip_whitespace();
        if self.consume(b']') {
            return Ok(());
        }
        loop {
            self.scan_value(depth)?;
            self.skip_whitespace();
            if self.consume(b']') {
                return Ok(());
            }
            self.expect(b',')?;
        }
    }

    fn scan_string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut output = String::new();
        loop {
            let byte = self
                .next()
                .ok_or("provider lock contains a truncated JSON string")?;
            match byte {
                b'"' => return Ok(output),
                b'\\' => {
                    let escape = self
                        .next()
                        .ok_or("provider lock contains a truncated JSON escape")?;
                    match escape {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{0008}'),
                        b'f' => output.push('\u{000c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => {
                            return Err(
                                "provider lock does not allow escaped Unicode code points".into()
                            )
                        }
                        _ => return Err("provider lock contains an invalid JSON escape".into()),
                    }
                }
                0x00..=0x1f => {
                    return Err("provider lock JSON string contains a control byte".into())
                }
                0x20..=0x7e => output.push(char::from(byte)),
                _ => return Err("provider lock JSON must be ASCII".into()),
            }
            if output.len() > MAX_TEXT_BYTES {
                return Err("provider lock JSON string exceeds its byte limit".into());
            }
        }
    }

    fn scan_literal(&mut self, literal: &[u8]) -> Result<(), String> {
        for expected in literal {
            let actual = self
                .next()
                .ok_or("provider lock contains a truncated JSON literal")?;
            if actual != *expected {
                return Err("provider lock contains an invalid JSON literal".into());
            }
        }
        Ok(())
    }

    fn scan_number(&mut self) -> Result<(), String> {
        if self.consume(b'-') && self.peek().is_none() {
            return Err("provider lock contains a truncated JSON number".into());
        }
        match self.peek() {
            Some(b'0') => {
                self.cursor += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err("provider lock JSON number has a leading zero".into());
                }
            }
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.cursor += 1;
                }
            }
            _ => return Err("provider lock contains an invalid JSON number".into()),
        }
        if self.consume(b'.') {
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err("provider lock JSON fraction is truncated".into());
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.cursor += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.cursor += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.cursor += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err("provider lock JSON exponent is truncated".into());
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.cursor += 1;
            }
        }
        Ok(())
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.cursor += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        match self.next() {
            Some(actual) if actual == expected => Ok(()),
            Some(actual) => Err(format!(
                "provider lock expected byte 0x{expected:02x}, found 0x{actual:02x}"
            )),
            None => Err(format!(
                "provider lock expected byte 0x{expected:02x}, found end of input"
            )),
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.cursor).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.cursor += 1;
        Some(byte)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const LOCK: &str = r#"{
  "format": "hoplite.provider-lock/v1",
  "provider": "hoplite.blob",
  "version": "0.1.0",
  "source_revision": "0123456789abcdef0123456789abcdef01234567",
  "release": {
    "repository": "greenways-ai/hoplite",
    "tag": "hoplite-blob-provider-v0.1.0"
  },
  "artifact": {
    "name": "hoplite-blob-provider-0.1.0.tar.gz",
    "media_type": "application/gzip",
    "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  }
}"#;

    fn expected() -> Expected<'static> {
        Expected {
            provider: "hoplite.blob",
            version: "0.1.0",
            repository: "greenways-ai/hoplite",
            tag: "hoplite-blob-provider-v0.1.0",
            asset: "hoplite-blob-provider-0.1.0.tar.gz",
            media_type: "application/gzip",
        }
    }

    fn mutate(change: impl FnOnce(&mut Value)) -> Vec<u8> {
        let mut value: Value = serde_json::from_str(LOCK).unwrap();
        change(&mut value);
        serde_json::to_vec(&value).unwrap()
    }

    #[test]
    fn accepts_one_exact_published_lock() {
        let lock = validate(LOCK.as_bytes(), expected(), DIGEST).unwrap();
        assert_eq!(lock.provider(), "hoplite.blob");
        assert_eq!(lock.version(), "0.1.0");
        assert_eq!(
            lock.source_revision(),
            "0123456789abcdef0123456789abcdef01234567"
        );
        assert_eq!(lock.repository(), "greenways-ai/hoplite");
        assert_eq!(lock.tag(), "hoplite-blob-provider-v0.1.0");
        assert_eq!(lock.asset(), "hoplite-blob-provider-0.1.0.tar.gz");
        assert_eq!(lock.media_type(), "application/gzip");
        assert_eq!(lock.artifact_digest(), DIGEST);
    }

    #[test]
    fn rejects_empty_oversized_and_non_ascii_locks() {
        assert_eq!(
            validate(&[], expected(), DIGEST).unwrap_err(),
            "provider lock is empty"
        );
        let oversized = vec![b' '; MAX_LOCK_BYTES + 1];
        assert!(validate(&oversized, expected(), DIGEST)
            .unwrap_err()
            .contains("with limit"));
        let non_ascii = LOCK.replace("hoplite.blob", "hoplite.blób");
        assert!(validate(non_ascii.as_bytes(), expected(), DIGEST)
            .unwrap_err()
            .contains("must be ASCII"));
    }

    #[test]
    fn rejects_duplicate_unknown_and_missing_fields() {
        let duplicate = LOCK.replace(
            "\"provider\": \"hoplite.blob\",",
            "\"provider\": \"hoplite.blob\", \"provider\": \"other\",",
        );
        assert!(validate(duplicate.as_bytes(), expected(), DIGEST)
            .unwrap_err()
            .contains("duplicate key \"provider\""));

        let unknown = mutate(|value| {
            value
                .as_object_mut()
                .unwrap()
                .insert("url".into(), Value::String("https://example.invalid".into()));
        });
        assert!(validate(&unknown, expected(), DIGEST)
            .unwrap_err()
            .contains("unknown [\"url\"]"));

        let missing = mutate(|value| {
            value.as_object_mut().unwrap().remove("artifact");
        });
        assert!(validate(&missing, expected(), DIGEST)
            .unwrap_err()
            .contains("missing [\"artifact\"]"));
    }

    #[test]
    fn rejects_incompatible_identity_and_manifest_digest() {
        let version = mutate(|value| value["version"] = Value::String("0.2.0".into()));
        assert!(validate(&version, expected(), DIGEST)
            .unwrap_err()
            .contains("incompatible"));

        let tag = mutate(|value| {
            value["release"]["tag"] = Value::String("other".into());
        });
        assert!(validate(&tag, expected(), DIGEST)
            .unwrap_err()
            .contains("incompatible"));

        let digest = mutate(|value| {
            value["artifact"]["digest"] = Value::String(
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    .into(),
            );
        });
        assert!(validate(&digest, expected(), DIGEST)
            .unwrap_err()
            .contains("does not match"));
    }

    #[test]
    fn rejects_invalid_revision_and_artifact_digest() {
        let revision = mutate(|value| {
            value["source_revision"] = Value::String("abc".into());
        });
        assert!(validate(&revision, expected(), DIGEST)
            .unwrap_err()
            .contains("40 lowercase hexadecimal"));

        assert!(validate(LOCK.as_bytes(), expected(), "sha256:ABC")
            .unwrap_err()
            .contains("64 lowercase hexadecimal"));
    }
}
