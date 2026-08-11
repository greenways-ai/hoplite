#[path = "../provider_lock.rs"]
mod provider_lock;
#[path = "../provider_manifest.rs"]
mod provider_manifest;

use provider_lock::{
    Expected as LockExpected, ObjectBackendExpected, ProviderSetExpected,
};
use provider_manifest::{ArtifactPolicy, Expected as ManifestExpected};
use std::env;
use std::fs;
use std::process;

fn main() {
    if let Err(error) = run(env::args().skip(1).collect()) {
        eprintln!("hoplite-provider-set-lock: {error}");
        process::exit(1);
    }
}

fn run(arguments: Vec<String>) -> Result<(), String> {
    if matches!(arguments.first().map(String::as_str), Some("help" | "--help" | "-h")) {
        usage();
        return Ok(());
    }
    if arguments.len() != 7 {
        return Err("provider set verification requires SET BLOB_LOCK BLOB_MANIFEST VALUE_LOCK VALUE_MANIFEST VALUE_BACKEND_LOCK PROFILE".into());
    }
    let set_source = read(&arguments[0])?;
    let blob_manifest_source = read(&arguments[2])?;
    let value_manifest_source = read(&arguments[4])?;

    let blob_manifest = provider_manifest::validate(
        &blob_manifest_source,
        ManifestExpected {
            provider: "hoplite.blob",
            request: "hoplite.blob-request/1",
            result: "hoplite.blob-result/1",
            abi_name: "hoplite.blob-provider-ffi",
            abi_version: "1",
            driver_name: "filesystem",
            driver_version: "1",
        },
        ArtifactPolicy::Required,
    )?;
    let value_manifest = provider_manifest::validate(
        &value_manifest_source,
        ManifestExpected {
            provider: "hoplite.value",
            request: "hoplite.value-request/1",
            result: "hoplite.value-result/1",
            abi_name: "hoplite.value-provider-ffi",
            abi_version: "1",
            driver_name: "filesystem",
            driver_version: "1",
        },
        ArtifactPolicy::Required,
    )?;

    let blob_lock = provider_lock::validate(
        &read(&arguments[1])?,
        LockExpected {
            provider: "hoplite.blob",
            version: "0.1.1",
            repository: "greenways-ai/hoplite",
            tag: "hoplite-blob-provider-v0.1.1",
            asset: "hoplite-blob-provider-0.1.1.tar.gz",
            media_type: "application/gzip",
        },
        blob_manifest
            .artifact_digest()
            .ok_or("blob published manifest has no artifact digest")?,
    )?;
    let value_lock = provider_lock::validate(
        &read(&arguments[3])?,
        LockExpected {
            provider: "hoplite.value",
            version: "0.1.0",
            repository: "greenways-ai/hoplite",
            tag: "hoplite-value-provider-v0.1.0",
            asset: "hoplite-value-provider-0.1.0.tar.gz",
            media_type: "application/gzip",
        },
        value_manifest
            .artifact_digest()
            .ok_or("value published manifest has no artifact digest")?,
    )?;
    let binding = provider_lock::validate_object_backend_lock(
        &read(&arguments[5])?,
        ObjectBackendExpected {
            consumer: "hoplite.value",
            package: "hoplite-blob-filesystem-reader",
            package_version: "0.1.0",
        },
        &blob_lock,
    )?;
    let set = provider_lock::validate_provider_set_lock(
        &set_source,
        ProviderSetExpected {
            profile: &arguments[6],
            backend_provider: "hoplite.blob",
            backend_version: "0.1.1",
            consumer_provider: "hoplite.value",
            consumer_version: "0.1.0",
            backend_package: "hoplite-blob-filesystem-reader",
            backend_package_version: "0.1.0",
        },
        &blob_lock,
        &value_lock,
        &binding,
    )?;

    println!(
        "provider set verified: {} {}@{} {}@{} {}@{}",
        set.profile(),
        set.backend_provider(),
        set.backend_version(),
        set.consumer_provider(),
        set.consumer_version(),
        set.backend_package(),
        set.backend_package_version()
    );
    Ok(())
}

fn read(path: &str) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|error| format!("cannot read {path}: {error}"))
}

fn usage() {
    println!("Hoplite provider set lock verifier");
    println!();
    println!("Usage:");
    println!("  hoplite-provider-set-lock SET BLOB_LOCK BLOB_MANIFEST \\");
    println!("    VALUE_LOCK VALUE_MANIFEST VALUE_BACKEND_LOCK PROFILE");
    println!();
    println!("The first profile is the closed pinned hoplite.blob + hoplite.value set.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_the_closed_argument_set() {
        assert!(run(vec![]).unwrap_err().contains("requires SET"));
    }
}
