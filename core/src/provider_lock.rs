use serde_json::{Map, Value};
use std::collections::BTreeSet;

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

    pub fn artifact_digest_hex(&self) -> &str {
        self.artifact_digest
            .strip_prefix("sha256:")
            .expect("validated lock digest has SHA-256 prefix")
    }

    pub fn shell_environment(&self) -> String {
        format!(
            "HOPLITE_PROVIDER_NAME={}\n\
HOPLITE_PROVIDER_VERSION={}\n\
HOPLITE_PROVIDER_SOURCE_REVISION={}\n\
HOPLITE_PROVIDER_REPOSITORY={}\n\
HOPLITE_PROVIDER_TAG={}\n\
HOPLITE_PROVIDER_ASSET={}\n\
HOPLITE_PROVIDER_MEDIA_TYPE={}\n\
HOPLITE_PROVIDER_SHA256={}\n",
            self.provider,
            self.version,
            self.source_revision,
            self.repository,
            self.tag,
            self.asset,
            self.media_type,
            self.artifact_digest_hex()
        )
    }
}

pub fn validate(
    source: &[u8],
    expected: Expected<'_>,
    manifest_digest: &str,
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
    validate_digest(manifest_digest, "published provider manifest digest")?;
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
        "provider lock release",
        &["repository", "tag"],
    )?;
    require_exact(
        release,
        "repository",
        expected.repository,
        "provider lock release",
    )?;
    require_exact(release, "tag", expected.tag, "provider lock release")?;
    validate_repository(expected.repository)?;
    validate_release_token("provider release tag", expected.tag)?;

    let artifact = exact_object(
        field(root, "artifact", "provider lock")?,
        "provider lock artifact",
        &["digest", "media_type", "name"],
    )?;
    require_exact(
        artifact,
        "name",
        expected.asset,
        "provider lock artifact",
    )?;
    require_exact(
        artifact,
        "media_type",
        expected.media_type,
        "provider lock artifact",
    )?;
    validate_release_token("provider artifact name", expected.asset)?;
    validate_media_type(expected.media_type)?;
    let artifact_digest = require_text(artifact, "digest", "provider lock artifact")?;
    validate_digest(artifact_digest, "provider lock artifact digest")?;
    if artifact_digest != manifest_digest {
        return Err(format!(
            "provider lock artifact digest does not match the published manifest: lock {artifact_digest:?}, manifest {manifest_digest:?}"
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
        ("expected tag", expected.tag),
        ("expected artifact name", expected.asset),
        ("expected media type", expected.media_type),
    ] {
        validate_text(name, value)?;
    }
    Ok(())
}

fn validate_text(name: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_TEXT_BYTES
        || !value.is_ascii()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(format!("{name} is not a bounded visible ASCII identifier"));
    }
    Ok(())
}

fn validate_revision(value: &str) -> Result<(), String> {
    if value.len() != 40
        || !value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(
            "provider lock source revision must contain exactly 40 lowercase hexadecimal digits"
                .into(),
        );
    }
    Ok(())
}

fn validate_repository(value: &str) -> Result<(), String> {
    let mut components = value.split('/');
    let owner = components.next().unwrap_or_default();
    let repository = components.next().unwrap_or_default();
    if components.next().is_some()
        || !valid_repository_component(owner)
        || !valid_repository_component(repository)
    {
        return Err("provider lock repository must be one closed owner/name identity".into());
    }
    Ok(())
}

fn valid_repository_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}

fn validate_release_token(name: &str, value: &str) -> Result<(), String> {
    if value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
    {
        return Err(format!("{name} contains unsupported bytes"));
    }
    Ok(())
}

fn validate_media_type(value: &str) -> Result<(), String> {
    let Some((type_name, subtype)) = value.split_once('/') else {
        return Err("provider artifact media type must contain one slash".into());
    };
    if type_name.is_empty()
        || subtype.is_empty()
        || subtype.contains('/')
        || !value.bytes().all(|byte| {
            matches!(
                byte,
                b'A'..=b'Z'
                    | b'a'..=b'z'
                    | b'0'..=b'9'
                    | b'!'
                    | b'#'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'*'
                    | b'+'
                    | b'-'
                    | b'.'
                    | b'/'
                    | b'^'
                    | b'_'
                    | b'`'
                    | b'|'
                    | b'~'
            )
        })
    {
        return Err("provider artifact media type is invalid".into());
    }
    Ok(())
}

fn validate_digest(value: &str, context: &str) -> Result<(), String> {
    let digest = value
        .strip_prefix("sha256:")
        .ok_or_else(|| format!("{context} must start with sha256:"))?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(format!(
            "{context} must contain exactly 64 lowercase hexadecimal digits"
        ));
    }
    Ok(())
}

fn field<'a>(
    object: &'a Map<String, Value>,
    name: &str,
    context: &str,
) -> Result<&'a Value, String> {
    object
        .get(name)
        .ok_or_else(|| format!("{context} is missing field {name:?}"))
}

