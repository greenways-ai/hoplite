#![forbid(unsafe_code)]

//! Read-only verified access to the immutable filesystem object layout used by
//! Hoplite blob providers.
//!
//! This package owns the physical `objects/sha256` layout, `HBO1` metadata,
//! shared `store.lock` coordination, bounded actual reads and SHA-256
//! verification. It does not stage, install, stream, decode or interpret an
//! object value.

use fs2::FileExt;
use hoplite_blob_store::Digest;
use sha2::{Digest as Sha2Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

const OBJECT_MAGIC: &[u8; 4] = b"HBO1";
const LOCK_FILE: &str = "store.lock";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_object_bytes: usize,
    pub max_media_type_bytes: usize,
    pub io_chunk_bytes: usize,
}

impl Limits {
    pub fn validate(self) -> Result<Self, Error> {
        if self.max_object_bytes == 0 {
            return Err(Error::InvalidLimits("max_object_bytes must be positive"));
        }
        if self.max_media_type_bytes == 0 {
            return Err(Error::InvalidLimits(
                "max_media_type_bytes must be positive",
            ));
        }
        if self.io_chunk_bytes == 0 || self.io_chunk_bytes > self.max_object_bytes {
            return Err(Error::InvalidLimits(
                "io_chunk_bytes must be positive and no greater than max_object_bytes",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Missing,
    Maximum,
    Digest,
    Provider,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedObject {
    digest: Digest,
    bytes: Vec<u8>,
}

impl VerifiedObject {
    fn new(digest: Digest, bytes: Vec<u8>) -> Self {
        Self { digest, bytes }
    }

    pub const fn digest(&self) -> Digest {
        self.digest
    }

    pub fn byte_length(&self) -> usize {
        self.bytes.len()
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[derive(Debug)]
pub enum Error {
    InvalidLimits(&'static str),
    Installation { code: &'static str, detail: String },
    Poisoned,
}

impl Error {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidLimits(_) => "blob-filesystem-reader-limits-invalid",
            Self::Installation { code, .. } => code,
            Self::Poisoned => "blob-filesystem-reader-lock-poisoned",
        }
    }

    fn installation(code: &'static str, detail: impl Into<String>) -> Self {
        Self::Installation {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLimits(message) => {
                write!(
                    formatter,
                    "invalid blob filesystem reader limits: {message}"
                )
            }
            Self::Installation { code, detail } => write!(formatter, "{code}: {detail}"),
            Self::Poisoned => formatter.write_str("blob-filesystem-reader-lock-poisoned"),
        }
    }
}

impl std::error::Error for Error {}

pub struct FilesystemObjectReader {
    root: PathBuf,
    objects_dir: PathBuf,
    lock_path: PathBuf,
    limits: Limits,
    process_lock: Mutex<()>,
}

impl fmt::Debug for FilesystemObjectReader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FilesystemObjectReader")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl FilesystemObjectReader {
    pub fn open(root: impl AsRef<Path>, limits: Limits) -> Result<Self, Error> {
        let limits = limits.validate()?;
        let root = canonical_real_directory(root.as_ref(), "blob-filesystem-reader-root-invalid")?;
        let objects_root = real_directory(
            &root.join("objects"),
            "blob-filesystem-reader-objects-invalid",
        )?;
        let objects_dir = real_directory(
            &objects_root.join("sha256"),
            "blob-filesystem-reader-sha256-root-invalid",
        )?;
        let lock_path = root.join(LOCK_FILE);
        require_regular_file(&lock_path, "blob-filesystem-reader-lock-invalid")?;
        Ok(Self {
            root,
            objects_dir,
            lock_path,
            limits,
            process_lock: Mutex::new(()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub const fn limits(&self) -> Limits {
        self.limits
    }

    pub fn read_verified(
        &self,
        digest: Digest,
        max_bytes: usize,
    ) -> Result<VerifiedObject, Failure> {
        if max_bytes == 0 || max_bytes > self.limits.max_object_bytes {
            return Err(Failure::Maximum);
        }
        let _guard = self.shared().map_err(|_| Failure::Provider)?;
        let (metadata_path, data_path) = self.object_paths(digest);
        let object_directory = metadata_path.parent().ok_or(Failure::Provider)?;
        match fs::symlink_metadata(object_directory) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(Failure::Provider);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(Failure::Missing);
            }
            Err(_) => return Err(Failure::Provider),
        }

        let metadata_exists = regular_file_exists(&metadata_path).map_err(|_| Failure::Provider)?;
        let data_exists = regular_file_exists(&data_path).map_err(|_| Failure::Provider)?;
        match (metadata_exists, data_exists) {
            (false, false) => return Err(Failure::Missing),
            (false, true) | (true, false) => return Err(Failure::Provider),
            (true, true) => {}
        }

        let metadata = self
            .read_object_metadata(&metadata_path)
            .map_err(|_| Failure::Provider)?;
        if metadata.digest != digest {
            return Err(Failure::Digest);
        }
        if metadata.size == 0 {
            return Err(Failure::Provider);
        }
        if metadata.size > max_bytes as u64 {
            return Err(Failure::Maximum);
        }

        let bytes = read_bounded_file(&data_path, max_bytes, self.limits.io_chunk_bytes)?;
        if bytes.len() as u64 != metadata.size {
            return Err(Failure::Provider);
        }
        let actual = Sha256::digest(&bytes);
        let mut actual_bytes = [0_u8; 32];
        actual_bytes.copy_from_slice(&actual);
        if &actual_bytes != digest.bytes() {
            return Err(Failure::Digest);
        }
        Ok(VerifiedObject::new(digest, bytes))
    }

    fn shared(&self) -> Result<OperationGuard<'_>, Error> {
        let process = self.process_lock.lock().map_err(|_| Error::Poisoned)?;
        require_regular_file(&self.lock_path, "blob-filesystem-reader-lock-invalid")?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.lock_path)
            .map_err(|error| io_error("blob-filesystem-reader-lock-open", error))?;
        FileExt::lock_shared(&file)
            .map_err(|error| io_error("blob-filesystem-reader-lock-acquire", error))?;
        Ok(OperationGuard {
            _process: process,
            file,
        })
    }

    fn object_paths(&self, digest: Digest) -> (PathBuf, PathBuf) {
        let value = digest.to_string();
        let hex = value
            .strip_prefix("sha256:")
            .expect("Digest display always uses sha256 prefix");
        let directory = self.objects_dir.join(&hex[..2]);
        let stem = &hex[2..];
        (
            directory.join(format!("{stem}.meta")),
            directory.join(format!("{stem}.blob")),
        )
    }

    fn read_object_metadata(&self, path: &Path) -> Result<ObjectMetadata, Error> {
        let maximum = 4_usize
            .checked_add(32)
            .and_then(|value| value.checked_add(8))
            .and_then(|value| value.checked_add(4))
            .and_then(|value| value.checked_add(self.limits.max_media_type_bytes))
            .ok_or_else(|| {
                Error::installation(
                    "blob-filesystem-reader-metadata-limit",
                    "object metadata limit overflow",
                )
            })?;
        let bytes = read_metadata_file(path, maximum)?;
        ObjectMetadata::decode(&bytes, self.limits.max_media_type_bytes)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ObjectMetadata {
    digest: Digest,
    size: u64,
}

impl ObjectMetadata {
    fn decode(bytes: &[u8], max_media_type_bytes: usize) -> Result<Self, Error> {
        let mut reader = MetadataReader::new(bytes);
        reader.expect_magic(OBJECT_MAGIC)?;
        let digest = Digest::from_bytes(reader.array_32()?);
        let size = reader.u64()?;
        let media_type = reader.sized_bytes(max_media_type_bytes)?;
        reader.finish()?;
        validate_media_type(media_type)?;
        Ok(Self { digest, size })
    }
}

struct MetadataReader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> MetadataReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], Error> {
        let end = self.cursor.checked_add(length).ok_or_else(|| {
            Error::installation(
                "blob-filesystem-reader-metadata-overflow",
                "metadata length overflow",
            )
        })?;
        let value = self.bytes.get(self.cursor..end).ok_or_else(|| {
            Error::installation(
                "blob-filesystem-reader-metadata-truncated",
                "metadata is truncated",
            )
        })?;
        self.cursor = end;
        Ok(value)
    }

    fn expect_magic(&mut self, expected: &[u8; 4]) -> Result<(), Error> {
        if self.take(4)? == expected {
            Ok(())
        } else {
            Err(Error::installation(
                "blob-filesystem-reader-metadata-magic",
                "object metadata has an unsupported format",
            ))
        }
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let bytes: [u8; 4] = self.take(4)?.try_into().map_err(|_| {
            Error::installation(
                "blob-filesystem-reader-metadata-u32",
                "invalid metadata u32",
            )
        })?;
        Ok(u32::from_be_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        let bytes: [u8; 8] = self.take(8)?.try_into().map_err(|_| {
            Error::installation(
                "blob-filesystem-reader-metadata-u64",
                "invalid metadata u64",
            )
        })?;
        Ok(u64::from_be_bytes(bytes))
    }

    fn array_32(&mut self) -> Result<[u8; 32], Error> {
        self.take(32)?.try_into().map_err(|_| {
            Error::installation(
                "blob-filesystem-reader-metadata-digest",
                "invalid metadata digest",
            )
        })
    }

    fn sized_bytes(&mut self, limit: usize) -> Result<&'a [u8], Error> {
        let length = usize::try_from(self.u32()?).map_err(|_| {
            Error::installation(
                "blob-filesystem-reader-metadata-text-length",
                "metadata text length does not fit usize",
            )
        })?;
        if length == 0 || length > limit {
            return Err(Error::installation(
                "blob-filesystem-reader-metadata-text-bounds",
                "metadata text exceeds configured bounds",
            ));
        }
        self.take(length)
    }

    fn finish(self) -> Result<(), Error> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(Error::installation(
                "blob-filesystem-reader-metadata-trailing",
                "metadata contains trailing bytes",
            ))
        }
    }
}

struct OperationGuard<'a> {
    _process: MutexGuard<'a, ()>,
    file: File,
}

impl Drop for OperationGuard<'_> {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn canonical_real_directory(path: &Path, code: &'static str) -> Result<PathBuf, Error> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(code, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(Error::installation(
            code,
            "trusted provider path is not a real directory",
        ));
    }
    let canonical = fs::canonicalize(path).map_err(|error| io_error(code, error))?;
    let metadata = fs::symlink_metadata(&canonical).map_err(|error| io_error(code, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(Error::installation(
            code,
            "trusted provider path does not resolve to a real directory",
        ));
    }
    Ok(canonical)
}

fn real_directory(path: &Path, code: &'static str) -> Result<PathBuf, Error> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(code, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(Error::installation(
            code,
            "provider-owned path is not a real directory",
        ));
    }
    Ok(path.to_path_buf())
}

