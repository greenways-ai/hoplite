use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// Portable document identity for the current pre-release envelope.
pub const FORMAT: &str = "hoplite.application-bundle/0-alpha";
/// Four-byte marker for the Hoplite-owned alpha envelope.
pub const MAGIC: &[u8; 4] = b"HAB0";
/// Hara-owned alpha bundle marker required inside the envelope.
pub const HARA_BUNDLE_MAGIC: &[u8; 4] = b"HBX0";
/// Numeric embedding ABI compatibility, independent of format maturity.
pub const RUNTIME_ABI_VERSION: u32 = 5;
pub const MAX_MANIFEST_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_BYTECODE_BYTES: usize = 64 * 1024 * 1024;

const CHECKSUM_BYTES: usize = 32;
const PREFIX_BYTES: usize = MAGIC.len() + CHECKSUM_BYTES;
const FIXED_PAYLOAD_BYTES: usize = 4 + CHECKSUM_BYTES + 4;
pub const MAX_BUNDLE_BYTES: usize = PREFIX_BYTES + FIXED_PAYLOAD_BYTES + MAX_BYTECODE_BYTES;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    EmptyManifest,
    ManifestTooLarge { actual: usize },
    EmptyBytecode,
    BytecodeTooLarge { actual: usize },
    BundleTooSmall { actual: usize },
    BundleTooLarge { actual: usize },
    InvalidMagic,
    ChecksumMismatch,
    RuntimeAbiMismatch { actual: u32 },
    ManifestDigestMismatch,
    InvalidBytecodeMagic,
    Truncated,
    TrailingBytes { actual: usize },
    LengthOverflow,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyManifest => {
                formatter.write_str("hoplite/application-manifest-empty")
            }
            Self::ManifestTooLarge { actual } => write!(
                formatter,
                "hoplite/application-manifest-too-large: {actual} bytes exceeds {MAX_MANIFEST_BYTES}"
            ),
            Self::EmptyBytecode => formatter.write_str("hoplite/application-bytecode-empty"),
            Self::BytecodeTooLarge { actual } => write!(
                formatter,
                "hoplite/application-bytecode-too-large: {actual} bytes exceeds {MAX_BYTECODE_BYTES}"
            ),
            Self::BundleTooSmall { actual } => write!(
                formatter,
                "hoplite/application-bundle-truncated: {actual} bytes"
            ),
            Self::BundleTooLarge { actual } => write!(
                formatter,
                "hoplite/application-bundle-too-large: {actual} bytes exceeds {MAX_BUNDLE_BYTES}"
            ),
            Self::InvalidMagic => {
                formatter.write_str("hoplite/application-bundle-invalid: expected HAB0")
            }
            Self::ChecksumMismatch => {
                formatter.write_str("hoplite/application-bundle-checksum-mismatch")
            }
            Self::RuntimeAbiMismatch { actual } => write!(
                formatter,
                "hoplite/application-bundle-incompatible: runtime ABI {actual}, expected {RUNTIME_ABI_VERSION}"
            ),
            Self::ManifestDigestMismatch => {
                formatter.write_str("hoplite/application-manifest-mismatch")
            }
            Self::InvalidBytecodeMagic => formatter.write_str(
                "hoplite/application-bytecode-invalid: expected embedded HBX0 bundle",
            ),
            Self::Truncated => formatter.write_str("hoplite/application-bundle-truncated"),
            Self::TrailingBytes { actual } => write!(
                formatter,
                "hoplite/application-bundle-trailing-bytes: {actual}"
            ),
            Self::LengthOverflow => {
                formatter.write_str("hoplite/application-bundle-length-overflow")
            }
        }
    }
}

impl std::error::Error for Error {}

#[derive(Debug)]
pub enum FileError {
    Io {
        class: &'static str,
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    NotRegular {
        class: &'static str,
        path: PathBuf,
    },
    TooLarge {
        class: &'static str,
        path: PathBuf,
        actual: u64,
        maximum: usize,
    },
    Changed {
        class: &'static str,
        path: PathBuf,
        expected: u64,
        actual: usize,
    },
}

impl fmt::Display for FileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                class,
                operation,
                path,
                source,
            } => write!(
                formatter,
                "hoplite/{class}-file-{operation}: {}: {source}",
                path.display()
            ),
            Self::NotRegular { class, path } => write!(
                formatter,
                "hoplite/{class}-file-not-regular: {}",
                path.display()
            ),
            Self::TooLarge {
                class,
                path,
                actual,
                maximum,
            } => write!(
                formatter,
                "hoplite/{class}-file-too-large: {} is {actual} bytes; maximum is {maximum}",
                path.display()
            ),
            Self::Changed {
                class,
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "hoplite/{class}-file-changed: {} was {expected} bytes and read as {actual}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for FileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoded<'a> {
    bytecode: &'a [u8],
    manifest_digest: [u8; CHECKSUM_BYTES],
}

impl<'a> Decoded<'a> {
    pub fn bytecode(self) -> &'a [u8] {
        self.bytecode
    }

    pub fn manifest_digest(self) -> [u8; CHECKSUM_BYTES] {
        self.manifest_digest
    }
}

