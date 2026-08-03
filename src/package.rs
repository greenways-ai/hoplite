use hara_wasm::kernel::{parse, Form};
use semver::Version;
use std::fs;
use std::path::{Path, PathBuf};

pub fn run(arguments: &[String]) -> Result<(), String> {
    hara_wasm::package::run(arguments)
}

pub fn installed_root(coordinate: &str, version: &Version) -> Result<PathBuf, String> {
    let (tap, package) = coordinate
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
    let root = entries.iter().find_map(|(key, value)| {
        matches!(key, Form::Keyword(name) if name == "root").then(|| match value {
            Form::String(path) => Some(PathBuf::from(path)),
            _ => None,
        })?
    });
    let root = root.ok_or_else(|| format!("{} is missing :root", registration.display()))?;
    if !root.join("project.edn").is_file() {
        return Err(format!(
            "installed package root {} is incomplete",
            root.display()
        ));
    }
    Ok(root)
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
}
