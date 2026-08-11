use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub const FORMAT: &str = "hoplite.provider-manifest/0-alpha";
const MAX_MANIFEST_BYTES: usize = 16 * 1024;
const MAX_TEXT_BYTES: usize = 256;
const MAX_JSON_DEPTH: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactPolicy {
    Optional,
    Required,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expected<'a> {
    pub provider: &'a str,
    pub request: &'a str,
    pub result: &'a str,
    pub abi_name: &'a str,
    pub abi_version: &'a str,
    pub driver_name: &'a str,
    pub driver_version: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedManifest {
    artifact_digest: Option<String>,
}

impl ValidatedManifest {
    pub fn artifact_digest(&self) -> Option<&str> {
        self.artifact_digest.as_deref()
    }
}

pub fn validate(
    source: &[u8],
    expected: Expected<'_>,
    artifact_policy: ArtifactPolicy,
) -> Result<ValidatedManifest, String> {
    if source.is_empty() {
        return Err("provider manifest is empty".into());
    }
    if source.len() > MAX_MANIFEST_BYTES {
        return Err(format!(
            "provider manifest is {} bytes with limit {}",
            source.len(),
            MAX_MANIFEST_BYTES
        ));
    }
    validate_expected(expected)?;
    JsonScanner::new(source).validate()?;

    let value: Value = serde_json::from_slice(source)
        .map_err(|error| format!("invalid provider manifest JSON: {error}"))?;
    let root = exact_object(
        &value,
        "provider manifest",
        &[
            "abi",
            "artifact",
            "contract",
            "distribution",
            "driver",
            "format",
            "provider",
        ],
    )?;

    require_exact(root, "format", FORMAT, "provider manifest")?;
    require_exact(root, "provider", expected.provider, "provider manifest")?;

    let contract = exact_object(
        field(root, "contract", "provider manifest")?,
        "provider contract",
        &["request", "result"],
    )?;
    require_exact(contract, "request", expected.request, "provider contract")?;
    require_exact(contract, "result", expected.result, "provider contract")?;

    let abi = exact_object(
        field(root, "abi", "provider manifest")?,
        "provider ABI",
        &["name", "version"],
    )?;
    require_exact(abi, "name", expected.abi_name, "provider ABI")?;
    require_exact(abi, "version", expected.abi_version, "provider ABI")?;

    let driver = exact_object(
        field(root, "driver", "provider manifest")?,
        "provider driver",
        &["name", "version"],
    )?;
    require_exact(driver, "name", expected.driver_name, "provider driver")?;
    require_exact(
        driver,
        "version",
        expected.driver_version,
        "provider driver",
    )?;

    let distribution = exact_object(
        field(root, "distribution", "provider manifest")?,
        "provider distribution",
        &["request_selectable", "selection"],
    )?;
    require_exact(
        distribution,
        "selection",
        "static-build",
        "provider distribution",
    )?;
    match field(distribution, "request_selectable", "provider distribution")? {
        Value::Bool(false) => {}
        Value::Bool(true) => {
            return Err("provider distribution must not be request-selectable".into())
        }
        _ => {
            return Err(
                "provider distribution field request_selectable must be boolean false".into(),
            )
        }
    }

    let artifact = exact_object(
        field(root, "artifact", "provider manifest")?,
        "provider artifact",
        &["digest"],
    )?;
    let artifact_digest = match field(artifact, "digest", "provider artifact")? {
        Value::Null if artifact_policy == ArtifactPolicy::Optional => None,
        Value::Null => return Err("published provider manifest requires an artifact digest".into()),
        Value::String(value) => {
            validate_text("provider artifact digest", value)?;
            validate_digest(value)?;
            Some(value.clone())
        }
        _ => return Err("provider artifact digest must be null or a SHA-256 string".into()),
    };

    Ok(ValidatedManifest { artifact_digest })
}

fn validate_expected(expected: Expected<'_>) -> Result<(), String> {
    for (name, value) in [
        ("expected provider", expected.provider),
        ("expected request contract", expected.request),
        ("expected result contract", expected.result),
        ("expected ABI name", expected.abi_name),
        ("expected ABI version", expected.abi_version),
        ("expected driver name", expected.driver_name),
        ("expected driver version", expected.driver_version),
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

fn require_exact(
    object: &Map<String, Value>,
    name: &str,
    expected: &str,
    context: &str,
) -> Result<(), String> {
    let actual = field(object, name, context)?
        .as_str()
        .ok_or_else(|| format!("{context} field {name:?} must be a string"))?;
    validate_text(&format!("{context} field {name:?}"), actual)?;
    if actual != expected {
        return Err(format!(
            "{context} field {name:?} is incompatible: expected {expected:?}, got {actual:?}"
        ));
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<(), String> {
    let digest = value
        .strip_prefix("sha256:")
        .ok_or("provider artifact digest must start with sha256:")?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(
            "provider artifact digest must contain exactly 64 lowercase hexadecimal digits".into(),
        );
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
                "provider manifest has trailing JSON bytes at {}",
                self.cursor
            ));
        }
        Ok(())
    }

    fn value(&mut self, depth: usize) -> Result<(), String> {
        if depth > MAX_JSON_DEPTH {
            return Err(format!(
                "provider manifest JSON nesting exceeds {}",
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
                "invalid provider manifest JSON byte {byte:?} at {}",
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
                .map_err(|error| format!("invalid provider manifest object key: {error}"))?;
            if !keys.insert(key.clone()) {
                return Err(format!("provider manifest contains duplicate key {key:?}"));
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
                                        "invalid provider manifest Unicode escape at {}",
                                        self.cursor
                                    ));
                                }
                                self.cursor += 1;
                            }
                        }
                        _ => {
                            return Err(format!(
                                "invalid provider manifest string escape at {}",
                                self.cursor
                            ))
                        }
                    }
                }
                byte if byte < 0x20 => {
                    return Err(format!(
                        "provider manifest string contains a control byte at {}",
                        self.cursor
                    ))
                }
                _ => self.cursor += 1,
            }
        }
        Err("unterminated provider manifest JSON string".into())
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
            .map_err(|error| format!("invalid provider manifest number: {error}"))?;
        if !value.is_number() {
            return Err("invalid provider manifest number".into());
        }
        Ok(())
    }

    fn literal(&mut self, literal: &[u8]) -> Result<(), String> {
        let end = self.cursor.saturating_add(literal.len());
        if self.source.get(self.cursor..end) != Some(literal) {
            return Err(format!(
                "invalid provider manifest JSON literal at {}",
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
                "expected provider manifest JSON byte {expected:?} at {}, got {actual:?}",
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
            .ok_or_else(|| "unexpected end of provider manifest JSON".into())
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

    const SOURCE_MANIFEST: &str = r#"{
  "format": "hoplite.provider-manifest/0-alpha",
  "provider": "hoplite.blob",
  "contract": {
    "request": "hoplite.blob-request/0-alpha",
    "result": "hoplite.blob-result/0-alpha"
  },
  "abi": {
    "name": "hoplite.blob-provider-ffi",
    "version": "1"
  },
  "driver": {
    "name": "filesystem",
    "version": "1"
  },
  "distribution": {
    "selection": "static-build",
    "request_selectable": false
  },
  "artifact": {
    "digest": null
  }
}"#;

    fn expected() -> Expected<'static> {
        Expected {
            provider: "hoplite.blob",
            request: "hoplite.blob-request/0-alpha",
            result: "hoplite.blob-result/0-alpha",
            abi_name: "hoplite.blob-provider-ffi",
            abi_version: "1",
            driver_name: "filesystem",
            driver_version: "1",
        }
    }

    fn mutate(change: impl FnOnce(&mut Value)) -> Vec<u8> {
        let mut value: Value = serde_json::from_str(SOURCE_MANIFEST).unwrap();
        change(&mut value);
        serde_json::to_vec(&value).unwrap()
    }

    #[test]
    fn accepts_the_source_tree_blob_manifest() {
        let manifest = validate(
            SOURCE_MANIFEST.as_bytes(),
            expected(),
            ArtifactPolicy::Optional,
        )
        .unwrap();
        assert_eq!(manifest.artifact_digest(), None);
    }

    #[test]
    fn published_manifests_require_and_accept_canonical_digests() {
        assert!(validate(
            SOURCE_MANIFEST.as_bytes(),
            expected(),
            ArtifactPolicy::Required
        )
        .unwrap_err()
        .contains("requires an artifact digest"));

        let source = mutate(|value| {
            value["artifact"]["digest"] = Value::String(format!("sha256:{}", "a".repeat(64)));
        });
        let manifest = validate(&source, expected(), ArtifactPolicy::Required).unwrap();
        assert_eq!(
            manifest.artifact_digest(),
            Some(format!("sha256:{}", "a".repeat(64)).as_str())
        );
    }

    #[test]
    fn closed_objects_reject_missing_and_unknown_fields() {
        let missing = mutate(|value| {
            value.as_object_mut().unwrap().remove("driver");
        });
        assert!(validate(&missing, expected(), ArtifactPolicy::Optional)
            .unwrap_err()
            .contains("missing [driver]"));

        let unknown = mutate(|value| {
            value["abi"].as_object_mut().unwrap().insert(
                "library_path".into(),
                Value::String("/tmp/provider.so".into()),
            );
        });
        assert!(validate(&unknown, expected(), ArtifactPolicy::Optional)
            .unwrap_err()
            .contains("unknown [library_path]"));
    }

    #[test]
    fn duplicate_keys_are_rejected_before_value_selection() {
        let source = SOURCE_MANIFEST.replace(
            "\"provider\": \"hoplite.blob\",",
            "\"provider\": \"hoplite.blob\", \"provider\": \"other\",",
        );
        assert!(
            validate(source.as_bytes(), expected(), ArtifactPolicy::Optional)
                .unwrap_err()
                .contains("duplicate key \"provider\"")
        );
    }

    #[test]
    fn compatibility_mismatches_fail_closed() {
        for source in [
            mutate(|value| value["provider"] = Value::String("hoplite.store".into())),
            mutate(|value| {
                value["contract"]["request"] = Value::String("hoplite.blob-request/0-alpha".into())
            }),
            mutate(|value| {
                value["contract"]["result"] = Value::String("hoplite.blob-result/0-alpha".into())
            }),
            mutate(|value| value["abi"]["version"] = Value::String("2".into())),
            mutate(|value| value["driver"]["version"] = Value::String("2".into())),
        ] {
            assert!(validate(&source, expected(), ArtifactPolicy::Optional)
                .unwrap_err()
                .contains("incompatible"));
        }
    }

    #[test]
    fn distribution_selection_is_static_and_not_request_controlled() {
        let dynamic = mutate(|value| {
            value["distribution"]["selection"] = Value::String("runtime-plugin".into())
        });
        assert!(validate(&dynamic, expected(), ArtifactPolicy::Optional)
            .unwrap_err()
            .contains("incompatible"));

        let request_selectable =
            mutate(|value| value["distribution"]["request_selectable"] = Value::Bool(true));
        assert!(
            validate(&request_selectable, expected(), ArtifactPolicy::Optional)
                .unwrap_err()
                .contains("must not be request-selectable")
        );
    }

    #[test]
    fn artifact_digests_are_lowercase_sha256_values() {
        for digest in [
            "sha256:abc",
            "sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            let source = mutate(|value| value["artifact"]["digest"] = Value::String(digest.into()));
            assert!(validate(&source, expected(), ArtifactPolicy::Required).is_err());
        }
    }

    #[test]
    fn manifest_size_and_json_depth_are_bounded() {
        let oversized = vec![b' '; MAX_MANIFEST_BYTES + 1];
        assert!(validate(&oversized, expected(), ArtifactPolicy::Optional)
            .unwrap_err()
            .contains("with limit"));

        let nested = format!(
            "{}null{}",
            "[".repeat(MAX_JSON_DEPTH + 1),
            "]".repeat(MAX_JSON_DEPTH + 1)
        );
        assert!(
            validate(nested.as_bytes(), expected(), ArtifactPolicy::Optional)
                .unwrap_err()
                .contains("nesting exceeds")
        );
    }
}
