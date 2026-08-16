use hara_wasm::kernel::{parse, Form};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackage {
    pub coordinate: String,
    pub version: String,
    pub tap: String,
    pub registry_commit: String,
    pub identity_revision: String,
    pub archive_sha256: String,
    pub namespaces: Vec<String>,
}

pub fn catalog_from_lock(source: &str) -> Result<Vec<LockedPackage>, String> {
    let document = parse(source)?;
    let root = map(&document, "project.lock.edn must be an EDN map")?;
    if !matches!(lookup(root, "lock/format"), Some(Form::String(version)) if version == "0.0.0-alpha")
    {
        return Err("project.lock.edn requires :lock/format \"0.0.0-alpha\"".into());
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
        let registry_commit = string(
            required(descriptor, "registry-commit")?,
            "locked package :registry-commit",
        )?;
        validate_commit(&registry_commit, "registry-commit")?;
        let identity_revision = string(
            required(descriptor, "identity-revision")?,
            "locked package :identity-revision",
        )?;
        validate_commit(&identity_revision, "identity-revision")?;
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
            registry_commit,
            identity_revision,
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
    let value = value.strip_prefix("sha256:").unwrap_or(value);
    if value.len() == 64 && value.chars().all(|value| value.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("locked package :archive-sha256 must be SHA-256".into())
    }
}

fn validate_commit(value: &str, label: &str) -> Result<(), String> {
    if value.len() == 40
        && value
            .chars()
            .all(|value| value.is_ascii_hexdigit() && !value.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(format!(
            "locked package :{label} must be a lowercase 40-character Git commit"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package_descriptor(namespace: &str, digest: char, commit: char) -> String {
        format!(
            "{{:version \"1.2.3\" :tap \"hara\" :registry-commit \"{}\" :identity-revision \"{}\" :archive-sha256 \"sha256:{}\" :namespaces [{namespace}]}}",
            commit.to_string().repeat(40),
            commit.to_string().repeat(40),
            digest.to_string().repeat(64),
        )
    }

    #[test]
    fn reads_exact_lock_catalog() {
        let source = format!(
            "{{:lock/format \"0.0.0-alpha\" :packages {{\"hara:demo/core\" {}}}}}",
            package_descriptor("demo.core", 'a', 'b')
        );
        let catalog = catalog_from_lock(&source).unwrap();
        assert_eq!(catalog[0].coordinate, "hara:demo/core");
        assert_eq!(catalog[0].namespaces, vec!["demo.core"]);
    }

    #[test]
    fn rejects_namespace_conflicts() {
        let source = format!(
            "{{:lock/format \"0.0.0-alpha\" :packages {{\"hara:demo/first\" {} \"hara:demo/second\" {}}}}}",
            package_descriptor("demo.core", 'a', 'b'),
            package_descriptor("demo.core", 'c', 'd'),
        );
        assert!(catalog_from_lock(&source)
            .unwrap_err()
            .contains("package/namespace-conflict"));
    }
}
