use super::*;

pub const FORMAT: &str = "hoplite.object-backend-lock/0-alpha";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expected<'a> {
    pub consumer: &'a str,
    pub package: &'a str,
    pub package_version: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedLock {
    consumer: String,
    package: String,
    package_version: String,
    artifact_provider: String,
    artifact_version: String,
    artifact_digest: String,
}

impl ValidatedLock {
    pub fn consumer(&self) -> &str {
        &self.consumer
    }

    pub fn package(&self) -> &str {
        &self.package
    }

    pub fn package_version(&self) -> &str {
        &self.package_version
    }

    pub fn artifact_provider(&self) -> &str {
        &self.artifact_provider
    }

    pub fn artifact_version(&self) -> &str {
        &self.artifact_version
    }

    pub fn artifact_digest(&self) -> &str {
        &self.artifact_digest
    }
}

pub fn validate(
    source: &[u8],
    expected: Expected<'_>,
    backend: &super::ValidatedLock,
) -> Result<ValidatedLock, String> {
    if source.is_empty() {
        return Err("object backend lock is empty".into());
    }
    if source.len() > MAX_LOCK_BYTES {
        return Err(format!(
            "object backend lock is {} bytes with limit {}",
            source.len(),
            MAX_LOCK_BYTES
        ));
    }
    validate_expected(expected)?;
    JsonScanner::new(source).validate()?;

    let value: Value = serde_json::from_slice(source)
        .map_err(|error| format!("invalid object backend lock JSON: {error}"))?;
    let root = exact_object(
        &value,
        "object backend lock",
        &["backend", "consumer", "format"],
    )?;
    require_exact(root, "format", FORMAT, "object backend lock")?;
    require_exact(root, "consumer", expected.consumer, "object backend lock")?;

    let binding = exact_object(
        field(root, "backend", "object backend lock")?,
        "object backend binding",
        &["artifact", "package", "version"],
    )?;
    require_exact(
        binding,
        "package",
        expected.package,
        "object backend binding",
    )?;
    require_exact(
        binding,
        "version",
        expected.package_version,
        "object backend binding",
    )?;

    let artifact = exact_object(
        field(binding, "artifact", "object backend binding")?,
        "object backend artifact",
        &["digest", "provider", "version"],
    )?;
    require_exact(
        artifact,
        "provider",
        backend.provider(),
        "object backend artifact",
    )?;
    require_exact(
        artifact,
        "version",
        backend.version(),
        "object backend artifact",
    )?;
    let artifact_digest = require_text(artifact, "digest", "object backend artifact")?;
    validate_digest(artifact_digest, "object backend artifact digest")?;
    if artifact_digest != backend.artifact_digest() {
        return Err(format!(
            "object backend artifact digest does not match the validated provider lock: binding {artifact_digest:?}, provider lock {:?}",
            backend.artifact_digest()
        ));
    }

    Ok(ValidatedLock {
        consumer: expected.consumer.to_owned(),
        package: expected.package.to_owned(),
        package_version: expected.package_version.to_owned(),
        artifact_provider: backend.provider().to_owned(),
        artifact_version: backend.version().to_owned(),
        artifact_digest: artifact_digest.to_owned(),
    })
}

fn validate_expected(expected: Expected<'_>) -> Result<(), String> {
    for (name, value) in [
        ("expected object backend consumer", expected.consumer),
        ("expected object backend package", expected.package),
        (
            "expected object backend package version",
            expected.package_version,
        ),
    ] {
        validate_text(name, value)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const PROVIDER_LOCK: &str = r#"{
  "format": "hoplite.provider-lock/0-alpha",
  "provider": "hoplite.blob",
  "version": "0.1.1",
  "source_revision": "0123456789abcdef0123456789abcdef01234567",
  "release": {
    "repository": "greenways-ai/hoplite",
    "tag": "hoplite-blob-provider-v0.1.1"
  },
  "artifact": {
    "name": "hoplite-blob-provider-0.1.1.tar.gz",
    "media_type": "application/gzip",
    "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  }
}"#;
    const BINDING: &str = r#"{
  "format": "hoplite.object-backend-lock/0-alpha",
  "consumer": "hoplite.value",
  "backend": {
    "package": "hoplite-blob-filesystem-reader",
    "version": "0.1.0",
    "artifact": {
      "provider": "hoplite.blob",
      "version": "0.1.1",
      "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }
  }
}"#;

    fn backend() -> super::super::ValidatedLock {
        super::super::validate(
            PROVIDER_LOCK.as_bytes(),
            super::super::Expected {
                provider: "hoplite.blob",
                version: "0.1.1",
                repository: "greenways-ai/hoplite",
                tag: "hoplite-blob-provider-v0.1.1",
                asset: "hoplite-blob-provider-0.1.1.tar.gz",
                media_type: "application/gzip",
            },
            DIGEST,
        )
        .unwrap()
    }

    fn expected() -> Expected<'static> {
        Expected {
            consumer: "hoplite.value",
            package: "hoplite-blob-filesystem-reader",
            package_version: "0.1.0",
        }
    }

    fn mutate(change: impl FnOnce(&mut Value)) -> Vec<u8> {
        let mut value: Value = serde_json::from_str(BINDING).unwrap();
        change(&mut value);
        serde_json::to_vec(&value).unwrap()
    }

    #[test]
    fn accepts_one_exact_provider_bound_backend() {
        let binding = validate(BINDING.as_bytes(), expected(), &backend()).unwrap();
        assert_eq!(binding.consumer(), "hoplite.value");
        assert_eq!(binding.package(), "hoplite-blob-filesystem-reader");
        assert_eq!(binding.package_version(), "0.1.0");
        assert_eq!(binding.artifact_provider(), "hoplite.blob");
        assert_eq!(binding.artifact_version(), "0.1.1");
        assert_eq!(binding.artifact_digest(), DIGEST);
    }

    #[test]
    fn rejects_missing_unknown_and_duplicate_fields() {
        let missing = mutate(|value| {
            value["backend"].as_object_mut().unwrap().remove("artifact");
        });
        assert!(validate(&missing, expected(), &backend())
            .unwrap_err()
            .contains("missing [artifact]"));

        let unknown = mutate(|value| {
            value["backend"]
                .as_object_mut()
                .unwrap()
                .insert("root".into(), Value::String("/tmp/forbidden".into()));
        });
        assert!(validate(&unknown, expected(), &backend())
            .unwrap_err()
            .contains("unknown [root]"));

        let duplicate = BINDING.replace(
            "\"consumer\": \"hoplite.value\",",
            "\"consumer\": \"hoplite.value\", \"consumer\": \"other\",",
        );
        assert!(validate(duplicate.as_bytes(), expected(), &backend())
            .unwrap_err()
            .contains("duplicate key \"consumer\""));
    }

    #[test]
    fn rejects_package_and_provider_artifact_drift() {
        let package =
            mutate(|value| value["backend"]["package"] = Value::String("other-reader".into()));
        assert!(validate(&package, expected(), &backend())
            .unwrap_err()
            .contains("incompatible"));

        let version =
            mutate(|value| value["backend"]["artifact"]["version"] = Value::String("0.2.0".into()));
        assert!(validate(&version, expected(), &backend())
            .unwrap_err()
            .contains("incompatible"));

        let digest = mutate(|value| {
            value["backend"]["artifact"]["digest"] = Value::String(
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            )
        });
        assert!(validate(&digest, expected(), &backend())
            .unwrap_err()
            .contains("does not match"));
    }
}
