use hara_native::kernel::{parse_forms, Form};
use hara_native::vm::{self, BytecodeBundleModule};
use hara_native::Runtime;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const SOURCE_ROOT_ENV: &str = "HARA_SOURCE_ROOT";
const LEGACY_SOURCE_ROOT_ENV: &str = "HARA_ROOT";
const FOUNDATION_BOOTSTRAP_NAMESPACES: &[&str] = &[
    "std.foundation",
    "std.foundation.bytes",
    "std.foundation.coroutine",
    "std.foundation.pretty",
    "std.foundation.promise",
    "std.foundation.string",
];

/// Creates a compiler runtime with Foundation from the configured Hara source
/// checkout. This is a build-side operation: the production server receives
/// HBX0 bytecode and does not call this function.
pub fn compiler_runtime() -> Result<Runtime, String> {
    compiler_runtime_at(&configured_source_root()?)
}

/// Creates a compiler runtime from one checked-out Hara source tree.
///
/// Source is read only into this compiler process. No source body or source
/// path is retained in the HAB0/HBX0 artifact that Hoplite serves.
pub fn compiler_runtime_at(source_root: &Path) -> Result<Runtime, String> {
    let library_root = standard_library_root(source_root)?;
    let sources = namespace_sources(&library_root.join("std"))?;
    let mut runtime = Runtime::new();
    for (namespace, source) in &sources {
        runtime.register_resource(namespace, source);
    }
    runtime
        .require_resource("std.foundation")
        .map_err(|error| format!("cannot bootstrap Hara Foundation: {error:?}"))?;
    for namespace in sources
        .keys()
        .filter(|name| name.starts_with("std.foundation."))
    {
        runtime.require_resource(namespace).map_err(|error| {
            format!("cannot bootstrap Hara Foundation namespace {namespace}: {error:?}")
        })?;
    }
    Ok(runtime)
}

/// Returns the complete standard-library source set in stable namespace order.
/// The Hoplite compiler emits these modules as HBX0 alongside application code,
/// allowing the production server to load the result without a source checkout.
pub fn standard_library_sources() -> Result<Vec<(String, String)>, String> {
    let source_root = configured_source_root()?;
    let library_root = standard_library_root(&source_root)?;
    Ok(namespace_sources(&library_root.join("std"))?
        .into_iter()
        .collect())
}

/// Reads one standard-library module from the reviewed Hara source checkout.
pub fn standard_library_source(namespace: &str) -> Result<String, String> {
    standard_library_sources()?
        .into_iter()
        .find_map(|(candidate, source)| (candidate == namespace).then_some(source))
        .ok_or_else(|| format!("Hara standard library does not provide namespace {namespace}"))
}

/// Compiles the Foundation bootstrap family from reviewed Hara source into an
/// eager source-free HBX0 bundle for embedding hosts.
pub fn foundation_bytecode_bundle() -> Result<Vec<u8>, String> {
    let sources = standard_library_sources()?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let mut compiler = compiler_runtime()?;
    let mut modules = Vec::with_capacity(FOUNDATION_BOOTSTRAP_NAMESPACES.len());

    for &namespace in FOUNDATION_BOOTSTRAP_NAMESPACES {
        let source = sources.get(namespace).ok_or_else(|| {
            format!("Hara standard library is missing Foundation module {namespace}")
        })?;
        let forms = parse_forms(source)?;
        let declaration = forms
            .iter()
            .find(|form| namespace_form(form))
            .ok_or_else(|| format!("Foundation module {namespace} has no ns declaration"))?;
        let declaration = render_form(declaration);
        compiler
            .eval_native(&declaration)
            .map_err(|error| format!("{namespace}: cannot load namespace: {error}"))?;
        let body = forms
            .into_iter()
            .filter(|form| !namespace_form(form) && !macro_definition(form))
            .map(|form| render_form(&form))
            .collect::<Vec<_>>()
            .join("\n");
        let artifact = compiler
            .compile_bytecode_artifact(&body)
            .map_err(|error| format!("{namespace}: {error}"))?;
        compiler
            .eval_bytecode_artifact(&artifact)
            .map_err(|error| format!("{namespace}: cannot load bytecode: {error}"))?;
        modules.push(BytecodeBundleModule {
            resource: namespace.to_owned(),
            namespace_form: declaration,
            source_digest: Sha256::digest(source.as_bytes()).into(),
            dependencies: (namespace != "std.foundation")
                .then(|| vec!["std.foundation".to_owned()])
                .unwrap_or_default(),
            eager: true,
            artifact,
        });
    }
    vm::encode_bytecode_bundle(&modules)
}

/// Resolves the Hara source checkout used for Hoplite source compilation.
pub fn configured_source_root() -> Result<PathBuf, String> {
    let configured = env::var_os(SOURCE_ROOT_ENV)
        .or_else(|| env::var_os(LEGACY_SOURCE_ROOT_ENV))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../hara"));
    configured.canonicalize().map_err(|error| {
        format!(
            "cannot resolve Hara source checkout {}: {error}; set {SOURCE_ROOT_ENV} to the reviewed Hara checkout",
            configured.display()
        )
    })
}

