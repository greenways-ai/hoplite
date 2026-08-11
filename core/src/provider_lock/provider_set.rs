use super::*;

pub const FORMAT: &str = "hoplite.provider-set-lock/v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expected<'a> {
    pub profile: &'a str,
    pub backend_provider: &'a str,
    pub backend_version: &'a str,
    pub consumer_provider: &'a str,
    pub consumer_version: &'a str,
    pub backend_package: &'a str,
    pub backend_package_version: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedSet {
    profile: String,
    backend_provider: String,
    backend_version: String,
    backend_digest: String,
    consumer_provider: String,
    consumer_version: String,
    consumer_digest: String,
    backend_package: String,
    backend_package_version: String,
}

impl ValidatedSet {
    pub fn profile(&self) -> &str {
        &self.profile
    }

    pub fn backend_provider(&self) -> &str {
        &self.backend_provider
    }

    pub fn backend_version(&self) -> &str {
        &self.backend_version
    }

    pub fn backend_digest(&self) -> &str {
        &self.backend_digest
    }

    pub fn consumer_provider(&self) -> &str {
        &self.consumer_provider
    }

    pub fn consumer_version(&self) -> &str {
        &self.consumer_version
    }

    pub fn consumer_digest(&self) -> &str {
        &self.consumer_digest
    }

    pub fn backend_package(&self) -> &str {
        &self.backend_package
    }

    pub fn backend_package_version(&self) -> &str {
        &self.backend_package_version
    }
}

pub fn validate(
    source: &[u8],
    expected: Expected<'_>,
    backend_lock: &super::ValidatedLock,
    consumer_lock: &super::ValidatedLock,
    binding: &super::ValidatedObjectBackendLock,
) -> Result<ValidatedSet, String> {
    if source.is_empty() {
        return Err("provider set lock is empty".into());
    }
    if source.len() > MAX_LOCK_BYTES {
        return Err(format!(
            "provider set lock is {} bytes with limit {}",
            source.len(),
            MAX_LOCK_BYTES
        ));
    }
    validate_expected(expected)?;
    JsonScanner::new(source).validate()?;

    let value: Value = serde_json::from_slice(source)
        .map_err(|error| format!("invalid provider set lock JSON: {error}"))?;
    let root = exact_object(
        &value,
        "provider set lock",
        &["bindings", "format", "profile", "providers"],
    )?;
    require_exact(root, "format", FORMAT, "provider set lock")?;
    require_exact(root, "profile", expected.profile, "provider set lock")?;

    let providers = field(root, "providers", "provider set lock")?
        .as_array()
        .ok_or("provider set lock field providers must be an array")?;
    if providers.len() != 2 {
        return Err(format!(
            "provider set lock requires exactly 2 providers, found {}",
            providers.len()
        ));
    }

    let mut seen = BTreeSet::new();
    let mut backend_seen = false;
    let mut consumer_seen = false;
    for provider in providers {
        let provider = exact_object(
            provider,
            "provider set entry",
            &["digest", "provider", "version"],
        )?;
        let name = require_text(provider, "provider", "provider set entry")?;
        let version = require_text(provider, "version", "provider set entry")?;
        let digest = require_text(provider, "digest", "provider set entry")?;
        validate_digest(digest, "provider set digest")?;
        if !seen.insert(name.to_owned()) {
            return Err(format!("provider set contains duplicate provider {name:?}"));
        }

        if name == expected.backend_provider {
            require_provider(
                "backend",
                version,
                digest,
                expected.backend_version,
                backend_lock,
            )?;
            backend_seen = true;
        } else if name == expected.consumer_provider {
            require_provider(
                "consumer",
                version,
                digest,
                expected.consumer_version,
                consumer_lock,
            )?;
            consumer_seen = true;
        } else {
            return Err(format!("provider set contains unknown provider {name:?}"));
        }
    }
    if !backend_seen {
        return Err(format!(
            "provider set is missing backend provider {:?}",
            expected.backend_provider
        ));
    }
    if !consumer_seen {
        return Err(format!(
            "provider set is missing consumer provider {:?}",
            expected.consumer_provider
        ));
    }

    let bindings = field(root, "bindings", "provider set lock")?
        .as_array()
        .ok_or("provider set lock field bindings must be an array")?;
    if bindings.len() != 1 {
        return Err(format!(
            "provider set lock requires exactly 1 binding, found {}",
            bindings.len()
        ));
    }
    let set_binding = exact_object(
        &bindings[0],
        "provider set binding",
        &[
            "backend",
            "consumer",
            "package",
            "package-version",
        ],
    )?;
    require_exact(
        set_binding,
        "consumer",
        expected.consumer_provider,
        "provider set binding",
    )?;
    require_exact(
        set_binding,
        "backend",
        expected.backend_provider,
        "provider set binding",
    )?;
    require_exact(
        set_binding,
        "package",
        expected.backend_package,
        "provider set binding",
    )?;
    require_exact(
        set_binding,
        "package-version",
        expected.backend_package_version,
        "provider set binding",
    )?;

    if binding.consumer() != expected.consumer_provider
        || binding.artifact_provider() != expected.backend_provider
        || binding.artifact_version() != expected.backend_version
        || binding.artifact_digest() != backend_lock.artifact_digest()
        || binding.package() != expected.backend_package
        || binding.package_version() != expected.backend_package_version
    {
        return Err("provider set binding does not match the validated object backend lock".into());
    }

    Ok(ValidatedSet {
        profile: expected.profile.to_owned(),
        backend_provider: expected.backend_provider.to_owned(),
        backend_version: expected.backend_version.to_owned(),
        backend_digest: backend_lock.artifact_digest().to_owned(),
        consumer_provider: expected.consumer_provider.to_owned(),
        consumer_version: expected.consumer_version.to_owned(),
        consumer_digest: consumer_lock.artifact_digest().to_owned(),
        backend_package: expected.backend_package.to_owned(),
        backend_package_version: expected.backend_package_version.to_owned(),
    })
}

