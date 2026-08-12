use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const FORMAT: &str = "hoplite.core-boundary/0-alpha";
const ALLOWED_STATUSES: [&str; 8] = [
    "public",
    "core",
    "generic-interface",
    "reference-conformance",
    "development",
    "distribution",
    "internal-evidence",
    "migration-only",
];

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
        .unwrap_or_else(|| panic!("core-boundary registry must contain array {name}"))
}

fn field<'a>(entry: &'a Value, section_name: &str, name: &str) -> &'a str {
    entry
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{section_name} entry must contain string field {name}"))
}

fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .expect("inventory path must be beneath repository root")
        .to_string_lossy()
        .replace('\\', "/")
}

fn recursive_files(directory: &Path, extension: &str, root: &Path) -> BTreeSet<String> {
    let mut output = BTreeSet::new();
    collect_recursive_files(directory, extension, root, &mut output);
    output
}

fn collect_recursive_files(
    directory: &Path,
    extension: &str,
    root: &Path,
    output: &mut BTreeSet<String>,
) {
    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()))
    {
        let path = entry.expect("directory entry must be readable").path();
        if path.is_dir() {
            collect_recursive_files(&path, extension, root, output);
        } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
            output.insert(relative(&path, root));
        }
    }
}

fn immediate_directories(directory: &Path, root: &Path) -> BTreeSet<String> {
    fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()))
        .map(|entry| entry.expect("directory entry must be readable").path())
        .filter(|path| path.is_dir())
        .map(|path| relative(&path, root))
        .collect()
}

fn immediate_files(directory: &Path, root: &Path) -> BTreeSet<String> {
    fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()))
        .map(|entry| entry.expect("directory entry must be readable").path())
        .filter(|path| path.is_file())
        .map(|path| relative(&path, root))
        .collect()
}

fn registered_paths(document: &Value, section_name: &str) -> BTreeSet<String> {
    section(document, section_name)
        .iter()
        .map(|entry| field(entry, section_name, "path").to_owned())
        .collect()
}

#[test]
fn core_boundary_is_explicit_complete_and_consistent() {
    let root = repo_root();
    let path = root.join("docs/core-boundary.json");
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let document: Value = serde_json::from_str(&source)
        .unwrap_or_else(|error| panic!("invalid {}: {error}", path.display()));

    assert_eq!(
        document.get("format").and_then(Value::as_str),
        Some(FORMAT),
        "the core-boundary registry is an evolving alpha contract"
    );

    for section_name in [
        "rust_sources",
        "abi_packages",
        "provider_products",
        "packaging_scripts",
    ] {
        let mut paths = BTreeSet::new();
        for entry in section(&document, section_name) {
            let path = field(entry, section_name, "path");
            let status = field(entry, section_name, "status");
            let summary = field(entry, section_name, "summary");
            assert!(
                paths.insert(path),
                "duplicate {section_name} entry for {path}"
            );
            assert!(
                ALLOWED_STATUSES.contains(&status),
                "unsupported status {status} for {section_name} entry {path}"
            );
            assert!(
                !summary.trim().is_empty(),
                "{section_name} entry {path} must explain its role"
            );
            assert!(
                root.join(path).exists(),
                "{section_name} entry points at missing path {path}"
            );
        }
    }

    assert_eq!(
        registered_paths(&document, "rust_sources"),
        recursive_files(&root.join("core/src"), "rs", &root),
        "every Rust source under core/src must receive an explicit boundary status"
    );
    assert_eq!(
        registered_paths(&document, "abi_packages"),
        immediate_directories(&root.join("core/abi"), &root),
        "every top-level core/abi package must receive an explicit boundary status"
    );
    assert_eq!(
        registered_paths(&document, "provider_products"),
        immediate_directories(&root.join("packaging/providers"), &root),
        "every provider-product packaging directory must be explicit"
    );
    assert_eq!(
        registered_paths(&document, "packaging_scripts"),
        immediate_files(&root.join("packaging/scripts"), &root),
        "every packaging script must receive an explicit boundary status"
    );

    let default_product = document
        .get("default_product")
        .and_then(Value::as_object)
        .expect("core-boundary registry must declare default_product");
    let binaries = default_product
        .get("binary_targets")
        .and_then(Value::as_array)
        .expect("default_product must declare binary_targets")
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("default binary target must be a string")
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(binaries, BTreeSet::from(["hoplite", "hoplite-server"]));

    let opt_in = default_product
        .get("opt_in_features")
        .and_then(Value::as_object)
        .expect("default_product must declare opt_in_features")
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        opt_in,
        BTreeSet::from([
            "internal-evidence",
            "legacy-management",
            "legacy-provider-products",
        ])
    );

    let public_source = fs::read_to_string(root.join("docs/public-surfaces.json"))
        .expect("public-surface registry must be readable");
    let public: Value = serde_json::from_str(&public_source)
        .expect("public-surface registry must be valid JSON");
    let availability = public
        .get("cli_programs")
        .and_then(Value::as_array)
        .expect("public-surface registry must contain cli_programs")
        .iter()
        .map(|entry| {
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .expect("CLI program must have a name");
            let availability = entry
                .get("availability")
                .and_then(Value::as_str)
                .expect("CLI program must have availability");
            (name, availability)
        })
        .collect::<BTreeMap<_, _>>();

    for program in ["hoplite", "hoplite-server"] {
        assert_eq!(availability.get(program), Some(&"default"));
    }
    assert_eq!(
        availability.get("hoplite-bytecode-loading"),
        Some(&"internal-evidence")
    );
    for program in [
        "hoplite-object-backend-lock",
        "hoplite-provider-lock",
        "hoplite-provider-manifest",
        "hoplite-provider-set-lock",
    ] {
        assert_eq!(
            availability.get(program),
            Some(&"legacy-provider-products")
        );
    }
}
