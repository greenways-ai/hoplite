use provider_lock::{Expected as LockExpected, ObjectBackendExpected};
use provider_manifest::{ArtifactPolicy, Expected as ManifestExpected};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

fn main() {
    if let Err(error) = run(env::args().skip(1)) {
        eprintln!("hoplite-object-backend-lock: {error}");
        process::exit(1);
    }
}

fn run(arguments: impl IntoIterator<Item = String>) -> Result<(), String> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    if matches!(
        arguments.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        usage();
        return Ok(());
    }
    if matches!(
        arguments.first().map(String::as_str),
        Some("version" | "--version" | "-V")
    ) {
        println!("Hoplite object backend lock {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let binding_path = arguments.first().map(PathBuf::from).ok_or(
        "object backend verification requires BINDING CONSUMER_MANIFEST BACKEND_LOCK BACKEND_MANIFEST and compatibility options",
    )?;
    let consumer_manifest_path = arguments
        .get(1)
        .map(PathBuf::from)
        .ok_or("object backend verification requires a consumer MANIFEST")?;
    let backend_lock_path = arguments
        .get(2)
        .map(PathBuf::from)
        .ok_or("object backend verification requires a backend LOCK")?;
    let backend_manifest_path = arguments
        .get(3)
        .map(PathBuf::from)
        .ok_or("object backend verification requires a backend published MANIFEST")?;

    let mut options = BTreeMap::new();
    let mut index = 4;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        if !matches!(
            option,
            "--consumer"
                | "--consumer-request"
                | "--consumer-result"
                | "--consumer-abi"
                | "--consumer-abi-version"
                | "--consumer-driver"
                | "--consumer-driver-version"
                | "--backend-package"
                | "--backend-package-version"
                | "--backend-provider"
                | "--backend-version"
                | "--backend-repository"
                | "--backend-tag"
                | "--backend-asset"
                | "--backend-media-type"
                | "--backend-request"
                | "--backend-result"
                | "--backend-abi"
                | "--backend-abi-version"
                | "--backend-driver"
                | "--backend-driver-version"
        ) {
            return Err(format!("unknown object backend lock option {option:?}"));
        }
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| format!("object backend lock option {option} requires a value"))?;
        if value.starts_with("--") {
            return Err(format!(
                "object backend lock option {option} requires a value"
            ));
        }
        if options.insert(option.to_owned(), value.clone()).is_some() {
            return Err(format!("duplicate object backend lock option {option}"));
        }
        index += 2;
    }

    let consumer_expected = ManifestExpected {
        provider: required(&options, "--consumer")?,
        request: required(&options, "--consumer-request")?,
        result: required(&options, "--consumer-result")?,
        abi_name: required(&options, "--consumer-abi")?,
        abi_version: required(&options, "--consumer-abi-version")?,
        driver_name: required(&options, "--consumer-driver")?,
        driver_version: required(&options, "--consumer-driver-version")?,
    };
    let consumer_manifest_source = read(&consumer_manifest_path, "consumer provider manifest")?;
    provider_manifest::validate(
        &consumer_manifest_source,
        consumer_expected,
        ArtifactPolicy::Optional,
    )?;

    let backend_manifest_expected = ManifestExpected {
        provider: required(&options, "--backend-provider")?,
        request: required(&options, "--backend-request")?,
        result: required(&options, "--backend-result")?,
        abi_name: required(&options, "--backend-abi")?,
        abi_version: required(&options, "--backend-abi-version")?,
        driver_name: required(&options, "--backend-driver")?,
        driver_version: required(&options, "--backend-driver-version")?,
    };
    let backend_manifest_source = read(
        &backend_manifest_path,
        "backend published provider manifest",
    )?;
    let backend_manifest = provider_manifest::validate(
        &backend_manifest_source,
        backend_manifest_expected,
        ArtifactPolicy::Required,
    )?;
    let backend_manifest_digest = backend_manifest
        .artifact_digest()
        .ok_or("backend published provider manifest did not contain an artifact digest")?;

    let backend_lock_expected = LockExpected {
        provider: backend_manifest_expected.provider,
        version: required(&options, "--backend-version")?,
        repository: required(&options, "--backend-repository")?,
        tag: required(&options, "--backend-tag")?,
        asset: required(&options, "--backend-asset")?,
        media_type: required(&options, "--backend-media-type")?,
    };
    let backend_lock_source = read(&backend_lock_path, "backend provider lock")?;
    let backend_lock = provider_lock::validate(
        &backend_lock_source,
        backend_lock_expected,
        backend_manifest_digest,
    )?;

    let binding_expected = ObjectBackendExpected {
        consumer: consumer_expected.provider,
        package: required(&options, "--backend-package")?,
        package_version: required(&options, "--backend-package-version")?,
    };
    let binding_source = read(&binding_path, "object backend lock")?;
    let binding = provider_lock::validate_object_backend_lock(
        &binding_source,
        binding_expected,
        &backend_lock,
    )?;

    println!(
        "object backend lock verified: {} {} {} {} {} {}",
        binding.consumer(),
        binding.package(),
        binding.package_version(),
        binding.artifact_provider(),
        binding.artifact_version(),
        binding.artifact_digest()
    );
    Ok(())
}

fn read(path: &PathBuf, context: &str) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|error| format!("cannot read {context} {}: {error}", path.display()))
}

fn required<'a>(options: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, String> {
    options
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("object backend verification requires {name} VALUE"))
}

fn usage() {
    println!("Hoplite object backend lock verifier");
    println!();
    println!("Usage:");
    println!(
        "  hoplite-object-backend-lock BINDING CONSUMER_MANIFEST BACKEND_LOCK BACKEND_MANIFEST \\"
    );
    println!("    --consumer NAME --consumer-request PROTOCOL --consumer-result PROTOCOL \\");
    println!("    --consumer-abi NAME --consumer-abi-version VERSION \\");
    println!("    --consumer-driver NAME --consumer-driver-version VERSION \\");
    println!("    --backend-package NAME --backend-package-version VERSION \\");
    println!("    --backend-provider NAME --backend-version VERSION \\");
    println!("    --backend-repository OWNER/REPOSITORY --backend-tag TAG \\");
    println!("    --backend-asset NAME --backend-media-type TYPE/SUBTYPE \\");
    println!("    --backend-request PROTOCOL --backend-result PROTOCOL \\");
    println!("    --backend-abi NAME --backend-abi-version VERSION \\");
    println!("    --backend-driver NAME --backend-driver-version VERSION");
    println!();
    println!("All paths and compatibility values are trusted distribution inputs.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_options_are_closed_and_unique() {
        let options = BTreeMap::from([("--consumer".to_owned(), "hoplite.value".to_owned())]);
        assert_eq!(required(&options, "--consumer").unwrap(), "hoplite.value");
        assert!(required(&options, "--backend-package").is_err());

        let error = run([
            "binding.json",
            "consumer.json",
            "lock.json",
            "backend.json",
            "--consumer",
            "hoplite.value",
            "--consumer",
            "hoplite.store",
        ]
        .into_iter()
        .map(str::to_owned))
        .unwrap_err();
        assert!(error.contains("duplicate object backend lock option"));
    }

    #[test]
    fn unknown_options_fail_before_file_access() {
        let error = run([
            "binding.json",
            "consumer.json",
            "lock.json",
            "backend.json",
            "--backend-url",
            "https://example.invalid",
        ]
        .into_iter()
        .map(str::to_owned))
        .unwrap_err();
        assert!(error.contains("unknown object backend lock option"));
    }
}
