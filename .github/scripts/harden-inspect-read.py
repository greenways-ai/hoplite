from pathlib import Path

root = Path(__file__).resolve().parents[2]
source_path = root / "core/src/diagnostics.rs"
source = source_path.read_text()

old_imports = "use std::fs;\nuse std::io;"
new_imports = "use std::fs::{self, File};\nuse std::io::{self, Read};"
if source.count(old_imports) != 1:
    raise SystemExit("unexpected diagnostics import block")
source = source.replace(old_imports, new_imports, 1)

old_function = '''fn inspect_optional_artifact(path: &Path, label: &'static str) -> Result<Artifact, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Artifact::absent()),
        Err(error) => {
            return Err(format!(
                "hoplite/inspect-{label}-metadata: {}",
                io_kind(&error)
            ))
        }
    };
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
    let bytes = fs::read(path)
        .map_err(|error| format!("hoplite/inspect-{label}-read: {}", io_kind(&error)))?;
    if bytes.len() as u64 != metadata.len() {
        return Err(format!(
            "hoplite/inspect-{label}-changed: expected {} bytes, read {}",
            metadata.len(),
            bytes.len()
        ));
    }
    Ok(Artifact {
        present: true,
        bytes: Some(bytes.len()),
        sha256: Some(digest_hex(&bytes)),
    })
}
'''

new_function = '''fn inspect_optional_artifact(path: &Path, label: &'static str) -> Result<Artifact, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Artifact::absent()),
        Err(error) => {
            return Err(format!(
                "hoplite/inspect-{label}-open: {}",
                io_kind(&error)
            ))
        }
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
'''

if source.count(old_function) != 1:
    raise SystemExit("unexpected inspect_optional_artifact implementation")
source_path.write_text(source.replace(old_function, new_function, 1))

(root / ".github/workflows/harden-inspect-read.yml").unlink()
Path(__file__).unlink()
