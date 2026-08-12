from pathlib import Path
import subprocess

MIGRATION_FILES = {
    Path(".github/alpha-version-migration.py"),
    Path(".github/workflows/alpha-version-migration-v2.yml"),
}

REPLACEMENTS = [
    ("hoplite.application-bundle/1", "hoplite.application-bundle/0-alpha"),
    ("HAB1", "HAB0"),
    ("hab1", "hab0"),
    ("HBB2", "HBX0"),
    ("hbb2", "hbx0"),
    ("Hoplite 0.2", "Hoplite alpha"),
    ("0.2 release line", "alpha release line"),
    ("0.2 release", "alpha release"),
    ("0.2 line", "alpha line"),
    ("0.2 acceptance", "alpha acceptance"),
    (
        "494b17a39c952f24a68bfeda1bd66283a80f735c",
        "6dd73bbb9ba546b90dee0a075323998dd3abac85",
    ),
]

counts = {old: 0 for old, _ in REPLACEMENTS[:5]}

for raw in subprocess.check_output(["git", "ls-files", "-z"]).split(b"\0"):
    if not raw:
        continue
    path = Path(raw.decode())
    if path in MIGRATION_FILES:
        continue
    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        continue
    original = text
    for old, new in REPLACEMENTS:
        found = text.count(old)
        if old in counts:
            counts[old] += found
        text = text.replace(old, new)
    if text != original:
        path.write_text(text, encoding="utf-8")

for required in ("hoplite.application-bundle/1", "HAB1", "HBB2"):
    if counts[required] == 0:
        raise SystemExit(f"expected stale alpha-reset marker was not found: {required}")

bundle_path = Path("core/application-bundle/src/lib.rs")
bundle = bundle_path.read_text()
old_constants = "\n".join(
    [
        'pub const FORMAT: &str = "hoplite.application-bundle/0-alpha";',
        'pub const MAGIC: &[u8; 4] = b"HAB0";',
        "pub const RUNTIME_ABI_VERSION: u32 = 4;",
    ]
)
new_constants = "\n".join(
    [
        "/// Portable document identity for the current pre-release envelope.",
        'pub const FORMAT: &str = "hoplite.application-bundle/0-alpha";',
        "/// Four-byte marker for the Hoplite-owned alpha envelope.",
        'pub const MAGIC: &[u8; 4] = b"HAB0";',
        "/// Hara-owned alpha bundle marker required inside the envelope.",
        'pub const HARA_BUNDLE_MAGIC: &[u8; 4] = b"HBX0";',
        "/// Numeric embedding ABI compatibility, independent of format maturity.",
        "pub const RUNTIME_ABI_VERSION: u32 = 4;",
    ]
)
if old_constants not in bundle:
    raise SystemExit("application-bundle constants did not match")
bundle = bundle.replace(old_constants, new_constants, 1)

old_validator = 'if bytecode.len() < 4 || &bytecode[..4] != b"HBX0" {'
new_validator = "\n".join(
    [
        "if bytecode.len() < HARA_BUNDLE_MAGIC.len()",
        "        || &bytecode[..HARA_BUNDLE_MAGIC.len()] != HARA_BUNDLE_MAGIC",
        "    {",
    ]
)
if old_validator not in bundle:
    raise SystemExit("inner bundle validator was not found")
bundle = bundle.replace(old_validator, new_validator, 1)

marker = "\n".join(
    [
        "    #[test]",
        "    fn bounded_file_reads_preserve_exact_bytes_and_reject_oversize() {",
    ]
)
negative_test = "\n".join(
    [
        "    #[test]",
        "    fn rejects_pre_reset_bundle_markers() {",
        '        let manifest = b"manifest";',
        "",
        "        let mut old_outer = encode(manifest, &hbx0()).unwrap();",
        "        let mut old_outer_magic = *MAGIC;",
        "        old_outer_magic[3] = b'1';",
        "        old_outer[..MAGIC.len()].copy_from_slice(&old_outer_magic);",
        "        assert_eq!(decode(&old_outer, manifest), Err(Error::InvalidMagic));",
        "",
        "        let mut old_inner = HARA_BUNDLE_MAGIC.to_vec();",
        "        old_inner[2] = b'B';",
        "        old_inner[3] = b'2';",
        '        old_inner.extend_from_slice(b"legacy-bytecode");',
        "        assert_eq!(",
        "            encode(manifest, &old_inner),",
        "            Err(Error::InvalidBytecodeMagic)",
        "        );",
        "    }",
        "",
    ]
)
if marker not in bundle:
    raise SystemExit("bundle test insertion point was not found")
bundle_path.write_text(bundle.replace(marker, negative_test + marker, 1))

runtime_path = Path("core/runtime/src/lib.rs")
runtime = runtime_path.read_text()
running_arm = "\n".join(
    [
        "            vm::VmFiberState::Running => {",
        "                self.reject_work(",
        "                    work,",
        '                    error_value("fiber/invalid-state", "running fiber escaped".into()),',
        "                );",
        "            }",
    ]
)
yielded_and_running = "\n".join(
    [
        "            vm::VmFiberState::Yielded(_) => {",
        "                self.reject_work(",
        "                    work,",
        "                    error_value(",
        '                        "fiber/yield-unsupported",',
        '                        "yielded outside a coroutine driver".into(),',
        "                    ),",
        "                );",
        "            }",
        running_arm,
    ]
)
if runtime.count(running_arm) != 1:
    raise SystemExit("runtime running-state arm did not match exactly once")
runtime_path.write_text(runtime.replace(running_arm, yielded_and_running, 1))

header_path = Path("core/nginx/hoplite_runtime.h")
header = header_path.read_text()
old_header = "\n".join(
    [
        "/*",
        " * Validate one HAB0 bundle against the exact route manifest, load its embedded",
        " * HBX0 modules, and prepare every app route as one transactional startup.",
        " */",
    ]
)
new_header = "\n".join(
    [
        "/*",
        " * The `_v1` suffix on these exported bootstrap symbols identifies the first C",
        " * function shape. It is independent of the alpha document epoch below.",
        " *",
        " * Validate one HAB0 bundle against the exact route manifest, load its embedded",
        " * HBX0 modules, and prepare every app route as one transactional startup.",
        " */",
    ]
)
if old_header not in header:
    raise SystemExit("runtime bootstrap comment was not found")
header_path.write_text(header.replace(old_header, new_header, 1))

cargo_path = Path("core/application-bundle/Cargo.toml")
cargo = cargo_path.read_text()
old_description = 'description = "Versioned application bundle envelope for Hoplite"'
new_description = 'description = "Pre-release alpha application bundle envelope for Hoplite"'
if old_description not in cargo:
    raise SystemExit("application-bundle Cargo description did not match")
cargo_path.write_text(cargo.replace(old_description, new_description, 1))

for path in MIGRATION_FILES:
    path.unlink(missing_ok=True)
