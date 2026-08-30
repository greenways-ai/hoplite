use crate::app;
use crate::platform::ResolvedModule;
use hara_native::project::Project;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub struct ImportProjection {
    root: PathBuf,
    source_paths: Vec<PathBuf>,
}

impl Drop for ImportProjection {
    fn drop(&mut self) {
        if self.root.exists() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

pub fn augment_project(
    project: &Project,
    modules: &[ResolvedModule],
) -> Result<(Project, ImportProjection), String> {
    let projection = materialize(modules)?;
    let mut augmented = project.clone();
    augmented
        .source_paths
        .extend(projection.source_paths.iter().cloned());
    Ok((augmented, projection))
}

fn materialize(modules: &[ResolvedModule]) -> Result<ImportProjection, String> {
    let root = std::env::temp_dir().join(format!(
        "hoplite-imports-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let source_root = root.join("src");
    let mut selected = BTreeMap::new();
    for module in modules {
        for source in &module.export.sources {
            if let Some(embedded) = embedded_source(&source.namespace) {
                if embedded != source.source.as_str() {
                    return Err(format!(
                        "module {} export :{} cannot replace embedded namespace {}",
                        module.export.coordinate, module.export.export, source.namespace
                    ));
                }
                continue;
            }
            if let Some(current) = selected.insert(source.namespace.clone(), source.source.clone())
            {
                if current != source.source {
                    return Err(format!(
                        "selected Hoplite modules provide conflicting namespace {}",
                        source.namespace
                    ));
                }
            }
        }
    }

    if selected.is_empty() {
        return Ok(ImportProjection {
            root,
            source_paths: Vec::new(),
        });
    }

    let result = (|| {
        for (namespace, source) in selected {
            let relative = namespace_path(&namespace)?;
            let target = source_root.join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(io)?;
            }
            fs::write(&target, source).map_err(io)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&root);
        return Err(error);
    }
    Ok(ImportProjection {
        root,
        source_paths: vec![source_root],
    })
}

fn embedded_source(namespace: &str) -> Option<&'static str> {
    match namespace {
        "hoplite.core" => Some(app::CORE_SOURCE),
        "hoplite.host" => Some(app::HOST_SOURCE),
        "hoplite.internal" => Some(app::INTERNAL_SOURCE),
        "hoplite.raw" => Some(app::RAW_SOURCE),
        "hoplite.response-source" => Some(app::RESPONSE_SOURCE),
        "hoplite.rtc" => Some(app::RTC_SOURCE),
        _ => None,
    }
}

fn namespace_path(namespace: &str) -> Result<PathBuf, String> {
    if namespace.is_empty() || namespace.contains('/') || namespace.chars().any(char::is_whitespace)
    {
        return Err(format!("invalid imported namespace {namespace:?}"));
    }
    let relative = format!("{}.hal", namespace.replace('.', "/").replace('-', "_"));
    let path = Path::new(&relative);
    if path.is_absolute() {
        return Err(format!("invalid imported namespace {namespace:?}"));
    }
    Ok(path.to_path_buf())
}

fn io(error: std::io::Error) -> String {
    format!("package import I/O error: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package_export::{ResolvedExport, ResolvedSource};

    fn module(namespace: &str, source: &str) -> ResolvedModule {
        ResolvedModule {
            alias: "feature".into(),
            export: ResolvedExport {
                coordinate: "gh:example:modules".into(),
                version: "1.0.0".into(),
                archive_sha256:
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                export: "example/feature".into(),
                namespace: namespace.into(),
                sources: vec![ResolvedSource {
                    namespace: namespace.into(),
                    path: PathBuf::from("fixture.hal"),
                    source: source.into(),
                    dependencies: Vec::new(),
                }],
            },
        }
    }

    #[test]
    fn materializes_selected_non_embedded_namespaces() {
        let projection = materialize(&[module(
            "example.feature",
            "(ns example.feature)\n(defn value [] 42)\n",
        )])
        .unwrap();
        assert_eq!(projection.source_paths.len(), 1);
        assert!(projection.source_paths[0]
            .join("example/feature.hal")
            .is_file());
    }

    #[test]
    fn refuses_to_replace_an_embedded_namespace() {
        let error = materialize(&[module("hoplite.core", "(ns hoplite.core)\n")]).unwrap_err();
        assert!(
            error.contains("cannot replace embedded namespace"),
            "{error}"
        );
    }
}