fn standard_library_root(source_root: &Path) -> Result<PathBuf, String> {
    for candidate in [
        source_root.join("src"),
        source_root.join("core/rust/hal-src"),
        source_root.join("core/lib/src"),
    ] {
        if candidate.join("std/foundation.hal").is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "Hara source checkout {} has no standard-library source; expected src, core/rust/hal-src, or core/lib/src with std/foundation.hal",
        source_root.display()
    ))
}

fn namespace_sources(root: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut files = Vec::new();
    collect_hal_files(root, &mut files)?;
    files.sort();

    let mut sources = BTreeMap::new();
    for path in files {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read Hara source {}: {error}", path.display()))?;
        let namespace = declared_namespace(&source).ok_or_else(|| {
            format!(
                "Hara source {} does not declare an ns or ns+ namespace",
                path.display()
            )
        })?;
        let replacement_len = source.len();
        if let Some(previous) = sources.insert(namespace.clone(), source) {
            return Err(format!(
                "Hara source checkout declares namespace {namespace} more than once ({} bytes and {replacement_len} bytes)",
                previous.len()
            ));
        }
    }
    Ok(sources)
}

fn collect_hal_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(root).map_err(|error| {
        format!(
            "cannot read Hara source directory {}: {error}",
            root.display()
        )
    })? {
        let path = entry
            .map_err(|error| {
                format!(
                    "cannot inspect Hara source directory {}: {error}",
                    root.display()
                )
            })?
            .path();
        if path.is_dir() {
            collect_hal_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "hal") {
            files.push(path);
        }
    }
    Ok(())
}

fn declared_namespace(source: &str) -> Option<String> {
    parse_forms(source).ok()?.into_iter().find_map(|form| match form {
        Form::List(items)
            if matches!(items.first(), Some(Form::Symbol(operator)) if operator == "ns" || operator == "ns+") =>
        {
            match items.get(1) {
                Some(Form::Symbol(namespace)) => Some(namespace.clone()),
                _ => None,
            }
        }
        _ => None,
    })
}

fn namespace_form(form: &Form) -> bool {
    matches!(form,
        Form::List(items)
            if matches!(items.first(), Some(Form::Symbol(operator)) if operator == "ns" || operator == "ns+"))
}

fn macro_definition(form: &Form) -> bool {
    matches!(form,
        Form::List(items)
            if matches!(items.first(), Some(Form::Symbol(operator)) if operator == "defmacro" || operator == "defmacro-"))
}

fn render_form(form: &Form) -> String {
    match form {
        Form::Metadata(metadata, value) => {
            format!("^{} {}", render_form(metadata), render_form(value))
        }
        Form::Tagged(tag, value) => format!("#{tag}{}", render_form(value)),
        Form::List(values) => render_sequence(values, "(", ")"),
        Form::Vector(values) => render_sequence(values, "[", "]"),
        Form::Set(values) => render_sequence(values, "#{", "}"),
        Form::Map(entries) => {
            let values = entries
                .iter()
                .flat_map(|(key, value)| [render_form(key), render_form(value)])
                .collect::<Vec<_>>();
            format!("{{{}}}", values.join(" "))
        }
        _ => form.to_string(),
    }
}

fn render_sequence(values: &[Form], prefix: &str, suffix: &str) -> String {
    format!(
        "{prefix}{}{suffix}",
        values.iter().map(render_form).collect::<Vec<_>>().join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_source_checkout_has_a_foundation_root() {
        let source_root = configured_source_root().unwrap();
        let library_root = standard_library_root(&source_root).unwrap();
        assert!(library_root.join("std/foundation.hal").is_file());
    }

    #[test]
    fn source_namespace_is_read_from_the_declaration() {
        assert_eq!(
            declared_namespace("(ns+ sample.source)\n(def value 1)"),
            Some("sample.source".into())
        );
        assert_eq!(declared_namespace("(def value 1)"), None);
    }

    #[test]
    fn standard_library_inventory_contains_foundation_and_string_support() {
        let sources = standard_library_sources().unwrap();
        let names = sources
            .iter()
            .map(|(namespace, _)| namespace.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"std.foundation"));
        assert!(names.contains(&"std.foundation.string"));
    }

    #[test]
    fn foundation_bundle_loads_without_a_source_provider() {
        let bundle = foundation_bytecode_bundle().unwrap();
        assert_eq!(&bundle[..4], b"HBX0");

        let mut runtime = Runtime::new();
        vm::eval_bytecode_bundle(&mut runtime, &bundle).unwrap();
        assert_eq!(
            runtime
                .eval_native_value("(get {:answer 42} :answer)")
                .unwrap(),
            hara_native::core::Value::Number(42)
        );

        let namespaces = hara_native::embedding_namespace_registry();
        let protocols = hara_native::core::ProtocolRegistry::core();
        vm::eval_eager_bytecode_bundle_with_registries(&namespaces, &protocols, &bundle).unwrap();
    }
}
