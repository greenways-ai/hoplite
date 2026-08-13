use hara_wasm::core::{self, Value};
use hara_wasm::hta;
use hoplite_application_bundle as application_bundle;
use serde_json::{json, Value as JsonValue};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub const FORMAT: &str = "hoplite.inspect/0-alpha";
const MANIFEST_FORMAT: i64 = 2;
const MAX_INSPECTED_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
const MAX_REPORTED_SOURCE_INPUTS: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Artifact {
    present: bool,
    bytes: Option<usize>,
    sha256: Option<String>,
}

impl Artifact {
    fn absent() -> Self {
        Self {
            present: false,
            bytes: None,
            sha256: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ManifestSummary {
    format: i64,
    applications: usize,
    routes: usize,
    adapters: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceInputs {
    count: usize,
    examples: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct InspectionPaths {
    output: String,
    bundle: String,
    manifest: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Inspection {
    bundle_bytes: usize,
    bundle_sha256: String,
    manifest_bytes: usize,
    manifest_sha256: String,
    bytecode_bytes: usize,
    manifest: ManifestSummary,
    nginx_configuration: Artifact,
    platform_hta: Artifact,
    platform_edn: Artifact,
    openapi_documents: usize,
    source_inputs: SourceInputs,
    paths: Option<InspectionPaths>,
}

struct ResolvedPaths {
    output: PathBuf,
    bundle: PathBuf,
    manifest: PathBuf,
}

pub fn run(arguments: &[String]) -> Result<(), String> {
    if matches!(
        arguments.first().map(String::as_str),
        Some("--help" | "-h" | "help")
    ) {
        usage();
        return Ok(());
    }

    let mut target = None;
    let mut manifest_override = None;
    let mut json_output = false;
    let mut show_paths = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--json" => json_output = true,
            "--show-paths" => show_paths = true,
            "--manifest" => {
                index += 1;
                manifest_override =
                    Some(PathBuf::from(arguments.get(index).ok_or(
                        "hoplite/inspect-argument-invalid: --manifest requires a file",
                    )?));
            }
            value if value.starts_with('-') => {
                return Err(format!(
                    "hoplite/inspect-argument-invalid: unknown option {value:?}"
                ))
            }
            value if target.is_none() => target = Some(PathBuf::from(value)),
            value => {
                return Err(format!(
                    "hoplite/inspect-argument-invalid: unexpected target {value:?}"
                ))
            }
        }
        index += 1;
    }

    let target = target.unwrap_or(std::env::current_dir().map_err(|error| {
        format!(
            "hoplite/inspect-current-directory-unavailable: {}",
            io_kind(&error)
        )
    })?);
    let inspection = inspect_target(&target, manifest_override.as_deref(), show_paths)?;
    if json_output {
        print!("{}", render_json(&inspection));
    } else {
        print!("{}", render_human(&inspection));
    }
    Ok(())
}

fn usage() {
    println!("Inspect generated Hoplite application output without executing source");
    println!();
    println!(
        "usage: hoplite inspect [--json] [--show-paths] [--manifest FILE] [PROJECT|OUTPUT|BUNDLE]"
    );
    println!();
    println!("Paths are redacted unless --show-paths is supplied.");
}

fn inspect_target(
    target: &Path,
    manifest_override: Option<&Path>,
    show_paths: bool,
) -> Result<Inspection, String> {
    let paths = resolve_paths(target, manifest_override);
    let bundle =
        application_bundle::read_bundle_file(&paths.bundle).map_err(redacted_bundle_file_error)?;
    let manifest = application_bundle::read_manifest_file(&paths.manifest)
        .map_err(redacted_bundle_file_error)?;
    let decoded =
        application_bundle::decode(&bundle, &manifest).map_err(|error| error.to_string())?;
    let manifest_value = hta::decode(&manifest)
        .map_err(|error| format!("hoplite/inspect-manifest-invalid: {error}"))?;
    let manifest_summary = summarize_manifest(&manifest_value)?;

    let paths_output = show_paths.then(|| InspectionPaths {
        output: paths.output.display().to_string(),
        bundle: paths.bundle.display().to_string(),
        manifest: paths.manifest.display().to_string(),
    });
    Ok(Inspection {
        bundle_bytes: bundle.len(),
        bundle_sha256: digest_hex(&bundle),
        manifest_bytes: manifest.len(),
        manifest_sha256: lower_hex(&decoded.manifest_digest()),
        bytecode_bytes: decoded.bytecode().len(),
        manifest: manifest_summary,
        nginx_configuration: inspect_optional_artifact(
            &paths.output.join("conf/nginx.conf"),
            "nginx-configuration",
        )?,
        platform_hta: inspect_optional_artifact(
            &paths.output.join("platform.hta"),
            "platform-hta",
        )?,
        platform_edn: inspect_optional_artifact(
            &paths.output.join("platform.edn"),
            "platform-edn",
        )?,
        openapi_documents: count_openapi_documents(&paths.output.join("openapi"))?,
        source_inputs: inspect_source_inputs(&paths.output)?,
        paths: paths_output,
    })
}

fn resolve_paths(target: &Path, manifest_override: Option<&Path>) -> ResolvedPaths {
    let explicit_bundle = target.is_file()
        || target
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("hbx"));
    let (output, bundle, default_manifest) = if explicit_bundle {
        let output = target
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        (
            output.clone(),
            target.to_path_buf(),
            output.join("apps.hta"),
        )
    } else {
        let is_output = target
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value == ".hoplite")
            || target.join("app.hbx").is_file()
            || target.join("apps.hta").is_file();
        let output = if is_output {
            target.to_path_buf()
        } else {
            target.join(".hoplite")
        };
        (
            output.clone(),
            output.join("app.hbx"),
            output.join("apps.hta"),
        )
    };
    ResolvedPaths {
        output,
        bundle,
        manifest: manifest_override
            .map(Path::to_path_buf)
            .unwrap_or(default_manifest),
    }
}

fn summarize_manifest(value: &Value) -> Result<ManifestSummary, String> {
    let format = number_field(value, "format").ok_or("hoplite/inspect-manifest-format-missing")?;
    if format != MANIFEST_FORMAT {
        return Err(format!(
            "hoplite/inspect-manifest-incompatible: format {format}, expected {MANIFEST_FORMAT}"
        ));
    }
    let applications =
        sequence_field(value, "apps").ok_or("hoplite/inspect-manifest-apps-missing")?;
    if applications.is_empty() {
        return Err("hoplite/inspect-manifest-apps-empty".into());
    }

    let mut routes = 0;
    let mut adapters = BTreeMap::new();
    for application in &applications {
        let application_routes = sequence_field(application, "routes")
            .ok_or("hoplite/inspect-manifest-routes-missing")?;
        for route in application_routes {
            let adapter = keyword_field(&route, "adapter")
                .ok_or("hoplite/inspect-manifest-adapter-missing")?;
            if !matches!(adapter.as_str(), "raw" | "request" | "request+hta") {
                return Err(format!(
                    "hoplite/inspect-manifest-adapter-unsupported: {adapter}"
                ));
            }
            routes += 1;
            *adapters.entry(adapter).or_insert(0) += 1;
        }
    }
    Ok(ManifestSummary {
        format,
        applications: applications.len(),
        routes,
        adapters,
    })
}

fn field(value: &Value, name: &str) -> Option<Value> {
    core::map_entries(value)?
        .into_iter()
        .find_map(|(key, value)| {
            matches!(&key, Value::Keyword(keyword) if keyword.as_str() == name).then_some(value)
        })
}

fn number_field(value: &Value, name: &str) -> Option<i64> {
    match field(value, name)? {
        Value::Number(value) => Some(value),
        _ => None,
    }
}

fn keyword_field(value: &Value, name: &str) -> Option<String> {
    match field(value, name)? {
        Value::Keyword(value) => Some(value.as_str().to_owned()),
        _ => None,
    }
}

fn sequence_field(value: &Value, name: &str) -> Option<Vec<Value>> {
    match field(value, name)? {
        Value::Vector(values) => Some(values.iter().cloned().collect()),
        Value::List(values) => Some(values.iter().cloned().collect()),
        Value::Tuple(values) => Some(values.iter().cloned().collect()),
        _ => None,
    }
}

fn inspect_optional_artifact(path: &Path, label: &'static str) -> Result<Artifact, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Artifact::absent()),
        Err(error) => return Err(format!("hoplite/inspect-{label}-open: {}", io_kind(&error))),
    };
    let metadata = file
        .metadata()
        .map_err(|error| format!("hoplite/inspect-{label}-metadata: {}", io_kind(&error)))?;
    if !metadata.is_file() {
        return Err(format!("hoplite/inspect-{label}-not-regular"));
    }
    if metadata.len() > MAX_INSPECTED_ARTIFACT_BYTES as u64 {
        return Err(format!(
            "hoplite/inspect-{label}-too-large: {} bytes exceeds {}",
            metadata.len(),
            MAX_INSPECTED_ARTIFACT_BYTES
        ));
    }

    let expected = metadata.len();
    let mut bytes = Vec::with_capacity(expected as usize);
    file.take(MAX_INSPECTED_ARTIFACT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("hoplite/inspect-{label}-read: {}", io_kind(&error)))?;
    if bytes.len() > MAX_INSPECTED_ARTIFACT_BYTES {
        return Err(format!(
            "hoplite/inspect-{label}-too-large: {} bytes exceeds {}",
            bytes.len(),
            MAX_INSPECTED_ARTIFACT_BYTES
        ));
    }
    if bytes.len() as u64 != expected {
        return Err(format!(
            "hoplite/inspect-{label}-changed: expected {expected} bytes, read {}",
            bytes.len()
        ));
    }
    Ok(Artifact {
        present: true,
        bytes: Some(bytes.len()),
        sha256: Some(digest_hex(&bytes)),
    })
}

fn count_openapi_documents(path: &Path) -> Result<usize, String> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(format!(
                "hoplite/inspect-openapi-directory-read: {}",
                io_kind(&error)
            ))
        }
    };
    let mut count = 0;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "hoplite/inspect-openapi-directory-entry: {}",
                io_kind(&error)
            )
        })?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("hoplite/inspect-openapi-file-type: {}", io_kind(&error)))?;
        if file_type.is_file()
            && entry.path().extension().and_then(|value| value.to_str()) == Some("json")
        {
            count += 1;
        }
    }
    Ok(count)
}

