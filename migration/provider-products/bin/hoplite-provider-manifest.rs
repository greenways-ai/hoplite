use provider_manifest::{ArtifactPolicy, Expected};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

fn main() {
    if let Err(error) = run(env::args().skip(1)) {
        eprintln!("hoplite-provider-manifest: {error}");
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
        println!("Hoplite provider manifest {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let manifest_path = arguments
        .first()
        .map(PathBuf::from)
        .ok_or("provider manifest verification requires MANIFEST and compatibility options")?;
    let mut options = BTreeMap::new();
    let mut require_artifact = false;
    let mut index = 1;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        if option == "--require-artifact" {
            if require_artifact {
                return Err("duplicate provider manifest option --require-artifact".into());
            }
            require_artifact = true;
            index += 1;
            continue;
        }
        if !matches!(
            option,
            "--provider"
                | "--request"
                | "--result"
                | "--abi"
                | "--abi-version"
                | "--driver"
                | "--driver-version"
        ) {
            return Err(format!("unknown provider manifest option {option:?}"));
        }
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| format!("provider manifest option {option} requires a value"))?;
        if value.starts_with("--") {
            return Err(format!(
                "provider manifest option {option} requires a value"
            ));
        }
        if options.insert(option.to_owned(), value.clone()).is_some() {
            return Err(format!("duplicate provider manifest option {option}"));
        }
        index += 2;
    }

    let expected = Expected {
        provider: required(&options, "--provider")?,
        request: required(&options, "--request")?,
        result: required(&options, "--result")?,
        abi_name: required(&options, "--abi")?,
        abi_version: required(&options, "--abi-version")?,
        driver_name: required(&options, "--driver")?,
        driver_version: required(&options, "--driver-version")?,
    };
    let source = fs::read(&manifest_path).map_err(|error| {
        format!(
            "cannot read provider manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    let artifact_policy = if require_artifact {
        ArtifactPolicy::Required
    } else {
        ArtifactPolicy::Optional
    };
    let manifest = provider_manifest::validate(&source, expected, artifact_policy)?;
    let artifact = manifest.artifact_digest().unwrap_or("source-tree");
    println!(
        "provider manifest verified: {} {} {}/{} ({artifact})",
        expected.provider,
        manifest_path.display(),
        expected.driver_name,
        expected.driver_version
    );
    Ok(())
}

fn required<'a>(options: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, String> {
    options
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("provider manifest verification requires {name} VALUE"))
}

fn usage() {
    println!("Hoplite provider package manifest verifier");
    println!();
    println!("Usage:");
    println!("  hoplite-provider-manifest MANIFEST \\");
    println!("    --provider NAME --request PROTOCOL --result PROTOCOL \\");
    println!("    --abi NAME --abi-version VERSION \\");
    println!("    --driver NAME --driver-version VERSION [--require-artifact]");
    println!();
    println!("Compatibility values are supplied only by trusted distribution configuration.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_options_are_closed_and_unique() {
        let options = BTreeMap::from([("--provider".to_owned(), "hoplite.blob".to_owned())]);
        assert_eq!(required(&options, "--provider").unwrap(), "hoplite.blob");
        assert!(required(&options, "--request").is_err());

        let error = run([
            "manifest.json",
            "--provider",
            "hoplite.blob",
            "--provider",
            "hoplite.store",
        ]
        .into_iter()
        .map(str::to_owned))
        .unwrap_err();
        assert!(error.contains("duplicate provider manifest option"));
    }

    #[test]
    fn unknown_options_fail_before_file_access() {
        let error = run(["missing.json", "--library-path", "/tmp/provider.so"]
            .into_iter()
            .map(str::to_owned))
        .unwrap_err();
        assert!(error.contains("unknown provider manifest option"));
    }
}
