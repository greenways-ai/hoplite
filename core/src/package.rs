use hara_wasm::kernel::{parse, Form};
use semver::Version;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn run(arguments: &[String]) -> Result<(), String> {
    if arguments.first().map(String::as_str) == Some("verify") {
        let coordinate = arguments
            .get(1)
            .ok_or("hoplite package verify requires COORDINATE VERSION")?;
        let version_text = arguments
            .get(2)
            .ok_or("hoplite package verify requires COORDINATE VERSION")?;
        let version = Version::parse(version_text)
            .map_err(|error| format!("invalid package version: {error}"))?;
        let root = installed_root(coordinate, &version)?;
        println!(
            "package verified: {} {} ({})",
            coordinate,
            version,
            root.display()
        );
        return Ok(());
    }
    hara_wasm::package::run(arguments)
}

pub fn ensure_locked(package: &crate::package_catalog::LockedPackage) -> Result<PathBuf, String> {
    let version = Version::parse(&package.version)
        .map_err(|error| format!("invalid locked package version: {error}"))?;
    let expected = package
        .archive_sha256
        .strip_prefix("sha256:")
        .unwrap_or(&package.archive_sha256);
    if let Ok(root) = installed_root_locked(
        &package.coordinate,
        &version,
        Some(&format!("sha256:{expected}")),
    ) {
        return Ok(root);
    }
    let origin = std::env::var("HARA_PACKAGES_URL")
        .unwrap_or_else(|_| "https://packages.hara-lang.org".into())
        .trim_end_matches('/')
        .to_owned();
    let registry_url = format!("{origin}/v1/registry?ref={}", package.registry_commit);
    let registry = curl_bytes(&registry_url, None)?;
    verify_registry_release(&registry, package)?;
    let object_url = format!("{origin}/objects/sha256/{expected}");
    let temporary = std::env::temp_dir().join(format!(
        "hoplite-package-{}-{expected}.harp",
        std::process::id()
    ));
    curl_bytes(&object_url, Some(&temporary))?;
    let actual = encode_hex(&Sha256::digest(fs::read(&temporary).map_err(io)?));
    if actual != expected {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "downloaded package digest mismatch: expected sha256:{expected}, got sha256:{actual}"
        ));
    }
    let installed = hara_wasm::package::install_path(&temporary);
    let _ = fs::remove_file(&temporary);
    installed?;
    installed_root_locked(
        &package.coordinate,
        &version,
        Some(&format!("sha256:{expected}")),
    )
}

fn curl_bytes(url: &str, output: Option<&Path>) -> Result<Vec<u8>, String> {
    let mut command = Command::new("curl");
    command.args([
        "--fail",
        "--location",
        "--silent",
        "--show-error",
        "--proto",
        "=https",
        "--tlsv1.2",
    ]);
    if let Some(path) = output {
        command.arg("--output").arg(path);
    }
    command.arg(url);
    if output.is_some() {
        let status = command
            .status()
            .map_err(|error| format!("cannot execute curl: {error}"))?;
        if !status.success() {
            return Err(format!("cannot download package resource {url}"));
        }
        return Ok(Vec::new());
    }
    let result = command
        .output()
        .map_err(|error| format!("cannot execute curl: {error}"))?;
    if !result.status.success() {
        return Err(format!("cannot download package registry {url}"));
    }
    Ok(result.stdout)
}

fn verify_registry_release(
    source: &[u8],
    package: &crate::package_catalog::LockedPackage,
) -> Result<(), String> {
    let source = std::str::from_utf8(source).map_err(|_| "package registry is not UTF-8")?;
    let Form::Map(registry) = parse(source)? else {
        return Err("package registry must be an EDN map".into());
    };
    let packages = map_field(&registry, "registry/packages", Path::new("registry.edn"))?;
    let release = packages.iter().find_map(|(coordinate, releases)| {
        (matches!(coordinate, Form::String(value) | Form::Symbol(value) | Form::Keyword(value) if value == &package.coordinate))
            .then_some(releases)
    }).ok_or_else(|| format!("package/not-in-registry: {}", package.coordinate))?;
    let Form::Map(releases) = release else {
        return Err("registry package versions must be a map".into());
    };
    let descriptor = releases.iter().find_map(|(version, descriptor)| {
        (matches!(version, Form::String(value) | Form::Symbol(value) if value == &package.version))
            .then_some(descriptor)
    }).ok_or_else(|| format!("package/version-not-in-registry: {} {}", package.coordinate, package.version))?;
    let Form::Map(descriptor) = descriptor else {
        return Err("registry release descriptor must be a map".into());
    };
    for (field, expected) in [
        ("archive-sha256", package.archive_sha256.as_str()),
        ("identity-revision", package.identity_revision.as_str()),
    ] {
        if string_field(descriptor, field).as_deref() != Some(expected) {
            return Err(format!(
                "package/registry-mismatch: {} {} :{field}",
                package.coordinate, package.version
            ));
        }
    }
    Ok(())
}