fn exact_object<'a>(
    value: &'a Value,
    context: &str,
    expected_fields: &[&str],
) -> Result<&'a Map<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{context} must be a JSON object"))?;
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected_fields.iter().copied().collect::<BTreeSet<_>>();
    if actual != expected {
        let missing = expected
            .difference(&actual)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        let unknown = actual
            .difference(&expected)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "{context} fields do not match the closed contract; missing [{missing}], unknown [{unknown}]"
        ));
    }
    Ok(object)
}

fn require_text<'a>(
    object: &'a Map<String, Value>,
    name: &str,
    context: &str,
) -> Result<&'a str, String> {
    let value = field(object, name, context)?
        .as_str()
        .ok_or_else(|| format!("{context} field {name:?} must be a string"))?;
    validate_text(&format!("{context} field {name:?}"), value)?;
    Ok(value)
}

fn require_exact(
    object: &Map<String, Value>,
    name: &str,
    expected: &str,
    context: &str,
) -> Result<(), String> {
    let actual = require_text(object, name, context)?;
    if actual != expected {
        return Err(format!(
            "{context} field {name:?} is incompatible: expected {expected:?}, got {actual:?}"
        ));
    }
    Ok(())
}

struct JsonScanner<'a> {
    source: &'a [u8],
    cursor: usize,
}

impl<'a> JsonScanner<'a> {
    fn new(source: &'a [u8]) -> Self {
        Self { source, cursor: 0 }
    }

    fn validate(mut self) -> Result<(), String> {
        self.skip_whitespace();
        self.value(0)?;
        self.skip_whitespace();
        if self.cursor != self.source.len() {
            return Err(format!(
                "provider lock has trailing JSON bytes at {}",
                self.cursor
            ));
        }
        Ok(())
    }

    fn value(&mut self, depth: usize) -> Result<(), String> {
        if depth > MAX_JSON_DEPTH {
            return Err(format!(
                "provider lock JSON nesting exceeds {}",
                MAX_JSON_DEPTH
            ));
        }
        self.skip_whitespace();
        match self.peek()? {
            b'{' => self.object(depth + 1),
            b'[' => self.array(depth + 1),
            b'"' => {
                self.string_token()?;
                Ok(())
            }
            b't' => self.literal(b"true"),
            b'f' => self.literal(b"false"),
            b'n' => self.literal(b"null"),
            b'-' | b'0'..=b'9' => self.number(),
            byte => Err(format!(
                "invalid provider lock JSON byte {byte:?} at {}",
                self.cursor
            )),
        }
    }

    fn object(&mut self, depth: usize) -> Result<(), String> {
        self.expect(b'{')?;
        self.skip_whitespace();
        if self.consume(b'}') {
            return Ok(());
        }
        let mut keys = BTreeSet::new();
        loop {
            self.skip_whitespace();
            let raw = self.string_token()?;
            let key: String = serde_json::from_slice(raw)
                .map_err(|error| format!("invalid provider lock object key: {error}"))?;
            if !keys.insert(key.clone()) {
                return Err(format!("provider lock contains duplicate key {key:?}"));
            }
            self.skip_whitespace();
            self.expect(b':')?;
            self.value(depth)?;
            self.skip_whitespace();
            if self.consume(b'}') {
                return Ok(());
            }
            self.expect(b',')?;
        }
    }

    fn array(&mut self, depth: usize) -> Result<(), String> {
        self.expect(b'[')?;
        self.skip_whitespace();
        if self.consume(b']') {
            return Ok(());
        }
        loop {
            self.value(depth)?;
            self.skip_whitespace();
            if self.consume(b']') {
                return Ok(());
            }
            self.expect(b',')?;
        }
    }