fn require_regular_file(path: &Path, code: &'static str) -> Result<(), Error> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(code, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(Error::installation(
            code,
            "provider-owned path is not a regular file",
        ));
    }
    Ok(())
}

fn regular_file_exists(path: &Path) -> Result<bool, Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(Error::installation(
                "blob-filesystem-reader-object-path-invalid",
                "provider-owned object path is not a regular file",
            ))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error("blob-filesystem-reader-object-stat", error)),
    }
}

fn read_metadata_file(path: &Path, maximum: usize) -> Result<Vec<u8>, Error> {
    require_regular_file(path, "blob-filesystem-reader-metadata-invalid")?;
    let declared = fs::metadata(path)
        .map_err(|error| io_error("blob-filesystem-reader-metadata-stat", error))?
        .len();
    if declared == 0 || declared > maximum as u64 {
        return Err(Error::installation(
            "blob-filesystem-reader-metadata-bounds",
            "object metadata exceeds its bounded profile",
        ));
    }
    let mut file = File::open(path)
        .map_err(|error| io_error("blob-filesystem-reader-metadata-open", error))?;
    let mut bytes = Vec::with_capacity(declared as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| io_error("blob-filesystem-reader-metadata-read", error))?;
    if bytes.len() as u64 != declared || bytes.len() > maximum {
        return Err(Error::installation(
            "blob-filesystem-reader-metadata-race",
            "object metadata length changed while reading",
        ));
    }
    Ok(bytes)
}

