from __future__ import annotations

import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/quarantine-provider-tooling.yml"


def git_mv(source: str, destination: str) -> None:
    target = ROOT / destination
    target.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(["git", "mv", source, destination], cwd=ROOT, check=True)


def strip_prefix(path: str, prefix: str) -> None:
    target = ROOT / path
    source = target.read_text()
    if not source.startswith(prefix):
        raise SystemExit(f"{path}: unexpected module prefix")
    target.write_text(source[len(prefix):])


def replace_once(path: str, old: str, new: str) -> None:
    target = ROOT / path
    source = target.read_text()
    count = source.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one replacement target, found {count}")
    target.write_text(source.replace(old, new, 1))


for source, destination in [
    ("core/src/provider_lock.rs", "migration/provider-products/src/provider_lock.rs"),
    (
        "core/src/provider_lock/object_backend.rs",
        "migration/provider-products/src/provider_lock/object_backend.rs",
    ),
    (
        "core/src/provider_lock/provider_set.rs",
        "migration/provider-products/src/provider_lock/provider_set.rs",
    ),
    (
        "core/src/provider_manifest.rs",
        "migration/provider-products/src/provider_manifest.rs",
    ),
    (
        "core/src/bin/hoplite-provider-lock.rs",
        "migration/provider-products/bin/hoplite-provider-lock.rs",
    ),
    (
        "core/src/bin/hoplite-provider-manifest.rs",
        "migration/provider-products/bin/hoplite-provider-manifest.rs",
    ),
    (
        "core/src/bin/hoplite-object-backend-lock.rs",
        "migration/provider-products/bin/hoplite-object-backend-lock.rs",
    ),
    (
        "core/src/bin/hoplite-provider-set-lock.rs",
        "migration/provider-products/bin/hoplite-provider-set-lock.rs",
    ),
]:
    git_mv(source, destination)

single_prefix = '#[path = "../provider_manifest.rs"]\nmod provider_manifest;\n\n'
dual_prefix = (
    '#[path = "../provider_lock.rs"]\n'
    'mod provider_lock;\n'
    '#[path = "../provider_manifest.rs"]\n'
    'mod provider_manifest;\n\n'
)
strip_prefix(
    "migration/provider-products/bin/hoplite-provider-manifest.rs",
    single_prefix,
)
for path in [
    "migration/provider-products/bin/hoplite-provider-lock.rs",
    "migration/provider-products/bin/hoplite-object-backend-lock.rs",
    "migration/provider-products/bin/hoplite-provider-set-lock.rs",
]:
    strip_prefix(path, dual_prefix)

single_wrapper = '''#[path = "../../../migration/provider-products/src/provider_manifest.rs"]
mod provider_manifest;

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../migration/provider-products/bin/hoplite-provider-manifest.rs"
));
'''
(ROOT / "core/src/bin/hoplite-provider-manifest.rs").write_text(single_wrapper)

for name in [
    "hoplite-provider-lock",
    "hoplite-object-backend-lock",
    "hoplite-provider-set-lock",
]:
    wrapper = f'''#[path = "../../../migration/provider-products/src/provider_lock.rs"]
mod provider_lock;
#[path = "../../../migration/provider-products/src/provider_manifest.rs"]
mod provider_manifest;

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../migration/provider-products/bin/{name}.rs"
));
'''
    (ROOT / f"core/src/bin/{name}.rs").write_text(wrapper)

(ROOT / "migration/provider-products/README.md").write_text(
    """# Legacy provider distribution tooling

This directory contains the historical provider manifest, provider lock, object
backend lock, and provider-set lock validators and their CLI implementations.
They are trusted distribution tooling retained only for extraction and
compatibility. They are not part of the default Hoplite binaries, generic
application model, worker runtime, production image, or required merge gate.

The existing Cargo target names remain as tiny wrappers under `core/src/bin` so
a deliberate build with `legacy-provider-products` can run the exact historical
interfaces during the migration window. Request data and application values
cannot enable these targets or select provider packages, artifact identities,
repositories, tags, paths, roots, credentials, or backends.

## Compatibility window

The wrappers remain for one documented pre-1.0 migration window while provider
release and distribution ownership moves to the extracted products. New generic
Hoplite code must not import these validators. Eventual removal must preserve
closed-schema parsing, bounded input, duplicate-field rejection, exact digest
binding, request-selection exclusion, and negative conformance in the owning
product repositories.
"""
)

boundary_path = ROOT / "docs/core-boundary.json"
boundary = json.loads(boundary_path.read_text())
removed = {
    "core/src/provider_lock.rs",
    "core/src/provider_lock/object_backend.rs",
    "core/src/provider_lock/provider_set.rs",
    "core/src/provider_manifest.rs",
}
boundary["rust_sources"] = [
    entry for entry in boundary["rust_sources"] if entry["path"] not in removed
]
boundary["default_product"]["opt_in_features"]["legacy-provider-products"] = (
    "Historical provider manifest and lock tools physically quarantined and retained only for path-scoped extraction conformance."
)
products = boundary["migration_products"]
if any(entry["path"] == "migration/provider-products" for entry in products):
    raise SystemExit("provider-products migration inventory already exists")
products.append(
    {
        "path": "migration/provider-products",
        "status": "migration-only",
        "summary": (
            "Physically quarantined historical provider distribution validators, CLI implementations, and focused tests."
        ),
    }
)
products.sort(key=lambda entry: entry["path"])
boundary_path.write_text(json.dumps(boundary, indent=2) + "\n")

public_path = ROOT / "docs/public-surfaces.json"
public = json.loads(public_path.read_text())
for entry in public["cli_programs"]:
    if entry["name"] in {
        "hoplite-object-backend-lock",
        "hoplite-provider-lock",
        "hoplite-provider-manifest",
        "hoplite-provider-set-lock",
    }:
        entry["summary"] = (
            "Migration-only wrapper for physically quarantined provider distribution tooling, available only through legacy-provider-products."
        )
public_path.write_text(json.dumps(public, indent=2) + "\n")

replace_once(
    "docs/core-boundary.md",
    '| `legacy-provider-products` | Enables historical provider manifest and lock generators while release/product material is extracted. |',
    '| `legacy-provider-products` | Enables tiny Cargo wrappers for provider manifest and lock implementations physically quarantined beneath `migration/provider-products`. |',
)
replace_once(
    "docs/core-boundary.md",
    'The legacy features are temporary migration seams, not extension points and not\nrelease promises.',
    'The legacy features are temporary migration seams, not extension points and not\nrelease promises. Provider manifest/lock validators and their CLI implementations\nare physically outside `core/src`; only target-name wrappers remain for the\ndocumented compatibility window.',
)

replace_once(
    "docs/public-api.md",
    'The object/provider lock\ngenerator binaries are migration-only source-build tools.',
    'The object/provider lock generator targets are migration-only wrappers around\nimplementations physically quarantined beneath `migration/provider-products` and\navailable only through `legacy-provider-products`.',
)

for temporary in [WORKFLOW, Path(__file__)]:
    temporary.unlink()
