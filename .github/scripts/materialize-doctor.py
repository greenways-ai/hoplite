from __future__ import annotations

import base64
import gzip
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/materialize-doctor.yml"
PAYLOAD = ROOT / ".github/scripts/doctor.rs.gz.b64"
DOCTOR_DOC = '''

## Diagnosing the local runtime

`hoplite doctor` checks the complete local Hoplite serving environment without
starting Nginx:

```shell
hoplite doctor [--json] [--show-paths] [--deep] [--strict] [PROJECT]
```

The default pass is read-only and does not evaluate application source. It
checks:

- the supported host operating system and current Hoplite executable;
- the selected Nginx distribution, executable permission, and `nginx -v` probe;
- availability of the separate source-free `hoplite-server` executable;
- a trusted CA bundle for secure static upstreams;
- `project.edn`, the default Hoplite profile, qualified main Var, HAL source
  discovery, and `:host/nginx` capability;
- platform module and exact package-lock consistency;
- generated HAB0, exact-manifest, route-count, and source-free evidence when a
  build is present.

`--deep` explicitly authorizes source compilation, application Var evaluation,
HAB0 construction, and the full application/platform preflight. `--strict`
turns warnings, such as a project not yet being built or a development build
containing `app.hal`, into a non-zero result.

Human-readable output is the default. `--json` emits
`hoplite.doctor/0-alpha`. Required failures always produce a non-zero exit.
Warnings remain visible but non-fatal unless `--strict` is supplied.

Paths are redacted unless `--show-paths` is supplied. Underlying project,
compiler, environment, or process errors are mapped to stable
`hoplite/doctor-*` classes rather than printing build-machine paths, command
arguments, credentials, signatures, source text, or native pointers.
'''


def replace_once(path: str, old: str, new: str) -> None:
    target = ROOT / path
    source = target.read_text()
    count = source.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one replacement target, found {count}")
    target.write_text(source.replace(old, new, 1))


doctor = gzip.decompress(base64.b64decode(PAYLOAD.read_text())).decode()
(ROOT / "core/src/doctor.rs").write_text(doctor)

replace_once(
    "core/src/main.rs",
    "mod diagnostics;\nmod host;",
    "mod diagnostics;\nmod doctor;\nmod host;",
)
replace_once(
    "core/src/main.rs",
    '        Some("serve") => run_serve_command(&arguments[1..])?,\n'
    '        Some("inspect") => diagnostics::run(&arguments[1..])?,',
    '        Some("serve") => run_serve_command(&arguments[1..])?,\n'
    '        Some("doctor") => doctor::run(&arguments[1..])?,\n'
    '        Some("inspect") => diagnostics::run(&arguments[1..])?,',
)
replace_once(
    "core/src/main.rs",
    '    println!("  hoplite run FILE");\n'
    '    println!("  hoplite inspect [--json] [--show-paths] [PROJECT|OUTPUT|BUNDLE]");',
    '    println!("  hoplite run FILE");\n'
    '    println!("  hoplite doctor [--json] [--show-paths] [--deep] [--strict] [PROJECT]");\n'
    '    println!("  hoplite inspect [--json] [--show-paths] [PROJECT|OUTPUT|BUNDLE]");',
)

health_anchor = '''struct ResolvedPaths {
    output: PathBuf,
    bundle: PathBuf,
    manifest: PathBuf,
}

pub fn run(arguments: &[String]) -> Result<(), String> {'''
health_replacement = '''struct ResolvedPaths {
    output: PathBuf,
    bundle: PathBuf,
    manifest: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Health {
    pub(crate) applications: usize,
    pub(crate) routes: usize,
    pub(crate) source_inputs: usize,
}

pub(crate) fn health(target: &Path) -> Result<Health, String> {
    let inspection = inspect_target(target, None, false)?;
    Ok(Health {
        applications: inspection.manifest.applications,
        routes: inspection.manifest.routes,
        source_inputs: inspection.source_inputs.count,
    })
}

pub fn run(arguments: &[String]) -> Result<(), String> {'''
replace_once("core/src/diagnostics.rs", health_anchor, health_replacement)

replace_once(
    "README.md",
    '`hoplite.inspect/0-alpha` report. Verify a built application without executing it\n'
    'with `hoplite verify .`.',
    '`hoplite.inspect/0-alpha` report. Verify a built application without executing it\n'
    'with `hoplite verify .`. Diagnose the complete local project, Nginx, trust,\n'
    'package-lock, and generated-output environment with `hoplite doctor .`; add\n'
    '`--deep` only when source compilation and application preflight are intended.',
)