pub fn read_bundle_file(path: impl AsRef<Path>) -> Result<Vec<u8>, FileError> {
    read_bounded(path.as_ref(), MAX_BUNDLE_BYTES, "application-bundle")
}

pub fn read_manifest_file(path: impl AsRef<Path>) -> Result<Vec<u8>, FileError> {
    read_bounded(path.as_ref(), MAX_MANIFEST_BYTES, "application-manifest")
}

fn read_bounded(path: &Path, maximum: usize, class: &'static str) -> Result<Vec<u8>, FileError> {
    let path = path.to_path_buf();
    let file = File::open(&path).map_err(|source| FileError::Io {
        class,
        operation: "open",
        path: path.clone(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| FileError::Io {
        class,
        operation: "metadata",
        path: path.clone(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(FileError::NotRegular { class, path });
    }
    if metadata.len() > maximum as u64 {
        return Err(FileError::TooLarge {
            class,
            path,
            actual: metadata.len(),
            maximum,
        });
    }

    let expected = metadata.len();
    let mut bytes = Vec::with_capacity(expected as usize);
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| FileError::Io {
            class,
            operation: "read",
            path: path.clone(),
            source,
        })?;
    if bytes.len() > maximum {
        return Err(FileError::TooLarge {
            class,
            path,
            actual: bytes.len() as u64,
            maximum,
        });
    }
    if bytes.len() as u64 != expected {
        return Err(FileError::Changed {
            class,
            path,
            expected,
            actual: bytes.len(),
        });
    }
    Ok(bytes)
}

pub fn encode(manifest: &[u8], bytecode: &[u8]) -> Result<Vec<u8>, Error> {
    validate_manifest(manifest)?;
    validate_bytecode(bytecode)?;

    let length = u32::try_from(bytecode.len()).map_err(|_| Error::LengthOverflow)?;
    let manifest_digest = digest(manifest);
    let mut payload = Vec::with_capacity(FIXED_PAYLOAD_BYTES + bytecode.len());
    payload.extend_from_slice(&RUNTIME_ABI_VERSION.to_le_bytes());
    payload.extend_from_slice(&manifest_digest);
    payload.extend_from_slice(&length.to_le_bytes());
    payload.extend_from_slice(bytecode);

    let checksum = digest(&payload);
    let mut bundle = Vec::with_capacity(PREFIX_BYTES + payload.len());
    bundle.extend_from_slice(MAGIC);
    bundle.extend_from_slice(&checksum);
    bundle.extend_from_slice(&payload);
    Ok(bundle)
}

pub fn decode<'a>(bundle: &'a [u8], manifest: &[u8]) -> Result<Decoded<'a>, Error> {
    validate_manifest(manifest)?;
    if bundle.len() < PREFIX_BYTES + FIXED_PAYLOAD_BYTES {
        return Err(Error::BundleTooSmall {
            actual: bundle.len(),
        });
    }
    if bundle.len() > MAX_BUNDLE_BYTES {
        return Err(Error::BundleTooLarge {
            actual: bundle.len(),
        });
    }
    if &bundle[..MAGIC.len()] != MAGIC {
        return Err(Error::InvalidMagic);
    }

    let expected_checksum: [u8; CHECKSUM_BYTES] =
        bundle[MAGIC.len()..PREFIX_BYTES].try_into().unwrap();
    let payload = &bundle[PREFIX_BYTES..];
    if digest(payload) != expected_checksum {
        return Err(Error::ChecksumMismatch);
    }

    let mut input = payload;
    let runtime_abi = take_u32(&mut input)?;
    if runtime_abi != RUNTIME_ABI_VERSION {
        return Err(Error::RuntimeAbiMismatch {
            actual: runtime_abi,
        });
    }

    let manifest_digest: [u8; CHECKSUM_BYTES] =
        take(&mut input, CHECKSUM_BYTES)?.try_into().unwrap();
    if digest(manifest) != manifest_digest {
        return Err(Error::ManifestDigestMismatch);
    }

    let bytecode_len = take_u32(&mut input)? as usize;
    if bytecode_len == 0 {
        return Err(Error::EmptyBytecode);
    }
    if bytecode_len > MAX_BYTECODE_BYTES {
        return Err(Error::BytecodeTooLarge {
            actual: bytecode_len,
        });
    }
    let bytecode = take(&mut input, bytecode_len)?;
    if !input.is_empty() {
        return Err(Error::TrailingBytes {
            actual: input.len(),
        });
    }
    validate_bytecode(bytecode)?;
    Ok(Decoded {
        bytecode,
        manifest_digest,
    })
}

fn validate_manifest(manifest: &[u8]) -> Result<(), Error> {
    if manifest.is_empty() {
        return Err(Error::EmptyManifest);
    }
    if manifest.len() > MAX_MANIFEST_BYTES {
        return Err(Error::ManifestTooLarge {
            actual: manifest.len(),
        });
    }
    Ok(())
}

fn validate_bytecode(bytecode: &[u8]) -> Result<(), Error> {
    if bytecode.is_empty() {
        return Err(Error::EmptyBytecode);
    }
    if bytecode.len() > MAX_BYTECODE_BYTES {
        return Err(Error::BytecodeTooLarge {
            actual: bytecode.len(),
        });
    }
    if bytecode.len() < HARA_BUNDLE_MAGIC.len()
        || &bytecode[..HARA_BUNDLE_MAGIC.len()] != HARA_BUNDLE_MAGIC
    {
        return Err(Error::InvalidBytecodeMagic);
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> [u8; CHECKSUM_BYTES] {
    Sha256::digest(bytes).into()
}

fn take_u32(input: &mut &[u8]) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(take(input, 4)?.try_into().unwrap()))
}

fn take<'a>(input: &mut &'a [u8], length: usize) -> Result<&'a [u8], Error> {
    if input.len() < length {
        return Err(Error::Truncated);
    }
    let (value, remaining) = input.split_at(length);
    *input = remaining;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hbx0() -> Vec<u8> {
        b"HBX0deterministic-bytecode".to_vec()
    }

    fn reseal(bundle: &mut [u8]) {
        let checksum = digest(&bundle[PREFIX_BYTES..]);
        bundle[MAGIC.len()..PREFIX_BYTES].copy_from_slice(&checksum);
    }

    #[test]
    fn round_trip_is_deterministic_and_manifest_bound() {
        let manifest = b"exact-route-manifest";
        let bytecode = hbx0();
        let first = encode(manifest, &bytecode).unwrap();
        let second = encode(manifest, &bytecode).unwrap();
        assert_eq!(first, second);
        assert_eq!(&first[..4], MAGIC);
        let decoded = decode(&first, manifest).unwrap();
        assert_eq!(decoded.bytecode(), bytecode);
        assert_eq!(decoded.manifest_digest(), digest(manifest));
    }

    #[test]
    fn rejects_manifest_drift_before_exposing_bytecode() {
        let bundle = encode(b"manifest-a", &hbx0()).unwrap();
        assert_eq!(
            decode(&bundle, b"manifest-b"),
            Err(Error::ManifestDigestMismatch)
        );
    }

    #[test]
    fn rejects_runtime_abi_drift_even_with_a_valid_checksum() {
        let previous = RUNTIME_ABI_VERSION - 1;
        let mut bundle = encode(b"manifest", &hbx0()).unwrap();
        bundle[PREFIX_BYTES..PREFIX_BYTES + 4].copy_from_slice(&previous.to_le_bytes());
        reseal(&mut bundle);
        assert_eq!(
            decode(&bundle, b"manifest"),
            Err(Error::RuntimeAbiMismatch { actual: previous })
        );
    }

    #[test]
    fn rejects_tampering_and_trailing_bytes() {
        let mut tampered = encode(b"manifest", &hbx0()).unwrap();
        *tampered.last_mut().unwrap() ^= 1;
        assert_eq!(decode(&tampered, b"manifest"), Err(Error::ChecksumMismatch));

        let mut trailing = encode(b"manifest", &hbx0()).unwrap();
        trailing.push(0);
        reseal(&mut trailing);
        assert_eq!(
            decode(&trailing, b"manifest"),
            Err(Error::TrailingBytes { actual: 1 })
        );
    }

    #[test]
    fn rejects_empty_oversized_and_non_hbx0_inputs() {
        assert_eq!(encode(b"", &hbx0()), Err(Error::EmptyManifest));
        assert_eq!(encode(b"manifest", b""), Err(Error::EmptyBytecode));
        assert_eq!(
            encode(b"manifest", b"not-bytecode"),
            Err(Error::InvalidBytecodeMagic)
        );

        let manifest = vec![0; MAX_MANIFEST_BYTES + 1];
        assert_eq!(
            encode(&manifest, &hbx0()),
            Err(Error::ManifestTooLarge {
                actual: MAX_MANIFEST_BYTES + 1
            })
        );
    }

    #[test]
    fn rejects_pre_reset_bundle_markers() {
        let manifest = b"manifest";

        let mut old_outer = encode(manifest, &hbx0()).unwrap();
        let mut old_outer_magic = *MAGIC;
        old_outer_magic[3] = b'1';
        old_outer[..MAGIC.len()].copy_from_slice(&old_outer_magic);
        assert_eq!(decode(&old_outer, manifest), Err(Error::InvalidMagic));

        let mut old_inner = HARA_BUNDLE_MAGIC.to_vec();
        old_inner[2] = b'B';
        old_inner[3] = b'2';
        old_inner.extend_from_slice(b"legacy-bytecode");
        assert_eq!(
            encode(manifest, &old_inner),
            Err(Error::InvalidBytecodeMagic)
        );
    }
    #[test]
    fn bounded_file_reads_preserve_exact_bytes_and_reject_oversize() {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "hoplite-hab0-file-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, b"exact").unwrap();
        assert_eq!(read_bounded(&path, 5, "test").unwrap(), b"exact");
        assert!(matches!(
            read_bounded(&path, 4, "test"),
            Err(FileError::TooLarge {
                actual: 5,
                maximum: 4,
                ..
            })
        ));
        std::fs::remove_file(path).unwrap();
    }
}