    fn string_token(&mut self) -> Result<&'a [u8], String> {
        let start = self.cursor;
        self.expect(b'"')?;
        while self.cursor < self.source.len() {
            match self.source[self.cursor] {
                b'"' => {
                    self.cursor += 1;
                    return Ok(&self.source[start..self.cursor]);
                }
                b'\\' => {
                    self.cursor += 1;
                    let escape = self.peek()?;
                    match escape {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {
                            self.cursor += 1;
                        }
                        b'u' => {
                            self.cursor += 1;
                            for _ in 0..4 {
                                let byte = self.peek()?;
                                if !byte.is_ascii_hexdigit() {
                                    return Err(format!(
                                        "invalid provider lock Unicode escape at {}",
                                        self.cursor
                                    ));
                                }
                                self.cursor += 1;
                            }
                        }
                        _ => {
                            return Err(format!(
                                "invalid provider lock string escape at {}",
                                self.cursor
                            ))
                        }
                    }
                }
                byte if byte < 0x20 => {
                    return Err(format!(
                        "provider lock string contains a control byte at {}",
                        self.cursor
                    ))
                }
                _ => self.cursor += 1,
            }
        }
        Err("unterminated provider lock JSON string".into())
    }

    fn number(&mut self) -> Result<(), String> {
        let start = self.cursor;
        while self.cursor < self.source.len()
            && !matches!(
                self.source[self.cursor],
                b' ' | b'\n' | b'\r' | b'\t' | b',' | b']' | b'}'
            )
        {
            self.cursor += 1;
        }
        let token = &self.source[start..self.cursor];
        let value: Value = serde_json::from_slice(token)
            .map_err(|error| format!("invalid provider lock number: {error}"))?;
        if !value.is_number() {
            return Err("invalid provider lock number".into());
        }
        Ok(())
    }

    fn literal(&mut self, literal: &[u8]) -> Result<(), String> {
        let end = self.cursor.saturating_add(literal.len());
        if self.source.get(self.cursor..end) != Some(literal) {
            return Err(format!(
                "invalid provider lock JSON literal at {}",
                self.cursor
            ));
        }
        self.cursor = end;
        Ok(())
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        let actual = self.peek()?;
        if actual != expected {
            return Err(format!(
                "expected provider lock JSON byte {expected:?} at {}, got {actual:?}",
                self.cursor
            ));
        }
        self.cursor += 1;
        Ok(())
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.source.get(self.cursor) == Some(&expected) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Result<u8, String> {
        self.source
            .get(self.cursor)
            .copied()
            .ok_or_else(|| "unexpected end of provider lock JSON".into())
    }

    fn skip_whitespace(&mut self) {
        while self
            .source
            .get(self.cursor)
            .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.cursor += 1;
        }
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
    fn accepts_an_exact_content_addressed_lock() {
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
        assert_eq!(lock.artifact_digest_hex(), &DIGEST[7..]);
        assert!(lock
            .shell_environment()
            .contains("HOPLITE_PROVIDER_SHA256=aaaaaaaa"));
    }

    #[test]
    fn closed_objects_reject_missing_unknown_and_duplicate_fields() {
        let missing = mutate(|value| {
            value.as_object_mut().unwrap().remove("release");
        });
        assert!(validate(&missing, expected(), DIGEST)
            .unwrap_err()
            .contains("missing [release]"));

        let unknown = mutate(|value| {
            value["artifact"].as_object_mut().unwrap().insert(
                "url".into(),
                Value::String("https://example.invalid/provider".into()),
            );
        });
        assert!(validate(&unknown, expected(), DIGEST)
            .unwrap_err()
            .contains("unknown [url]"));

        let duplicate = LOCK.replace(
            "\"provider\": \"hoplite.blob\",",
            "\"provider\": \"hoplite.blob\", \"provider\": \"other\",",
        );
        assert!(validate(duplicate.as_bytes(), expected(), DIGEST)
            .unwrap_err()
            .contains("duplicate key \"provider\""));
    }

    #[test]
    fn compatibility_and_manifest_digest_mismatches_fail_closed() {
        for source in [
            mutate(|value| value["provider"] = Value::String("hoplite.store".into())),
            mutate(|value| value["version"] = Value::String("0.2.0".into())),
            mutate(|value| {
                value["release"]["repository"] = Value::String("other/repo".into())
            }),
            mutate(|value| {
                value["release"]["tag"] = Value::String("other-tag".into())
            }),
            mutate(|value| {
                value["artifact"]["name"] = Value::String("other.tar.gz".into())
            }),
            mutate(|value| {
                value["artifact"]["media_type"] = Value::String("application/zip".into())
            }),
        ] {
            assert!(validate(&source, expected(), DIGEST)
                .unwrap_err()
                .contains("incompatible"));
        }

        assert!(validate(
            LOCK.as_bytes(),
            expected(),
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        )
        .unwrap_err()
        .contains("does not match"));
    }

    #[test]
    fn revisions_and_release_identities_are_bounded() {
        let bad_revision = mutate(|value| {
            value["source_revision"] = Value::String("ABC".into())
        });
        assert!(validate(&bad_revision, expected(), DIGEST)
            .unwrap_err()
            .contains("40 lowercase"));

        for repository in ["owner", "owner/repo/extra", "../repo", "owner/.."] {
            let mut changed = expected();
            changed.repository = repository;
            assert!(validate(LOCK.as_bytes(), changed, DIGEST).is_err());
        }

        for tag in ["../tag", "tag/name", "tag name", "tag=value"] {
            let mut changed = expected();
            changed.tag = tag;
            assert!(validate(LOCK.as_bytes(), changed, DIGEST).is_err());
        }
    }

    #[test]
    fn lock_size_and_json_depth_are_bounded() {
        let oversized = vec![b' '; MAX_LOCK_BYTES + 1];
        assert!(validate(&oversized, expected(), DIGEST)
            .unwrap_err()
            .contains("with limit"));

        let nested = format!(
            "{}null{}",
            "[".repeat(MAX_JSON_DEPTH + 1),
            "]".repeat(MAX_JSON_DEPTH + 1)
        );
        assert!(validate(nested.as_bytes(), expected(), DIGEST)
            .unwrap_err()
            .contains("nesting exceeds"));
    }
}