with (ROOT / "docs/diagnostics.md").open("a") as output:
    output.write(DOCTOR_DOC)

replace_once(
    "docs/public-api.md",
    'The public `hoplite` control CLI supports `repl`, `eval`, `run`, `inspect`,\n'
    '`verify`, `package`, `serve`, and `version`.',
    'The public `hoplite` control CLI supports `repl`, `eval`, `run`, `doctor`,\n'
    '`inspect`, `verify`, `package`, `serve`, and `version`.',
)
replace_once(
    "docs/public-api.md",
    'Its JSON identity is `hoplite.inspect/0-alpha`; see\n'
    '[Diagnostics](diagnostics.md).\n\n'
    '`hoplite-server` consumes generated `.hoplite` output',
    'Its JSON identity is `hoplite.inspect/0-alpha`; see\n'
    '[Diagnostics](diagnostics.md).\n\n'
    '`hoplite doctor` diagnoses the executable runtime, Nginx, CA trust, project\n'
    'profile and source declarations, package-lock/platform consistency, and any\n'
    'generated HAB0 application. Its default pass does not execute source; `--deep`\n'
    'explicitly performs compilation and application preflight. Paths remain redacted\n'
    'unless `--show-paths` is supplied. Its JSON identity is\n'
    '`hoplite.doctor/0-alpha`.\n\n'
    '`hoplite-server` consumes generated `.hoplite` output',
)

replace_once(
    "docs/versioning.md",
    '| Hoplite application bundle | `hoplite.application-bundle/0-alpha` / `HAB0` |\n'
    '| Hoplite inspection report | `hoplite.inspect/0-alpha` |',
    '| Hoplite application bundle | `hoplite.application-bundle/0-alpha` / `HAB0` |\n'
    '| Hoplite doctor report | `hoplite.doctor/0-alpha` |\n'
    '| Hoplite inspection report | `hoplite.inspect/0-alpha` |',
)

public_surfaces_path = ROOT / "docs/public-surfaces.json"
public_surfaces = json.loads(public_surfaces_path.read_text())
commands = public_surfaces["cli_commands"]
if any(entry["program"] == "hoplite" and entry["name"] == "doctor" for entry in commands):
    raise SystemExit("docs/public-surfaces.json already contains hoplite doctor")
eval_index = next(
    index
    for index, entry in enumerate(commands)
    if entry["program"] == "hoplite" and entry["name"] == "eval"
)
commands.insert(
    eval_index,
    {
        "program": "hoplite",
        "name": "doctor",
        "status": "public",
        "availability": "default",
        "summary": (
            "Diagnose the local project, runtime, Nginx, trust, package-lock and "
            "generated-application environment with path-redacted results."
        ),
    },
)

documents = public_surfaces["portable_documents"]
if any(entry["name"] == "hoplite.doctor/0-alpha" for entry in documents):
    raise SystemExit("docs/public-surfaces.json already contains doctor report")
inspect_index = next(
    index for index, entry in enumerate(documents) if entry["name"] == "hoplite.inspect/0-alpha"
)
documents.insert(
    inspect_index,
    {
        "name": "hoplite.doctor/0-alpha",
        "path": "docs/diagnostics.md",
        "status": "public",
        "summary": "Path-redacted local runtime health and readiness report.",
    },
)
public_surfaces_path.write_text(json.dumps(public_surfaces, indent=2) + "\n")

core_boundary_path = ROOT / "docs/core-boundary.json"
core_boundary = json.loads(core_boundary_path.read_text())
sources = core_boundary["rust_sources"]
if any(entry["path"] == "core/src/doctor.rs" for entry in sources):
    raise SystemExit("docs/core-boundary.json already contains doctor.rs")
sources.append(
    {
        "path": "core/src/doctor.rs",
        "status": "core",
        "summary": (
            "Path-redacted local runtime diagnostics with explicit deep source preflight."
        ),
    }
)
sources.sort(key=lambda entry: entry["path"])
core_boundary_path.write_text(json.dumps(core_boundary, indent=2) + "\n")

for temporary in [WORKFLOW, PAYLOAD, Path(__file__)]:
    temporary.unlink()
