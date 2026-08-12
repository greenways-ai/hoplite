use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const ALLOWED_STATUSES: [&str; 4] = ["public", "experimental", "migration-only", "internal"];
const REGISTRY_FORMAT: &str = "hoplite.public-surfaces/0-alpha";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("core package must live beneath the repository root")
        .to_path_buf()
}

fn section<'a>(document: &'a Value, name: &str) -> &'a Vec<Value> {
    document
        .get(name)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("public-surface registry must contain array {name}"))
}

fn field<'a>(entry: &'a Value, section: &str, name: &str) -> &'a str {
    entry
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{section} entry must contain string field {name}"))
}

fn immediate_files(directory: &Path, extension: &str, root: &Path) -> BTreeSet<String> {
    fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()))
        .map(|entry| entry.expect("directory entry must be readable").path())
        .filter(|path| path.is_file())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some(extension))
        .map(|path| {
            path.strip_prefix(root)
                .expect("inventory path must be beneath repository root")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

fn registered_paths(document: &Value, section_name: &str) -> BTreeSet<String> {
    section(document, section_name)
        .iter()
        .map(|entry| field(entry, section_name, "path").to_owned())
        .collect()
}

#[test]
fn public_surface_registry_is_well_formed_and_complete() {
    let root = repo_root();
    let registry_path = root.join("docs/public-surfaces.json");
    let registry_source = fs::read_to_string(&registry_path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", registry_path.display()));
    let document: Value = serde_json::from_str(&registry_source)
        .unwrap_or_else(|error| panic!("invalid {}: {error}", registry_path.display()));

    assert_eq!(
        document.get("format").and_then(Value::as_str),
        Some(REGISTRY_FORMAT),
        "the registry format is an evolving alpha contract"
    );

    for section_name in [
        "hal_namespaces",
        "native_headers",
        "cli_programs",
        "cli_commands",
        "portable_documents",
    ] {
        let mut identities = BTreeSet::new();
        for entry in section(&document, section_name) {
            let name = field(entry, section_name, "name");
            let status = field(entry, section_name, "status");
            let summary = field(entry, section_name, "summary");
            let identity = if section_name == "cli_commands" {
                format!("{} {name}", field(entry, section_name, "program"))
            } else {
                name.to_owned()
            };

            assert!(
                identities.insert(identity.clone()),
                "duplicate {section_name} entry: {identity}"
            );
            assert!(
                ALLOWED_STATUSES.contains(&status),
                "unsupported status {status} for {section_name} entry {identity}"
            );
            assert!(
                !summary.trim().is_empty(),
                "{section_name} entry {identity} must explain its compatibility role"
            );

            if let Some(path) = entry.get("path").and_then(Value::as_str) {
                assert!(
                    root.join(path).is_file(),
                    "{section_name} entry {identity} points at missing file {path}"
                );
            }
        }
    }

    let registered_hal = registered_paths(&document, "hal_namespaces");
    let actual_hal = immediate_files(&root.join("core/lib/src/hoplite"), "hal", &root);
    assert_eq!(
        registered_hal, actual_hal,
        "every hoplite.* HAL namespace must receive an explicit compatibility status"
    );

    for entry in section(&document, "hal_namespaces") {
        let name = field(entry, "hal_namespaces", "name");
        let path = field(entry, "hal_namespaces", "path");
        let stem = Path::new(path)
            .file_stem()
            .and_then(|value| value.to_str())
            .expect("HAL path must have a UTF-8 file stem")
            .replace('_', "-");
        assert_eq!(
            name,
            format!("hoplite.{stem}"),
            "HAL namespace and file name must agree"
        );
    }

    let registered_headers = registered_paths(&document, "native_headers");
    let mut actual_headers = immediate_files(&root.join("core/nginx"), "h", &root);
    actual_headers.extend(immediate_files(
        &root.join("core/abi/data-plane-ffi/include"),
        "h",
        &root,
    ));
    assert_eq!(
        registered_headers, actual_headers,
        "every shipped Hoplite C header must receive an explicit compatibility status"
    );

    for entry in section(&document, "native_headers") {
        let name = field(entry, "native_headers", "name");
        let path = field(entry, "native_headers", "path");
        assert_eq!(
            Path::new(path).file_name().and_then(|value| value.to_str()),
            Some(name),
            "native header name and path must agree"
        );
    }

    let registered_program_paths = registered_paths(&document, "cli_programs");
    let mut actual_program_paths = immediate_files(&root.join("core/src/bin"), "rs", &root);
    actual_program_paths.insert("core/src/main.rs".to_owned());
    assert_eq!(
        registered_program_paths, actual_program_paths,
        "every compiled Hoplite binary target must receive an explicit compatibility status"
    );

    let registered_program_names = section(&document, "cli_programs")
        .iter()
        .map(|entry| field(entry, "cli_programs", "name"))
        .collect::<BTreeSet<_>>();
    for entry in section(&document, "cli_programs") {
        let name = field(entry, "cli_programs", "name");
        let path = field(entry, "cli_programs", "path");
        let expected = if path == "core/src/main.rs" {
            "hoplite".to_owned()
        } else {
            Path::new(path)
                .file_stem()
                .and_then(|value| value.to_str())
                .expect("CLI program path must have a UTF-8 file stem")
                .to_owned()
        };
        assert_eq!(name, expected, "CLI program name and path must agree");
        assert!(
            !field(entry, "cli_programs", "availability")
                .trim()
                .is_empty(),
            "CLI program {name} must declare its build availability"
        );
    }

    for entry in section(&document, "cli_commands") {
        let program = field(entry, "cli_commands", "program");
        let name = field(entry, "cli_commands", "name");
        assert!(
            registered_program_names.contains(program),
            "CLI command {program} {name} refers to an unregistered program"
        );
        assert!(
            !field(entry, "cli_commands", "availability")
                .trim()
                .is_empty(),
            "CLI command {program} {name} must declare its build availability"
        );
    }

    for entry in section(&document, "portable_documents") {
        let name = field(entry, "portable_documents", "name");
        assert!(
            name.contains('/'),
            "portable document {name} must carry an explicit contract version or epoch"
        );
    }
}
