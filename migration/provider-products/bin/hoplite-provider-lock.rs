use provider_lock::Expected as LockExpected;
use provider_manifest::{ArtifactPolicy, Expected as ManifestExpected};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

fn main() {
    if let Err(error) = run(env::args().skip(1)) {
        eprintln!("hoplite-provider-lock: {error}");
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
        println!("Hoplite provider lock {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let lock_path = arguments
        .first()
        .map(PathBuf::from)
        .ok_or("provider lock verification requires LOCK MANIFEST and compatibility options")?;
    let manifest_path = arguments
        .get(1)
        .map(PathBuf::from)
        .ok_or("provider lock verification requires a published MANIFEST")?;
    let mut options = BTreeMap::new();
    let mut shell = false;
    let mut index = 2;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        if option == "--shell" {
            if shell {
                return Err("duplicate provider lock option --shell".into());
            }
            shell = true;
            index += 1;
            continue;
        }
        if !matches!(
            option,
            "--provider"
                | "--version"
                | "--repository"
                | "--tag"
                | "--asset"
                | "--media-type"
                | "--request"
                | "--result"
                | "--abi"
                | "--abi-version"
                | "--driver"
                | "--driver-version"
        ) {
            return Err(format!("unknown provider lock option {option:?}"));
        }
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| format!("provider lock option {option} requires a value"))?;
        if value.starts_with("--") {
            return Err(format!("provider lock option {option} requires a value"));
        }
        if options.insert(option.to_owned(), value.clone()).is_some() {
            return Err(format!("duplicate provider lock option {option}"));
        }
        index += 2;
    }

    let manifest_expected = ManifestExpected {
        provider: required(&options, "--provider")?,
        request: required(&options, "--request")?,
        result: required(&options, "--result")?,
        abi_name: required(&options, "--abi")?,
        abi_version: required(&options, "--abi-version")?,
        driver_name: required(&options, "--driver")?,
        driver_version: required(&options, "--driver-version")?,
    };
    let lock_expected = LockExpected {
        provider: manifest_expected.provider,
        version: required(&options, "--version")?,
        repository: required(&options, "--repository")?,
        tag: required(&options, "--tag")?,
        asset: required(&options, "--asset")?,
        media_type: required(&options, "--media-type")?,
    };

    let manifest_source = read(&manifest_path, "published provider manifest")?;
    let manifest = provider_manifest::validate(
        &manifest_source,
        manifest_expected,
        ArtifactPolicy::Required,
    )?;
    let manifest_digest = manifest
        .artifact_digest()
        .ok_or("published provider manifest did not contain an artifact digest")?;

    let lock_source = read(&lock_path, "provider lock")?;
    let lock = provider_lock::validate(&lock_source, lock_expected, manifest_digest)?;
    if shell {
        print!("{}", lock.shell_environment());
    } else {
        println!(
            "provider lock verified: {} {} {} {}",
            lock.provider(),
            lock.version(),
            lock.source_revision(),
            lock.artifact_digest()
        );
    }
    Ok(())
}

fn read(path: &PathBuf, context: &str) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|error| format!("cannot read {context} {}: {error}", path.display()))
}

fn required<'a>(options: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, String> {
    options
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("provider lock verification requires {name} VALUE"))
}

fn usage() {
    println!("Hoplite provider distribution lock verifier");
    println!();
    println!("Usage:");
    println!("  hoplite-provider-lock LOCK PUBLISHED_MANIFEST \\");
    println!("    --provider NAME --version VERSION \\");
    println!("    --repository OWNER/REPOSITORY --tag TAG --asset NAME \\");
    println!("    --media-type TYPE/SUBTYPE \\");
    println!("    --request PROTOCOL --result PROTOCOL \\");
    println!("    --abi NAME --abi-version VERSION \\");
    println!("    --driver NAME --driver-version VERSION [--shell]");
    println!();
    println!("The lock and all compatibility expectations are trusted distribution inputs.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_options_are_closed_and_unique() {
        let options = BTreeMap::from([("--provider".to_owned(), "hoplite.blob".to_owned())]);
        assert_eq!(required(&options, "--provider").unwrap(), "hoplite.blob");
        assert!(required(&options, "--version").is_err());

        let error = run([
            "lock.json",
            "manifest.json",
            "--provider",
            "hoplite.blob",
            "--provider",
            "hoplite.store",
        ]
        .into_iter()
        .map(str::to_owned))
        .unwrap_err();
        assert!(error.contains("duplicate provider lock option"));
    }

    #[test]
    fn unknown_options_fail_before_file_access() {
        let error = run([
            "lock.json",
            "manifest.json",
            "--url",
            "https://example.invalid",
        ]
        .into_iter()
        .map(str::to_owned))
        .unwrap_err();
        assert!(error.contains("unknown provider lock option"));
    }
}