fn require_provider(
    role: &str,
    version: &str,
    digest: &str,
    expected_version: &str,
    lock: &super::ValidatedLock,
) -> Result<(), String> {
    if version != expected_version || version != lock.version() {
        return Err(format!(
            "provider set {role} version is incompatible: set {version:?}, expected {expected_version:?}, lock {:?}",
            lock.version()
        ));
    }
    if digest != lock.artifact_digest() {
        return Err(format!(
            "provider set {role} digest does not match the validated provider lock: set {digest:?}, lock {:?}",
            lock.artifact_digest()
        ));
    }
    Ok(())
}

fn validate_expected(expected: Expected<'_>) -> Result<(), String> {
    for (name, value) in [
        ("expected provider set profile", expected.profile),
        ("expected backend provider", expected.backend_provider),
        ("expected backend version", expected.backend_version),
        ("expected consumer provider", expected.consumer_provider),
        ("expected consumer version", expected.consumer_version),
        ("expected backend package", expected.backend_package),
        (
            "expected backend package version",
            expected.backend_package_version,
        ),
    ] {
        validate_text(name, value)?;
    }
    if expected.backend_provider == expected.consumer_provider {
        return Err("provider set backend and consumer providers must be distinct".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BACKEND_DIGEST: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const CONSUMER_DIGEST: &str =
        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const BACKEND_LOCK: &str = r#"{
  "format": "hoplite.provider-lock/v1",
  "provider": "hoplite.blob",
  "version": "0.1.1",
  "source_revision": "0123456789abcdef0123456789abcdef01234567",
  "release": {"repository": "greenways-ai/hoplite", "tag": "blob-v0.1.1"},
  "artifact": {"name": "blob.tar.gz", "media_type": "application/gzip", "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
}"#;
    const CONSUMER_LOCK: &str = r#"{
  "format": "hoplite.provider-lock/v1",
  "provider": "hoplite.value",
  "version": "0.1.0",
  "source_revision": "fedcba9876543210fedcba9876543210fedcba98",
  "release": {"repository": "greenways-ai/hoplite", "tag": "value-v0.1.0"},
  "artifact": {"name": "value.tar.gz", "media_type": "application/gzip", "digest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
}"#;
    const OBJECT_BINDING: &str = r#"{
  "format": "hoplite.object-backend-lock/v1",
  "consumer": "hoplite.value",
  "backend": {
    "package": "hoplite-blob-filesystem-reader",
    "version": "0.1.0",
    "artifact": {"provider": "hoplite.blob", "version": "0.1.1", "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
  }
}"#;
    const SET: &str = r#"{
  "format": "hoplite.provider-set-lock/v1",
  "profile": "hoplite.blob+value/1",
  "providers": [
    {"provider": "hoplite.blob", "version": "0.1.1", "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
    {"provider": "hoplite.value", "version": "0.1.0", "digest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
  ],
  "bindings": [
    {"consumer": "hoplite.value", "backend": "hoplite.blob", "package": "hoplite-blob-filesystem-reader", "package-version": "0.1.0"}
  ]
}"#;

    fn lock(source: &str, provider: &'static str, version: &'static str, digest: &str) -> super::super::ValidatedLock {
        super::super::validate(
            source.as_bytes(),
            super::super::Expected {
                provider,
                version,
                repository: "greenways-ai/hoplite",
                tag: if provider == "hoplite.blob" { "blob-v0.1.1" } else { "value-v0.1.0" },
                asset: if provider == "hoplite.blob" { "blob.tar.gz" } else { "value.tar.gz" },
                media_type: "application/gzip",
            },
            digest,
        )
        .unwrap()
    }

    fn expected() -> Expected<'static> {
        Expected {
            profile: "hoplite.blob+value/1",
            backend_provider: "hoplite.blob",
            backend_version: "0.1.1",
            consumer_provider: "hoplite.value",
            consumer_version: "0.1.0",
            backend_package: "hoplite-blob-filesystem-reader",
            backend_package_version: "0.1.0",
        }
    }

    fn fixtures() -> (
        super::super::ValidatedLock,
        super::super::ValidatedLock,
        super::super::ValidatedObjectBackendLock,
    ) {
        let backend = lock(BACKEND_LOCK, "hoplite.blob", "0.1.1", BACKEND_DIGEST);
        let consumer = lock(CONSUMER_LOCK, "hoplite.value", "0.1.0", CONSUMER_DIGEST);
        let binding = super::super::validate_object_backend_lock(
            OBJECT_BINDING.as_bytes(),
            super::super::ObjectBackendExpected {
                consumer: "hoplite.value",
                package: "hoplite-blob-filesystem-reader",
                package_version: "0.1.0",
            },
            &backend,
        )
        .unwrap();
        (backend, consumer, binding)
    }

    fn mutate(change: impl FnOnce(&mut Value)) -> Vec<u8> {
        let mut value: Value = serde_json::from_str(SET).unwrap();
        change(&mut value);
        serde_json::to_vec(&value).unwrap()
    }

    #[test]
    fn accepts_one_exact_blob_value_set() {
        let (backend, consumer, binding) = fixtures();
        let set = validate(SET.as_bytes(), expected(), &backend, &consumer, &binding).unwrap();
        assert_eq!(set.profile(), "hoplite.blob+value/1");
        assert_eq!(set.backend_digest(), BACKEND_DIGEST);
        assert_eq!(set.consumer_digest(), CONSUMER_DIGEST);
    }

    #[test]
    fn rejects_missing_duplicate_and_unknown_providers() {
        let (backend, consumer, binding) = fixtures();
        let missing = mutate(|value| {
            value["providers"].as_array_mut().unwrap().pop();
        });
        assert!(validate(&missing, expected(), &backend, &consumer, &binding)
            .unwrap_err()
            .contains("exactly 2 providers"));

        let duplicate = mutate(|value| {
            value["providers"][1] = value["providers"][0].clone();
        });
        assert!(validate(&duplicate, expected(), &backend, &consumer, &binding)
            .unwrap_err()
            .contains("duplicate provider"));

        let unknown = mutate(|value| {
            value["providers"][1]["provider"] = Value::String("hoplite.store".into());
        });
        assert!(validate(&unknown, expected(), &backend, &consumer, &binding)
            .unwrap_err()
            .contains("unknown provider"));
    }

    #[test]
    fn rejects_digest_and_binding_drift() {
        let (backend, consumer, binding) = fixtures();
        let digest = mutate(|value| {
            value["providers"][1]["digest"] = Value::String(BACKEND_DIGEST.into());
        });
        assert!(validate(&digest, expected(), &backend, &consumer, &binding)
            .unwrap_err()
            .contains("does not match"));

        let package = mutate(|value| {
            value["bindings"][0]["package"] = Value::String("other-reader".into());
        });
        assert!(validate(&package, expected(), &backend, &consumer, &binding)
            .unwrap_err()
            .contains("incompatible"));
    }
}