fn read_bounded_file(path: &Path, maximum: usize, chunk_bytes: usize) -> Result<Vec<u8>, Failure> {
    if !regular_file_exists(path).map_err(|_| Failure::Provider)? {
        return Err(Failure::Provider);
    }
    let declared = fs::metadata(path).map_err(|_| Failure::Provider)?.len();
    if declared > maximum as u64 {
        return Err(Failure::Maximum);
    }
    let mut file = File::open(path).map_err(|_| Failure::Provider)?;
    let sentinel = maximum.checked_add(1).ok_or(Failure::Maximum)?;
    let capacity = usize::try_from(declared).unwrap_or(maximum).min(maximum);
    let mut bytes = Vec::with_capacity(capacity);
    let mut buffer = vec![0_u8; chunk_bytes.min(sentinel).max(1)];
    while bytes.len() < sentinel {
        let capacity = buffer.len().min(sentinel - bytes.len());
        let read = file
            .read(&mut buffer[..capacity])
            .map_err(|_| Failure::Provider)?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() > maximum {
            return Err(Failure::Maximum);
        }
    }
    let mut extra = [0_u8; 1];
    if file.read(&mut extra).map_err(|_| Failure::Provider)? != 0 {
        return Err(Failure::Maximum);
    }
    if bytes.len() as u64 != declared {
        return Err(Failure::Provider);
    }
    Ok(bytes)
}

fn validate_media_type(bytes: &[u8]) -> Result<(), Error> {
    let value = std::str::from_utf8(bytes).map_err(|_| {
        Error::installation(
            "blob-filesystem-reader-metadata-media-type",
            "object media type is not UTF-8",
        )
    })?;
    let Some((type_name, subtype)) = value.split_once('/') else {
        return Err(Error::installation(
            "blob-filesystem-reader-metadata-media-type",
            "object media type is invalid",
        ));
    };
    let token = |character: char| {
        character.is_ascii_alphanumeric()
            || matches!(
                character,
                '!' | '#' | '$' | '&' | '\'' | '*' | '+' | '-' | '.' | '^' | '_' | '`' | '|' | '~'
            )
    };
    if type_name.is_empty()
        || subtype.is_empty()
        || subtype.contains('/')
        || !type_name.chars().all(token)
        || !subtype.chars().all(token)
    {
        return Err(Error::installation(
            "blob-filesystem-reader-metadata-media-type",
            "object media type is invalid",
        ));
    }
    Ok(())
}

fn io_error(code: &'static str, error: io::Error) -> Error {
    Error::installation(code, error.to_string())
}

#[cfg(test)]
mod tests;