fn inspect_source_inputs(root: &Path) -> Result<SourceInputs, String> {
    let mut result = SourceInputs {
        count: 0,
        examples: Vec::new(),
    };
    if !root.is_dir() {
        return Ok(result);
    }
    collect_source_inputs(root, root, &mut result)?;
    Ok(result)
}

fn collect_source_inputs(
    root: &Path,
    directory: &Path,
    result: &mut SourceInputs,
) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("hoplite/inspect-output-directory-read: {}", io_kind(&error)))?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "hoplite/inspect-output-directory-entry: {}",
                io_kind(&error)
            )
        })?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("hoplite/inspect-output-file-type: {}", io_kind(&error)))?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_source_inputs(root, &path, result)?;
            continue;
        }
        if !file_type.is_file() || !is_source_input(&path) {
            continue;
        }

        result.count += 1;
        if result.examples.len() < MAX_REPORTED_SOURCE_INPUTS {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            result.examples.push(relative);
        }
    }
    Ok(())
}

fn is_source_input(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some("hal")
        || matches!(
            path.file_name().and_then(|value| value.to_str()),
            Some("project.edn" | "hara.extension.edn")
        )
}

fn redacted_bundle_file_error(error: application_bundle::FileError) -> String {
    match error {
        application_bundle::FileError::Io {
            class,
            operation,
            source,
            ..
        } => format!("hoplite/{class}-file-{operation}: {}", io_kind(&source)),
        application_bundle::FileError::NotRegular { class, .. } => {
            format!("hoplite/{class}-file-not-regular")
        }
        application_bundle::FileError::TooLarge {
            class,
            actual,
            maximum,
            ..
        } => format!("hoplite/{class}-file-too-large: {actual} bytes exceeds {maximum}"),
        application_bundle::FileError::Changed {
            class,
            expected,
            actual,
            ..
        } => format!("hoplite/{class}-file-changed: expected {expected} bytes, read {actual}"),
    }
}

