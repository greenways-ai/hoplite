from __future__ import annotations

import base64
import gzip
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/materialize-inspect.yml"
PAYLOAD = ROOT / ".github/scripts/diagnostics.rs.gz.b64"
DIAGNOSTICS_DOC = '''# Inspecting a built Hoplite application

`hoplite inspect` reads generated `.hoplite` output without discovering, parsing,
compiling, or executing application source.

```shell
hoplite inspect [--json] [--show-paths] [--manifest FILE] [PROJECT|OUTPUT|BUNDLE]
```

The command applies the same bounded HAB0 and exact-manifest validation used by
`hoplite verify`, then reports:

- the `hoplite.application-bundle/0-alpha` identity and runtime ABI;
- exact bundle, manifest, generated configuration, and platform artifact sizes
  and SHA-256 digests;
- the embedded HBX0 byte count;
- application, route, and adapter counts from the validated route manifest;
- the number of generated OpenAPI documents;
- whether the inspected output contains application source inputs.

Human-readable output is the default. `--json` emits
`hoplite.inspect/0-alpha`, a public alpha report whose incompatible changes
require a new epoch and migration note.

## Redaction

Filesystem paths are omitted from successful human and JSON output by default.
Use `--show-paths` when local path disclosure is intentional. File failures
report stable classes and I/O categories without embedding build-machine paths.
Source-free failures list only paths relative to the generated output directory.

The command never prints source text, credentials, signatures, native pointers,
or provider internals.

## Required and optional artifacts

`app.hbx` and its exact `apps.hta` are required and validated before a report is
emitted. Generated Nginx configuration, platform HTA/EDN, and OpenAPI documents
are reported as present or absent so an operator can inspect a partial build
without executing it. Invalid or incompatible required inputs exit non-zero.
'''


def replace_once(path: str, old: str, new: str) -> None:
    target = ROOT / path
    source = target.read_text()
    count = source.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one replacement target, found {count}")
    target.write_text(source.replace(old, new, 1))


diagnostics = gzip.decompress(base64.b64decode(PAYLOAD.read_text())).decode()
(ROOT / "core/src/diagnostics.rs").write_text(diagnostics)
(ROOT / "docs/diagnostics.md").write_text(DIAGNOSTICS_DOC)

replace_once(
    "core/src/main.rs",
    "mod dev_console;\nmod host;",
    "mod dev_console;\nmod diagnostics;\nmod host;",
)
replace_once(
    "core/src/main.rs",
    '        Some("serve") => run_serve_command(&arguments[1..])?,\n'
    '        Some("verify") => run_verify_command(&arguments[1..])?,',
    '        Some("serve") => run_serve_command(&arguments[1..])?,\n'
    '        Some("inspect") => diagnostics::run(&arguments[1..])?,\n'
    '        Some("verify") => run_verify_command(&arguments[1..])?,',
)
replace_once(
    "core/src/main.rs",
    '    println!("  hoplite run FILE");\n'
    '    println!("  hoplite verify [--manifest FILE] [PROJECT|OUTPUT|BUNDLE]");',
    '    println!("  hoplite run FILE");\n'
    '    println!("  hoplite inspect [--json] [--show-paths] [PROJECT|OUTPUT|BUNDLE]");\n'
    '    println!("  hoplite verify [--manifest FILE] [PROJECT|OUTPUT|BUNDLE]");',
)

replace_once(
    "README.md",
    'input, even when a project uses `:project/source-paths ["."]`. Verify a built\n'
    'application without executing it with `hoplite verify .`.',
    'input, even when a project uses `:project/source-paths ["."]`. Inspect the\n'
    'generated bundle, route manifest, configuration digests, and source-free status\n'
    'with `hoplite inspect .`; use `--json` for the machine-readable\n'
    '`hoplite.inspect/0-alpha` report. Verify a built application without executing it\n'
    'with `hoplite verify .`.',
)

replace_once(
    "docs/public-api.md",
    'The public `hoplite` control CLI supports `repl`, `eval`, `run`, `verify`,\n'
    '`package`, `serve`, and `version`.',
    'The public `hoplite` control CLI supports `repl`, `eval`, `run`, `inspect`,\n'
    '`verify`, `package`, `serve`, and `version`.',
)
replace_once(
    "docs/public-api.md",
    'Successful commands and help exit zero.\n\n'
    '`hoplite-server` consumes generated `.hoplite` output',
    'Successful commands and help exit zero.\n\n'
    '`hoplite inspect` validates only generated HAB0 and HTA bytes, reports route and\n'
    'adapter counts plus generated-artifact digests, detects source inputs beneath the\n'
    'output directory, and redacts filesystem paths unless `--show-paths` is supplied.\n'
    'Its JSON identity is `hoplite.inspect/0-alpha`; see\n'
    '[Diagnostics](diagnostics.md).\n\n'
    '`hoplite-server` consumes generated `.hoplite` output',
)

replace_once(
    "docs/versioning.md",
    '| Hoplite application bundle | `hoplite.application-bundle/0-alpha` / `HAB0` |\n'
    '| Other evolving owned contracts | `<contract>/0-alpha` |',
    '| Hoplite application bundle | `hoplite.application-bundle/0-alpha` / `HAB0` |\n'
    '| Hoplite inspection report | `hoplite.inspect/0-alpha` |\n'
    '| Other evolving owned contracts | `<contract>/0-alpha` |',
)

public_surfaces_path = ROOT / "docs/public-surfaces.json"
public_surfaces = json.loads(public_surfaces_path.read_text())
commands = public_surfaces["cli_commands"]
if any(entry["program"] == "hoplite" and entry["name"] == "inspect" for entry in commands):
    raise SystemExit("docs/public-surfaces.json already contains hoplite inspect")
package_index = next(
    index
    for index, entry in enumerate(commands)
    if entry["program"] == "hoplite" and entry["name"] == "package"
)
commands.insert(
    package_index,
    {
        "program": "hoplite",
        "name": "inspect",
        "status": "public",
        "availability": "default",
        "summary": (
            "Inspect generated HAB0, exact-manifest, configuration and source-free "
            "evidence without executing application source."
        ),
    },
)

documents = public_surfaces["portable_documents"]
if any(entry["name"] == "hoplite.inspect/0-alpha" for entry in documents):
    raise SystemExit("docs/public-surfaces.json already contains inspection report")
provider_index = next(
    index for index, entry in enumerate(documents) if entry["name"] == "hoplite.provider-hta/1"
)
documents.insert(
    provider_index,
    {
        "name": "hoplite.inspect/0-alpha",
        "path": "docs/diagnostics.md",
        "status": "public",
        "summary": "Path-redacted read-only report for a generated Hoplite application.",
    },
)
public_surfaces_path.write_text(json.dumps(public_surfaces, indent=2) + "\n")

core_boundary_path = ROOT / "docs/core-boundary.json"
core_boundary = json.loads(core_boundary_path.read_text())
sources = core_boundary["rust_sources"]
if any(entry["path"] == "core/src/diagnostics.rs" for entry in sources):
    raise SystemExit("docs/core-boundary.json already contains diagnostics.rs")
sources.append(
    {
        "path": "core/src/diagnostics.rs",
        "status": "core",
        "summary": (
            "Read-only bounded inspection of generated application and configuration "
            "artifacts."
        ),
    }
)
sources.sort(key=lambda entry: entry["path"])
core_boundary_path.write_text(json.dumps(core_boundary, indent=2) + "\n")

for temporary in [WORKFLOW, PAYLOAD, Path(__file__)]:
    temporary.unlink()