pub fn installed_root(coordinate: &str, version: &Version) -> Result<PathBuf, String> {
    installed_root_locked(coordinate, version, None)
}

pub fn installed_root_locked(
    coordinate: &str,
    version: &Version,
    expected_archive_sha256: Option<&str>,
) -> Result<PathBuf, String> {
    let installed_coordinate = installed_coordinate(coordinate)?;
    let (tap, package) = installed_coordinate
        .split_once(':')
        .ok_or_else(|| format!("invalid installed package coordinate {coordinate:?}"))?;
    let (owner, name) = package
        .split_once('/')
        .ok_or_else(|| format!("invalid installed package coordinate {coordinate:?}"))?;
    let registration = dist_root()
        .join("packages")
        .join(tap)
        .join(owner)
        .join(name)
        .join(format!("{version}.edn"));
    let source = fs::read_to_string(&registration).map_err(|error| {
        format!(
            "package {coordinate} {version} is not installed ({}): {error}",
            registration.display()
        )
    })?;
    let Form::Map(entries) = parse(&source)? else {
        return Err(format!(
            "{} must contain an EDN map",
            registration.display()
        ));
    };
    let root = string_field(&entries, "root").map(PathBuf::from);
    let root = root.ok_or_else(|| format!("{} is missing :root", registration.display()))?;
    if !root.join("project.edn").is_file() {
        return Err(format!(
            "installed package root {} is incomplete",
            root.display()
        ));
    }
    let archive_digest = string_field(&entries, "archive-sha256")
        .ok_or_else(|| format!("{} is missing :archive-sha256", registration.display()))?;
    let digest = archive_digest
        .strip_prefix("sha256:")
        .ok_or_else(|| format!("{} has an invalid archive digest", registration.display()))?;
    if expected_archive_sha256.is_some_and(|expected| expected != archive_digest) {
        return Err(format!(
            "installed package {coordinate} {version} archive digest does not match project.lock.edn"
        ));
    }
    if root.file_name().and_then(|name| name.to_str()) != Some(digest) {
        return Err(format!(
            "installed package root {} does not match registered archive digest",
            root.display()
        ));
    }
    verify_root(&root, &installed_coordinate, version)?;
    Ok(root)
}

fn installed_coordinate(coordinate: &str) -> Result<String, String> {
    if let Some(repository) = coordinate.strip_prefix("gh:") {
        let mut parts = repository.split(':');
        if let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) {
            return Ok(format!("gh:{owner}/{name}"));
        }
    }
    hara_wasm::project::normalize_coordinate(coordinate)
}

fn verify_root(root: &Path, coordinate: &str, version: &Version) -> Result<(), String> {
    let manifest_path = root.join("package.edn");
    let manifest_source = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("cannot read {}: {error}", manifest_path.display()))?;
    let Form::Map(manifest) = parse(&manifest_source)? else {
        return Err(format!(
            "{} must contain an EDN map",
            manifest_path.display()
        ));
    };
    if !matches!(field(&manifest, "harp/format"), Some(Form::String(version)) if version == "0.0.0-alpha")
    {
        return Err(format!(
            "{} has an unsupported HARP format",
            manifest_path.display()
        ));
    }
    let package = map_field(&manifest, "package", &manifest_path)?;
    if string_field(package, "identity").as_deref() != Some(coordinate)
        || string_field(package, "version").as_deref() != Some(version.to_string().as_str())
    {
        return Err(format!(
            "{} identity or version does not match activation",
            manifest_path.display()
        ));
    }
    let declared = map_field(&manifest, "files", &manifest_path)?;
    let mut expected = BTreeMap::new();
    for (path, descriptor) in declared {
        let Form::String(path) = path else {
            return Err("package.edn file paths must be strings".into());
        };
        let Form::Map(descriptor) = descriptor else {
            return Err(format!("package.edn descriptor for {path:?} must be a map"));
        };
        let digest = string_field(descriptor, "sha256")
            .and_then(|value| value.strip_prefix("sha256:").map(str::to_owned))
            .ok_or_else(|| format!("package.edn descriptor for {path:?} has no SHA-256"))?;
        let size = match field(descriptor, "size") {
            Some(Form::Number(value)) => u64::try_from(*value)
                .map_err(|_| format!("package.edn size for {path:?} is invalid"))?,
            _ => return Err(format!("package.edn descriptor for {path:?} has no size")),
        };
        expected.insert(path.clone(), (digest, size));
    }
    let mut actual = BTreeMap::new();
    collect_files(root, root, &mut actual)?;
    actual.remove("package.edn");
    if actual.keys().ne(expected.keys()) {
        return Err("installed package files do not match package.edn".into());
    }
    for (path, bytes) in actual {
        let (digest, size) = &expected[&path];
        if bytes.len() as u64 != *size || encode_hex(&Sha256::digest(&bytes)) != *digest {
            return Err(format!(
                "installed package file {path:?} failed integrity verification"
            ));
        }
    }
    Ok(())
}

