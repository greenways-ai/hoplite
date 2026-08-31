use hara_native::kernel::{parse, Form};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackage {
    pub coordinate: String,
    pub version: String,
    pub tap: String,
    pub oci_repository: String,
    pub oci_manifest: String,
    pub archive_sha256: String,
    pub namespaces: Vec<String>,
}

pub fn catalog_from_lock(source: &str) -> Result<Vec<LockedPackage>, String> {
    let document = parse(source)?;
    let root = map(&document, "project.lock.edn must be an EDN map")?;
    if !matches!(lookup(root, "lock/format"), Some(Form::String(version)) if version == "0.0.1") {
        return Err("project.lock.edn requires :lock/format \"0.0.1\"".into());
    }
    let packages = match lookup(root, "packages") {
        Some(value) => map(value, "project.lock.edn :packages must be a map")?,
        None => return Ok(Vec::new()),
    };
    let mut output = Vec::with_capacity(packages.len());
    for (coordinate, descriptor) in packages {
        let coordinate = scalar(coordinate, "locked package coordinate")?;
        let descriptor = map(descriptor, "locked package descriptor must be a map")?;
        let version = string(required(descriptor, "version")?, "locked package :version")?;
        semver::Version::parse(&version)
            .map_err(|error| format!("locked package {coordinate} has invalid version: {error}"))?;
        let archive_sha256 = string(
            required(descriptor, "archive-sha256")?,
            "locked package :archive-sha256",
        )?;
        validate_sha256(&archive_sha256)?;
        let tap = string(required(descriptor, "tap")?, "locked package :tap")?;
        let oci_repository = string(
            required(descriptor, "oci/repository")?,
            "locked package :oci/repository",
        )?;
        validate_oci_repository(&oci_repository)?;
        let oci_manifest = string(
            required(descriptor, "oci/manifest")?,
            "locked package :oci/manifest",
        )?;
        validate_digest(&oci_manifest, "oci/manifest")?;
        let namespaces = symbols(
            required(descriptor, "namespaces")?,
            "locked package :namespaces",
        )?;
        if namespaces.is_empty() {
            return Err(format!("locked package {coordinate} exports no namespaces"));
        }
        output.push(LockedPackage {
            coordinate,
            version,
            tap,
            oci_repository,
            oci_manifest,
            archive_sha256,
            namespaces,
        });
    }
    output.sort_by(|left, right| left.coordinate.cmp(&right.coordinate));
    let mut owners = std::collections::BTreeMap::new();
    for package in &output {
        for namespace in &package.namespaces {
            if let Some(previous) = owners.insert(namespace, &package.coordinate) {
                return Err(format!(
                    "package/namespace-conflict: {namespace} is exported by {previous} and {}",
                    package.coordinate
                ));
            }
        }
    }
    Ok(output)
}

fn map<'a>(form: &'a Form, message: &str) -> Result<&'a Vec<(Form, Form)>, String> {
    match form {
        Form::Map(entries) => Ok(entries),
        _ => Err(message.into()),
    }
}

fn lookup<'a>(entries: &'a [(Form, Form)], key: &str) -> Option<&'a Form> {
    entries.iter().find_map(|(candidate, value)| {
        matches!(candidate, Form::Keyword(name) if name == key).then_some(value)
    })
}

fn required<'a>(entries: &'a [(Form, Form)], key: &str) -> Result<&'a Form, String> {
    lookup(entries, key).ok_or_else(|| format!("locked package is missing :{key}"))
}

fn string(form: &Form, label: &str) -> Result<String, String> {
    match form {
        Form::String(value) => Ok(value.clone()),
        _ => Err(format!("{label} must be a string")),
    }
}

fn scalar(form: &Form, label: &str) -> Result<String, String> {
    match form {
        Form::String(value) | Form::Symbol(value) => Ok(value.clone()),
        _ => Err(format!("{label} must be a string or symbol")),
    }
}

fn symbols(form: &Form, label: &str) -> Result<Vec<String>, String> {
    let Form::Vector(values) = form else {
        return Err(format!("{label} must be a vector"));
    };
    let mut output = values
        .iter()
        .map(|value| scalar(value, label))
        .collect::<Result<Vec<_>, _>>()?;
    output.sort();
    output.dedup();
    Ok(output)
}

fn validate_sha256(value: &str) -> Result<(), String> {
    validate_digest(value, "archive-sha256")
}

fn validate_digest(value: &str, label: &str) -> Result<(), String> {
    let value = value.strip_prefix("sha256:").unwrap_or(value);
    if value.len() == 64
        && value
            .chars()
            .all(|value| value.is_ascii_hexdigit() && !value.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(format!("locked package :{label} must be SHA-256"))
    }
}

fn validate_oci_repository(value: &str) -> Result<(), String> {
    let Some(name) = value.strip_prefix("ghcr.io/hara-packages/") else {
        return Err("locked package :oci/repository must be under ghcr.io/hara-packages".into());
    };
    if !name.is_empty()
        && name.chars().all(|value| {
            value.is_ascii_lowercase() || value.is_ascii_digit() || matches!(value, '.' | '_' | '-')
        })
        && name
            .chars()
            .next()
            .is_some_and(|value| value.is_ascii_lowercase() || value.is_ascii_digit())
        && name
            .chars()
            .last()
            .is_some_and(|value| value.is_ascii_lowercase() || value.is_ascii_digit())
    {
        Ok(())
    } else {
        Err("locked package :oci/repository is invalid".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package_descriptor(namespace: &str, digest: char, manifest: char) -> String {
        format!(
            "{{:version \"1.2.3\" :tap \"hara\" :oci/repository \"ghcr.io/hara-packages/hara-lang.demo\" :oci/manifest \"sha256:{}\" :archive-sha256 \"sha256:{}\" :namespaces [{namespace}]}}",
            manifest.to_string().repeat(64),
            digest.to_string().repeat(64),
        )
    }

    #[test]
    fn reads_exact_lock_catalog() {
        let source = format!(
            "{{:lock/format \"0.0.1\" :packages {{\"hara:demo/core\" {}}}}}",
            package_descriptor("demo.core", 'a', 'b')
        );
        let catalog = catalog_from_lock(&source).unwrap();
        assert_eq!(catalog[0].coordinate, "hara:demo/core");
        assert_eq!(catalog[0].namespaces, vec!["demo.core"]);
        assert_eq!(
            catalog[0].oci_repository,
            "ghcr.io/hara-packages/hara-lang.demo"
        );
        assert_eq!(
            catalog[0].oci_manifest,
            format!("sha256:{}", "b".repeat(64))
        );
    }

    #[test]
    fn rejects_namespace_conflicts() {
        let source = format!(
            "{{:lock/format \"0.0.1\" :packages {{\"hara:demo/first\" {} \"hara:demo/second\" {}}}}}",
            package_descriptor("demo.core", 'a', 'b'),
            package_descriptor("demo.core", 'c', 'd'),
        );
        assert!(catalog_from_lock(&source)
            .unwrap_err()
            .contains("package/namespace-conflict"));
    }
}
