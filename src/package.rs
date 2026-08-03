use hara_wasm::kernel::{parse, Form};
use semver::Version;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

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

pub fn installed_root(coordinate: &str, version: &Version) -> Result<PathBuf, String> {
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
    if !matches!(field(&manifest, "harp/format"), Some(Form::Number(1))) {
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
                "{{:harp/format 1 :package {{:identity \"gh:greenways-ai/test\" :version \"1.0.0\"}} :files {{\"project.edn\" {{:sha256 \"sha256:{project_hash}\" :size {}}} \"src/value.txt\" {{:sha256 \"sha256:{value_hash}\" :size 7}}}}}}",
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
}
