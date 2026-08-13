from __future__ import annotations

import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/quarantine-legacy-value.yml"


def replace_once(path: str, old: str, new: str) -> None:
    target = ROOT / path
    source = target.read_text()
    count = source.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one replacement target, found {count}")
    target.write_text(source.replace(old, new, 1))


def git_mv(source: str, destination: str) -> None:
    target = ROOT / destination
    target.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["git", "mv", source, destination],
        cwd=ROOT,
        check=True,
    )


git_mv(
    "core/lib/src/hoplite/value.hal",
    "migration/value/src/hoplite/value.hal",
)
git_mv(
    "core/lib/test/hoplite/value_test.hal",
    "migration/value/test/hoplite/value_test.hal",
)

(ROOT / "migration/value/README.md").write_text(
    """# Legacy `hoplite.value` compatibility material\n\n"
    "This directory contains the historical bounded canonical-value HAL contract.\n"
    "It is migration input, not part of the default Hoplite runtime, application\n"
    "bundle, production image, public extension model, or required merge gate.\n\n"
    "The exact namespace remains available only with the non-default Cargo feature\n"
    "`legacy-value-contract`. Its focused conformance runs in the path-scoped\n"
    "Compatibility workflow. Request data cannot enable this feature or select the\n"
    "contract at runtime.\n\n"
    "## Compatibility window\n\n"
    "The feature remains available for one documented pre-1.0 migration window while\n"
    "consumers move to the owning value-provider package. New Hoplite applications\n"
    "must not import `hoplite.value`. The eventual extraction or retirement change\n"
    "must preserve any generic boundedness, canonical decoding, digest, corruption,\n"
    "and failure-shape invariant at the appropriate provider or data-plane boundary.\n"
)

replace_once(
    "core/Cargo.toml",
    "internal-evidence = []\nlegacy-provider-products = []\nlegacy-management = [",
    "internal-evidence = []\nlegacy-provider-products = []\nlegacy-value-contract = []\nlegacy-management = [",
)

replace_once(
    "core/src/app.rs",
    'pub const RESPONSE_SOURCE: &str = include_str!("../lib/src/hoplite/response_source.hal");\n'
    'pub const VALUE_SOURCE: &str = include_str!("../lib/src/hoplite/value.hal");\n'
    '#[cfg(test)]\n'
    'const CORE_TEST_SOURCE: &str = include_str!("../lib/test/hoplite/core_test.hal");\n'
    '#[cfg(test)]\n'
    'const RESPONSE_SOURCE_TEST_SOURCE: &str =\n'
    '    include_str!("../lib/test/hoplite/response_source_test.hal");\n'
    '#[cfg(test)]\n'
    'const VALUE_TEST_SOURCE: &str = include_str!("../lib/test/hoplite/value_test.hal");',
    'pub const RESPONSE_SOURCE: &str = include_str!("../lib/src/hoplite/response_source.hal");\n'
    '#[cfg(feature = "legacy-value-contract")]\n'
    'pub const VALUE_SOURCE: &str =\n'
    '    include_str!("../../migration/value/src/hoplite/value.hal");\n'
    '#[cfg(test)]\n'
    'const CORE_TEST_SOURCE: &str = include_str!("../lib/test/hoplite/core_test.hal");\n'
    '#[cfg(test)]\n'
    'const RESPONSE_SOURCE_TEST_SOURCE: &str =\n'
    '    include_str!("../lib/test/hoplite/response_source_test.hal");\n'
    '#[cfg(all(test, feature = "legacy-value-contract"))]\n'
    'const VALUE_TEST_SOURCE: &str =\n'
    '    include_str!("../../migration/value/test/hoplite/value_test.hal");',
)

replace_once(
    "core/src/app.rs",
    'pub fn register_contract_resources(runtime: &mut Runtime) {\n'
    '    runtime.register_resource("hoplite.response-source", RESPONSE_SOURCE);\n'
    '    runtime.register_resource("hoplite.value", VALUE_SOURCE);\n'
    '}\n\n'
    'pub fn register_resources(runtime: &mut Runtime) {',
    'pub fn register_contract_resources(runtime: &mut Runtime) {\n'
    '    runtime.register_resource("hoplite.response-source", RESPONSE_SOURCE);\n'
    '}\n\n'
    'pub fn register_resources(runtime: &mut Runtime) {',
)
replace_once(
    "core/src/app.rs",
    '    runtime.register_resource("hoplite.raw", RAW_SOURCE);\n'
    '    register_contract_resources(runtime);\n}',
    '    runtime.register_resource("hoplite.raw", RAW_SOURCE);\n'
    '    register_contract_resources(runtime);\n'
    '    #[cfg(feature = "legacy-value-contract")]\n'
    '    runtime.register_resource("hoplite.value", VALUE_SOURCE);\n}',
)

replace_once(
    "core/src/app.rs",
    '    #[test]\n'
    '    fn value_boundary_contract_evaluates_from_hoplite() {',
    '    #[cfg(not(feature = "legacy-value-contract"))]\n'
    '    #[test]\n'
    '    fn default_resources_exclude_the_legacy_value_contract() {\n'
    '        let mut runtime = Runtime::new();\n'
    '        register_resources(&mut runtime);\n'
    '        assert!(runtime\n'
    '            .eval_native_value(\n'
    '                "(ns sample.value (:require [hoplite.value :as value])) true",\n'
    '            )\n'
    '            .is_err());\n'
    '    }\n\n'
    '    #[cfg(feature = "legacy-value-contract")]\n'
    '    #[test]\n'
    '    fn value_boundary_contract_evaluates_from_hoplite() {',
)

replace_once(
    "core/src/main.rs",
    '    let mut modules = HashMap::new();\n'
    '    for source in [\n'
    '        app::CORE_SOURCE,\n'
    '        app::HOST_SOURCE,\n'
    '        app::INTERNAL_SOURCE,\n'
    '        app::RAW_SOURCE,\n'
    '        app::RESPONSE_SOURCE,\n'
    '        app::VALUE_SOURCE,\n'
    '    ] {',
    '    let mut modules = HashMap::new();\n'
    '    let mut builtins = vec![\n'
    '        app::CORE_SOURCE,\n'
    '        app::HOST_SOURCE,\n'
    '        app::INTERNAL_SOURCE,\n'
    '        app::RAW_SOURCE,\n'
    '        app::RESPONSE_SOURCE,\n'
    '    ];\n'
    '    #[cfg(feature = "legacy-value-contract")]\n'
    '    builtins.push(app::VALUE_SOURCE);\n'
    '    for source in builtins {',
)

replace_once(
    "core/src/main.rs",
    '    #[test]\n'
    '    fn orders_application_modules_before_their_dependents() {',
    '    #[cfg(not(feature = "legacy-value-contract"))]\n'
    '    #[test]\n'
    '    fn default_application_bundle_excludes_the_legacy_value_contract() {\n'
    '        let modules = application_modules(&[]).unwrap();\n'
    '        assert!(!modules\n'
    '            .iter()\n'
    '            .any(|module| module.namespace == "hoplite.value"));\n'
    '    }\n\n'
    '    #[cfg(feature = "legacy-value-contract")]\n'
    '    #[test]\n'
    '    fn compatibility_application_bundle_includes_the_legacy_value_contract() {\n'
    '        let modules = application_modules(&[]).unwrap();\n'
    '        assert!(modules\n'
    '            .iter()\n'
    '            .any(|module| module.namespace == "hoplite.value"));\n'
    '    }\n\n'
    '    #[test]\n'
    '    fn orders_application_modules_before_their_dependents() {',
)

replace_once(
    "core/tests/public_surfaces.rs",
    'fn registered_paths(document: &Value, section_name: &str) -> BTreeSet<String> {\n'
    '    section(document, section_name)\n'
    '        .iter()\n'
    '        .map(|entry| field(entry, section_name, "path").to_owned())\n'
    '        .collect()\n'
    '}\n',
    'fn registered_paths(document: &Value, section_name: &str) -> BTreeSet<String> {\n'
    '    section(document, section_name)\n'
    '        .iter()\n'
    '        .map(|entry| field(entry, section_name, "path").to_owned())\n'
    '        .collect()\n'
    '}\n\n'
    'fn migration_hal_files(root: &Path) -> BTreeSet<String> {\n'
    '    let migration = root.join("migration");\n'
    '    if !migration.is_dir() {\n'
    '        return BTreeSet::new();\n'
    '    }\n'
    '    let mut output = BTreeSet::new();\n'
    '    for entry in fs::read_dir(&migration).expect("migration directory must be readable") {\n'
    '        let source = entry\n'
    '            .expect("migration entry must be readable")\n'
    '            .path()\n'
    '            .join("src/hoplite");\n'
    '        if source.is_dir() {\n'
    '            output.extend(immediate_files(&source, "hal", root));\n'
    '        }\n'
    '    }\n'
    '    output\n'
    '}\n',
)
replace_once(
    "core/tests/public_surfaces.rs",
    '    let registered_hal = registered_paths(&document, "hal_namespaces");\n'
    '    let actual_hal = immediate_files(&root.join("core/lib/src/hoplite"), "hal", &root);',
    '    let registered_hal = registered_paths(&document, "hal_namespaces");\n'
    '    let mut actual_hal = immediate_files(&root.join("core/lib/src/hoplite"), "hal", &root);\n'
    '    actual_hal.extend(migration_hal_files(&root));',
)

replace_once(
    "core/tests/core_boundary.rs",
    '        "provider_products",\n'
    '        "packaging_scripts",',
    '        "provider_products",\n'
    '        "migration_products",\n'
    '        "packaging_scripts",',
)
replace_once(
    "core/tests/core_boundary.rs",
    '    assert_eq!(\n'
    '        registered_paths(&document, "provider_products"),\n'
    '        immediate_directories(&root.join("packaging/providers"), &root),\n'
    '        "every provider-product packaging directory must be explicit"\n'
    '    );\n'
    '    assert_eq!(\n'
    '        registered_paths(&document, "packaging_scripts"),',
    '    assert_eq!(\n'
    '        registered_paths(&document, "provider_products"),\n'
    '        immediate_directories(&root.join("packaging/providers"), &root),\n'
    '        "every provider-product packaging directory must be explicit"\n'
    '    );\n'
    '    assert_eq!(\n'
    '        registered_paths(&document, "migration_products"),\n'
    '        immediate_directories(&root.join("migration"), &root),\n'
    '        "every physically quarantined migration product must be explicit"\n'
    '    );\n'
    '    assert_eq!(\n'
    '        registered_paths(&document, "packaging_scripts"),',
)
replace_once(
    "core/tests/core_boundary.rs",
    '            "legacy-management",\n'
    '            "legacy-provider-products",',
    '            "legacy-management",\n'
    '            "legacy-provider-products",\n'
    '            "legacy-value-contract",',
)

public_path = ROOT / "docs/public-surfaces.json"
public = json.loads(public_path.read_text())
value = next(entry for entry in public["hal_namespaces"] if entry["name"] == "hoplite.value")
value["path"] = "migration/value/src/hoplite/value.hal"
value["availability"] = "legacy-value-contract"
value["summary"] = (
    "Historical canonical-value provider contract physically quarantined and "
    "available only through a non-default compatibility feature."
)
public_path.write_text(json.dumps(public, indent=2) + "\n")

boundary_path = ROOT / "docs/core-boundary.json"
boundary = json.loads(boundary_path.read_text())
boundary["default_product"]["opt_in_features"]["legacy-value-contract"] = (
    "Historical hoplite.value HAL contract retained only for path-scoped migration conformance."
)
boundary["migration_products"] = [
    {
        "path": "migration/value",
        "status": "migration-only",
        "summary": (
            "Physically quarantined historical canonical-value HAL contract and focused conformance."
        ),
    }
]
boundary_path.write_text(json.dumps(boundary, indent=2) + "\n")

replace_once(
    "docs/core-boundary.md",
    "Three non-default features make retained source explicit:",
    "Four non-default features make retained source explicit:",
)
replace_once(
    "docs/core-boundary.md",
    '| `legacy-provider-products` | Enables historical provider manifest and lock generators while release/product material is extracted. |',
    '| `legacy-provider-products` | Enables historical provider manifest and lock generators while release/product material is extracted. |\n'
    '| `legacy-value-contract` | Enables the physically quarantined historical `hoplite.value` HAL contract only for migration conformance. |',
)
replace_once(
    "docs/core-boundary.md",
    'The legacy features are temporary migration seams, not extension points and not\n'
    'release promises. New generic runtime code must not depend on them.',
    'The legacy features are temporary migration seams, not extension points and not\n'
    'release promises. New generic runtime code must not depend on them. Ordinary\n'
    'resource registration and application-bundle construction exclude `hoplite.value`;\n'
    'only the explicit `legacy-value-contract` feature can restore it for focused\n'
    'compatibility testing.',
)
replace_once(
    "docs/core-boundary.md",
    'The required workspace test inventories every Rust source under `core/src`, every\n'
    'top-level package under `core/abi`, every provider-product packaging directory,\n'
    'and every packaging script.',
    'The required workspace test inventories every Rust source under `core/src`, every\n'
    'top-level package under `core/abi`, every provider-product packaging directory,\n'
    'every physically quarantined top-level migration product, and every packaging\n'
    'script.',
)

replace_once(
    "docs/public-api.md",
    '`hoplite.auth` and `hoplite.value` are migration-only historical\n'
    'policy/provider helpers. They are extraction input, not generic runtime API.',
    '`hoplite.auth` and `hoplite.value` are migration-only historical\n'
    'policy/provider helpers. `hoplite.value` is physically quarantined beneath\n'
    '`migration/value`, excluded from ordinary resource registration and HAB0/HBX0\n'
    'application construction, and available only with `legacy-value-contract` for\n'
    'path-scoped compatibility evidence. Neither namespace is generic runtime API.',
)

replace_once(
    ".github/workflows/compatibility.yml",
    'name: Compatibility\n\non:',
    'name: Compatibility\n\nenv:\n  HARA_REF: 6dd73bbb9ba546b90dee0a075323998dd3abac85\n\non:',
)
replace_once(
    ".github/workflows/compatibility.yml",
    '      - ".github/workflows/compatibility.yml"\n'
    '      - "core/abi/blob-store/**"',
    '      - ".github/workflows/compatibility.yml"\n'
    '      - "core/Cargo.toml"\n'
    '      - "core/src/app.rs"\n'
    '      - "core/src/main.rs"\n'
    '      - "core/tests/core_boundary.rs"\n'
    '      - "core/tests/public_surfaces.rs"\n'
    '      - "docs/core-boundary.json"\n'
    '      - "docs/public-surfaces.json"\n'
    '      - "migration/value/**"\n'
    '      - "core/abi/blob-store/**"',
)

compatibility = ROOT / ".github/workflows/compatibility.yml"
with compatibility.open("a") as output:
    output.write(
        '''\n  legacy-value-contract:\n    name: legacy hoplite.value contract\n    runs-on: ubuntu-24.04\n    timeout-minutes: 20\n    steps:\n      - uses: actions/checkout@v5\n        with:\n          path: hoplite\n      - uses: actions/checkout@v5\n        with:\n          repository: hara-lang/hara\n          ref: ${{ env.HARA_REF }}\n          path: hara\n          submodules: recursive\n      - uses: dtolnay/rust-toolchain@stable\n        with:\n          components: rustfmt\n      - name: Verify the reviewed Hara revision\n        working-directory: hoplite\n        run: test "$(tr -d '[:space:]' < packaging/hara-revision)" = "$HARA_REF"\n      - name: Check focused compatibility formatting\n        run: cargo fmt --manifest-path hoplite/core/Cargo.toml -- --check\n      - name: Run the legacy value boundary contract\n        run: |\n          cargo test \\\n            --manifest-path hoplite/core/Cargo.toml \\\n            --locked \\\n            --features legacy-value-contract \\\n            value_boundary_contract_evaluates_from_hoplite\n      - name: Prove compatibility application bundles include the opted-in namespace\n        run: |\n          cargo test \\\n            --manifest-path hoplite/core/Cargo.toml \\\n            --locked \\\n            --features legacy-value-contract \\\n            compatibility_application_bundle_includes_the_legacy_value_contract\n'''
    )

for temporary in [WORKFLOW, Path(__file__)]:
    temporary.unlink()