fn io_kind(error: &io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::NotFound => "not-found",
        io::ErrorKind::PermissionDenied => "permission-denied",
        io::ErrorKind::AlreadyExists => "already-exists",
        io::ErrorKind::InvalidInput => "invalid-input",
        io::ErrorKind::InvalidData => "invalid-data",
        io::ErrorKind::TimedOut => "timed-out",
        io::ErrorKind::Interrupted => "interrupted",
        io::ErrorKind::UnexpectedEof => "unexpected-eof",
        _ => "io-error",
    }
}

fn digest_hex(bytes: &[u8]) -> String {
    lower_hex(&Sha256::digest(bytes))
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn artifact_json(artifact: &Artifact) -> JsonValue {
    json!({
        "present": artifact.present,
        "bytes": artifact.bytes,
        "sha256": artifact.sha256,
    })
}

fn render_json(inspection: &Inspection) -> String {
    let paths = inspection.paths.as_ref().map_or(JsonValue::Null, |paths| {
        json!({
            "output": paths.output,
            "bundle": paths.bundle,
            "manifest": paths.manifest,
        })
    });
    let document = json!({
        "format": FORMAT,
        "application_bundle": {
            "format": application_bundle::FORMAT,
            "runtime_abi": application_bundle::RUNTIME_ABI_VERSION,
            "bytes": inspection.bundle_bytes,
            "sha256": inspection.bundle_sha256,
            "embedded_hbx0_bytes": inspection.bytecode_bytes,
        },
        "manifest": {
            "format": inspection.manifest.format,
            "bytes": inspection.manifest_bytes,
            "sha256": inspection.manifest_sha256,
            "applications": inspection.manifest.applications,
            "routes": inspection.manifest.routes,
            "adapters": inspection.manifest.adapters,
        },
        "artifacts": {
            "nginx_configuration": artifact_json(&inspection.nginx_configuration),
            "platform_hta": artifact_json(&inspection.platform_hta),
            "platform_edn": artifact_json(&inspection.platform_edn),
            "openapi_documents": inspection.openapi_documents,
        },
        "source_free": inspection.source_inputs.count == 0,
        "source_inputs": {
            "count": inspection.source_inputs.count,
            "examples": inspection.source_inputs.examples,
        },
        "paths": paths,
    });
    serde_json::to_string_pretty(&document).expect("inspection JSON serialization") + "\n"
}

fn render_human(inspection: &Inspection) -> String {
    let mut output = String::new();
    writeln!(&mut output, "Hoplite application inspection").unwrap();
    writeln!(&mut output, "inspection format: {FORMAT}").unwrap();
    writeln!(
        &mut output,
        "application bundle: {}",
        application_bundle::FORMAT
    )
    .unwrap();
    writeln!(
        &mut output,
        "runtime ABI: {}",
        application_bundle::RUNTIME_ABI_VERSION
    )
    .unwrap();
    writeln!(
        &mut output,
        "bundle: {} bytes (sha256:{})",
        inspection.bundle_bytes, inspection.bundle_sha256
    )
    .unwrap();
    writeln!(
        &mut output,
        "manifest: format {}, {} bytes (sha256:{})",
        inspection.manifest.format, inspection.manifest_bytes, inspection.manifest_sha256
    )
    .unwrap();
    writeln!(
        &mut output,
        "embedded HBX0: {} bytes",
        inspection.bytecode_bytes
    )
    .unwrap();
    writeln!(
        &mut output,
        "applications: {}; routes: {}",
        inspection.manifest.applications, inspection.manifest.routes
    )
    .unwrap();
    let adapters = inspection
        .manifest
        .adapters
        .iter()
        .map(|(adapter, count)| format!("{adapter}={count}"))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        &mut output,
        "route adapters: {}",
        if adapters.is_empty() {
            "none"
        } else {
            adapters.as_str()
        }
    )
    .unwrap();
    writeln!(
        &mut output,
        "nginx configuration: {}",
        describe_artifact(&inspection.nginx_configuration)
    )
    .unwrap();
    writeln!(
        &mut output,
        "platform HTA: {}",
        describe_artifact(&inspection.platform_hta)
    )
    .unwrap();
    writeln!(
        &mut output,
        "platform EDN: {}",
        describe_artifact(&inspection.platform_edn)
    )
    .unwrap();
    writeln!(
        &mut output,
        "OpenAPI documents: {}",
        inspection.openapi_documents
    )
    .unwrap();
    if inspection.source_inputs.count == 0 {
        writeln!(&mut output, "source-free output: yes").unwrap();
    } else {
        writeln!(
            &mut output,
            "source-free output: no ({} source input{})",
            inspection.source_inputs.count,
            if inspection.source_inputs.count == 1 {
                ""
            } else {
                "s"
            }
        )
        .unwrap();
        for path in &inspection.source_inputs.examples {
            writeln!(&mut output, "  source input: {path}").unwrap();
        }
    }
    if let Some(paths) = &inspection.paths {
        writeln!(&mut output, "output: {}", paths.output).unwrap();
        writeln!(&mut output, "bundle path: {}", paths.bundle).unwrap();
        writeln!(&mut output, "manifest path: {}", paths.manifest).unwrap();
    } else {
        writeln!(&mut output, "paths: redacted (use --show-paths)").unwrap();
    }
    output
}

