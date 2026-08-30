use crate::package_catalog::LockedPackage;
use hara_native::kernel::{parse, parse_forms, Form};
use hara_native::project;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedSource {
    pub namespace: String,
    pub path: PathBuf,
    pub source: String,
    pub dependencies: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedExport {
    pub coordinate: String,
    pub version: String,
    pub archive_sha256: String,
    pub export: String,
    pub namespace: String,
    pub sources: Vec<ResolvedSource>,
}

pub fn resolve_locked_export(
    package: &LockedPackage,
    export: &str,
) -> Result<ResolvedExport, String> {
    let root = crate::package::ensure_locked(package)?;
    let resolved = resolve_export_root(
        &root,
        &package.coordinate,
        &package.version,
        &package.archive_sha256,
        export,
    )?;
    if !package.namespaces.contains(&resolved.namespace) {
        return Err(format!(
            "locked package {} does not export namespace {}",
            package.coordinate, resolved.namespace
        ));
    }
    Ok(resolved)
}

fn resolve_export_root(
    root: &Path,
    coordinate: &str,
    version: &str,
    archive_sha256: &str,
    export: &str,
) -> Result<ResolvedExport, String> {
    let namespace = export_namespace(root, export)?;

    let project = project::read(root)?;
    let mut sources = BTreeMap::new();
    for path in project::files_in(&project.root, &project.source_paths)? {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let (declared, dependencies) =
            source_declaration(&source).map_err(|error| format!("{}: {error}", path.display()))?;
        let resolved = ResolvedSource {
            namespace: declared.clone(),
            path: path.clone(),
            source,
            dependencies,
        };
        if sources.insert(declared.clone(), resolved).is_some() {
            return Err(format!(
                "package {} {} contains duplicate namespace {}",
                coordinate, version, declared
            ));
        }
    }
    if !sources.contains_key(&namespace) {
        return Err(format!(
            "package {} {} export :{} points to missing namespace {}",
            coordinate, version, export, namespace
        ));
    }

    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut ordered = Vec::new();
    visit_source(
        &namespace,
        &sources,
        &mut visiting,
        &mut visited,
        &mut ordered,
    )?;

    Ok(ResolvedExport {
        coordinate: coordinate.to_owned(),
        version: version.to_owned(),
        archive_sha256: format!(
            "sha256:{}",
            archive_sha256
                .strip_prefix("sha256:")
                .unwrap_or(archive_sha256)
        ),
        export: export.to_owned(),
        namespace,
        sources: ordered,
    })
}

fn export_namespace(root: &Path, export: &str) -> Result<String, String> {
    let path = root.join("project.edn");
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let manifest = parse(&source).map_err(|error| format!("{}: {error}", path.display()))?;
    let project = form_map(&manifest, "package project.edn must be an EDN map")?;
    let exports = lookup(project, "hoplite/exports")
        .ok_or_else(|| format!("{} does not declare :hoplite/exports", path.display()))?;
    let exports = form_map(exports, ":hoplite/exports must be an EDN map")?;
    let descriptor = exports
        .iter()
        .find_map(|(key, value)| (identifier(key).as_deref() == Some(export)).then_some(value))
        .ok_or_else(|| format!("package does not export :{export}"))?;
    let descriptor = form_map(descriptor, "Hoplite export descriptor must be an EDN map")?;
    let namespace = lookup(descriptor, "export/namespace")
        .and_then(identifier)
        .ok_or_else(|| format!("Hoplite export :{export} requires :export/namespace"))?;
    if namespace.is_empty() || namespace.contains('/') || namespace.chars().any(char::is_whitespace)
    {
        return Err(format!(
            "Hoplite export :{export} has invalid namespace {namespace:?}"
        ));
    }
    Ok(namespace)
}

fn source_declaration(source: &str) -> Result<(String, Vec<String>), String> {
    let forms = parse_forms(source)?;
    let declaration = forms
        .iter()
        .find_map(|form| match form {
            Form::List(values)
                if matches!(values.first(), Some(Form::Symbol(head)) if head == "ns" || head == "ns+") =>
            {
                Some(values)
            }
            _ => None,
        })
        .ok_or("HAL package source is missing ns form")?;
    let namespace = match declaration.get(1) {
        Some(Form::Symbol(namespace)) if !namespace.contains('/') => namespace.clone(),
        _ => return Err("HAL package namespace must be an unqualified symbol".into()),
    };
    let mut dependencies = Vec::new();
    for clause in &declaration[2..] {
        let Form::List(values) = clause else {
            continue;
        };
        if !matches!(values.first(), Some(Form::Keyword(keyword)) if keyword == "require") {
            continue;
        }
        for entry in &values[1..] {
            if let Form::Vector(require) = entry {
                if let Some(Form::Symbol(dependency)) = require.first() {
                    dependencies.push(dependency.clone());
                }
            }
        }
    }
    dependencies.sort();
    dependencies.dedup();
    Ok((namespace, dependencies))
}

fn visit_source(
    namespace: &str,
    sources: &BTreeMap<String, ResolvedSource>,
    visiting: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
    ordered: &mut Vec<ResolvedSource>,
) -> Result<(), String> {
    if visited.contains(namespace) {
        return Ok(());
    }
    if !visiting.insert(namespace.to_owned()) {
        return Err(format!(
            "package export namespace cycle includes {namespace}"
        ));
    }
    let source = sources
        .get(namespace)
        .ok_or_else(|| format!("missing package namespace {namespace}"))?;
    for dependency in &source.dependencies {
        if sources.contains_key(dependency) {
            visit_source(dependency, sources, visiting, visited, ordered)?;
        }
    }
    visiting.remove(namespace);
    visited.insert(namespace.to_owned());
    ordered.push(source.clone());
    Ok(())
}

fn form_map<'a>(form: &'a Form, message: &str) -> Result<&'a [(Form, Form)], String> {
    match form {
        Form::Map(entries) => Ok(entries),
        _ => Err(message.into()),
    }
}