fn collect_files(
    root: &Path,
    directory: &Path,
    output: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?
    {
        let entry = entry.map_err(|error| format!("cannot read package entry: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
        if file_type.is_symlink() {
            return Err(format!(
                "installed package contains symlink {}",
                entry.path().display()
            ));
        }
        if file_type.is_dir() {
            collect_files(root, &entry.path(), output)?;
        } else if file_type.is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|_| "installed package path escaped its root")?
                .to_string_lossy()
                .replace('\\', "/");
            output.insert(relative, fs::read(entry.path()).map_err(io)?);
        }
    }
    Ok(())
}

fn map_field<'a>(
    entries: &'a [(Form, Form)],
    name: &str,
    path: &Path,
) -> Result<&'a [(Form, Form)], String> {
    match field(entries, name) {
        Some(Form::Map(value)) => Ok(value),
        _ => Err(format!("{} is missing map :{name}", path.display())),
    }
}

fn field<'a>(entries: &'a [(Form, Form)], name: &str) -> Option<&'a Form> {
    entries.iter().find_map(|(key, value)| {
        matches!(key, Form::Keyword(candidate) if candidate == name).then_some(value)
    })
}

fn string_field(entries: &[(Form, Form)], name: &str) -> Option<String> {
    match field(entries, name) {
        Some(Form::String(value)) => Some(value.clone()),
        _ => None,
    }
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

fn io(error: std::io::Error) -> String {
    format!("package I/O error: {error}")
}

fn dist_root() -> PathBuf {
    std::env::var_os("HARA_DIST_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".hara/dist")))
        .unwrap_or_else(|| PathBuf::from(".hara/dist"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_packages_fail_with_an_install_message() {
        let error = installed_root(
            "gh:greenways-ai/definitely-not-installed",
            &Version::parse("9.9.9").unwrap(),
        )
        .unwrap_err();
        assert!(error.contains("is not installed"));
    }

    #[test]
    fn installed_roots_are_verified_against_the_harp_manifest() {
        let root = std::env::temp_dir().join(format!(
            "hoplite-installed-integrity-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        let project = b"{:hara/type :project}\n";
        fs::write(root.join("project.edn"), project).unwrap();
        fs::write(root.join("src/value.txt"), b"trusted").unwrap();
        let project_hash = encode_hex(&Sha256::digest(project));
        let value_hash = encode_hex(&Sha256::digest(b"trusted"));
        fs::write(
            root.join("package.edn"),
            format!(
                "{{:harp/format \"0.0.0-alpha\" :package {{:identity \"gh:greenways-ai/test\" :version \"1.0.0\"}} :files {{\"project.edn\" {{:sha256 \"sha256:{project_hash}\" :size {}}} \"src/value.txt\" {{:sha256 \"sha256:{value_hash}\" :size 7}}}}}}",
                project.len()
            ),
        )
        .unwrap();
        verify_root(
            &root,
            "gh:greenways-ai/test",
            &Version::parse("1.0.0").unwrap(),
        )
        .unwrap();
        fs::write(root.join("src/value.txt"), b"tampered").unwrap();
        assert!(verify_root(
            &root,
            "gh:greenways-ai/test",
            &Version::parse("1.0.0").unwrap()
        )
        .unwrap_err()
        .contains("failed integrity verification"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registry_release_must_match_the_locked_digest_and_identity_revision() {
        let package = crate::package_catalog::LockedPackage {
            coordinate: "gh:greenways-ai:demo".into(),
            version: "1.2.3".into(),
            tap: "hara".into(),
            registry_commit: "a".repeat(40),
            identity_revision: "b".repeat(40),
            archive_sha256: format!("sha256:{}", "c".repeat(64)),
            namespaces: vec!["demo.core".into()],
        };
        let registry = format!(
            "{{:registry/packages {{\"gh:greenways-ai:demo\" {{\"1.2.3\" {{:archive-sha256 \"sha256:{}\" :identity-revision \"{}\"}}}}}}}}",
            "c".repeat(64), "b".repeat(40)
        );
        verify_registry_release(registry.as_bytes(), &package).unwrap();
        assert!(verify_registry_release(
            registry
                .replace(&"c".repeat(64), &"d".repeat(64))
                .as_bytes(),
            &package
        )
        .unwrap_err()
        .contains("package/registry-mismatch"));
    }
}