fn describe_artifact(artifact: &Artifact) -> String {
    if !artifact.present {
        return "absent".into();
    }
    format!(
        "{} bytes (sha256:{})",
        artifact.bytes.unwrap_or_default(),
        artifact.sha256.as_deref().unwrap_or("unknown")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{self, App, Config, Route, RouteAdapter};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn fixture_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "hoplite-inspect-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn write_fixture(root: &Path) {
        let output = root.join(".hoplite");
        fs::create_dir_all(output.join("conf")).unwrap();
        fs::create_dir_all(output.join("openapi")).unwrap();

        let config = Config {
            workers: 2,
            apps: vec![App {
                id: 1,
                name: "example".into(),
                port: 8080,
                hostnames: Vec::new(),
                routes: vec![
                    Route {
                        method: "GET".into(),
                        path: "/raw".into(),
                        handler: "example/raw".into(),
                        name: None,
                        summary: None,
                        adapter: RouteAdapter::Raw,
                    },
                    Route {
                        method: "GET".into(),
                        path: "/request".into(),
                        handler: "example/request".into(),
                        name: None,
                        summary: None,
                        adapter: RouteAdapter::Request,
                    },
                    Route {
                        method: "POST".into(),
                        path: "/hta".into(),
                        handler: "example/hta".into(),
                        name: None,
                        summary: None,
                        adapter: RouteAdapter::RequestHta,
                    },
                ],
                request_body: None,
                proxies: Vec::new(),
                openapi_path: None,
            }],
        };
        let manifest = app::manifest(&config).unwrap();
        let bundle = application_bundle::encode(&manifest, b"HBX0inspection-bytecode").unwrap();
        fs::write(output.join("app.hbx"), bundle).unwrap();
        fs::write(output.join("apps.hta"), manifest).unwrap();
        fs::write(output.join("conf/nginx.conf"), "events {}\n").unwrap();
        fs::write(output.join("platform.hta"), b"platform-hta").unwrap();
        fs::write(output.join("platform.edn"), "{:platform true}\n").unwrap();
        fs::write(output.join("openapi/example.json"), "{}\n").unwrap();
    }

    #[test]
    fn inspects_generated_output_without_executing_source() {
        let root = fixture_root();
        write_fixture(&root);

        let inspection = inspect_target(&root, None, false).unwrap();
        assert_eq!(inspection.manifest.format, MANIFEST_FORMAT);
        assert_eq!(inspection.manifest.applications, 1);
        assert_eq!(inspection.manifest.routes, 3);
        assert_eq!(inspection.manifest.adapters["raw"], 1);
        assert_eq!(inspection.manifest.adapters["request"], 1);
        assert_eq!(inspection.manifest.adapters["request+hta"], 1);
        assert_eq!(inspection.openapi_documents, 1);
        assert!(inspection.nginx_configuration.present);
        assert_eq!(inspection.source_inputs.count, 0);

        let rendered = render_json(&inspection);
        assert!(!rendered.contains(root.to_string_lossy().as_ref()));
        assert!(rendered.contains("\"source_free\": true"));
        assert!(render_human(&inspection).contains("paths: redacted"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_paths_only_when_explicitly_requested() {
        let root = fixture_root();
        write_fixture(&root);

        let inspection = inspect_target(&root, None, true).unwrap();
        let rendered = render_json(&inspection);
        assert!(rendered.contains(root.to_string_lossy().as_ref()));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_source_inputs_using_relative_names() {
        let root = fixture_root();
        write_fixture(&root);
        fs::write(root.join(".hoplite/app.hal"), "(ns leaked.source)\n").unwrap();

        let inspection = inspect_target(&root, None, false).unwrap();
        assert_eq!(inspection.source_inputs.count, 1);
        assert_eq!(
            inspection.source_inputs.examples,
            vec!["app.hal".to_owned()]
        );
        let rendered = render_json(&inspection);
        assert!(rendered.contains("\"source_free\": false"));
        assert!(!rendered.contains(root.to_string_lossy().as_ref()));

        fs::remove_dir_all(root).unwrap();
    }
}