fn lookup<'a>(entries: &'a [(Form, Form)], key: &str) -> Option<&'a Form> {
    entries.iter().find_map(|(candidate, value)| {
        (identifier(candidate).as_deref() == Some(key)).then_some(value)
    })
}

fn identifier(form: &Form) -> Option<String> {
    match form {
        Form::Keyword(value) | Form::Symbol(value) | Form::String(value) => Some(value.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "hoplite-package-export-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("src/example")).unwrap();
        fs::write(
            root.join("project.edn"),
            "{:hara/type :project\n :hara/version \"1.0.0\"\n :project/id \"gh:example:modules\"\n :project/version \"1.2.3\"\n :project/source-paths [\"src\"]\n :project/test-paths []\n :project/extension-paths []\n :project/capabilities #{}\n :project/dependencies {}\n :hoplite/exports {:example/feature {:export/namespace example.feature}}}\n",
        )
        .unwrap();
        fs::write(
            root.join("src/example/base.hal"),
            "(ns example.base)\n(defn value [] 42)\n",
        )
        .unwrap();
        fs::write(
            root.join("src/example/feature.hal"),
            "(ns example.feature (:require [example.base :as base]))\n(defn value [] (base/value))\n",
        )
        .unwrap();
        fs::write(
            root.join("src/example/unrelated.hal"),
            "(ns example.unrelated)\n",
        )
        .unwrap();
        root
    }

    #[test]
    fn resolves_only_the_selected_namespace_closure() {
        let root = fixture();
        let resolved = resolve_export_root(
            &root,
            "gh:example:modules",
            "1.2.3",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "example/feature",
        )
        .unwrap();
        assert_eq!(resolved.namespace, "example.feature");
        assert_eq!(
            resolved
                .sources
                .iter()
                .map(|source| source.namespace.as_str())
                .collect::<Vec<_>>(),
            ["example.base", "example.feature"]
        );
        assert_eq!(
            resolved.archive_sha256,
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_exports_missing_from_the_package_manifest() {
        let root = fixture();
        let error = resolve_export_root(
            &root,
            "gh:example:modules",
            "1.2.3",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "example/missing",
        )
        .unwrap_err();
        assert!(error.contains("does not export"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }
}
