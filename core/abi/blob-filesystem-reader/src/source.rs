#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedObjectSummary {
    digest: Digest,
    size: u64,
}

impl VerifiedObjectSummary {
    pub const fn digest(&self) -> Digest {
        self.digest
    }

    pub const fn size(&self) -> u64 {
        self.size
    }
}

struct ResolvedObject {
    metadata: ObjectMetadata,
    data_path: PathBuf,
}

pub struct FilesystemResponseSource {
    file: File,
    declared: u64,
    remaining: u64,
    closed: bool,
}

impl ResponseSource for FilesystemResponseSource {
    fn declared_length(&self) -> u64 {
        self.declared
    }

    fn read(&mut self, output: &mut [u8]) -> Result<usize, BlobError> {
        if self.closed {
            return Err(BlobError::SourceClosed);
        }
        if output.is_empty() || self.remaining == 0 {
            return Ok(0);
        }
        let capacity = usize::try_from(self.remaining)
            .unwrap_or(usize::MAX)
            .min(output.len());
        let read = self
            .file
            .read(&mut output[..capacity])
            .map_err(|error| BlobError::driver("blob-filesystem-source-read", error.to_string()))?;
        if read == 0 {
            return Err(BlobError::InvalidRange {
                offset: self.declared - self.remaining,
                length: self.remaining,
                size: Some(self.declared),
            });
        }
        self.remaining -= read as u64;
        Ok(read)
    }

    fn close(&mut self) -> Result<(), BlobError> {
        if self.closed {
            return Err(BlobError::SourceClosed);
        }
        self.closed = true;
        Ok(())
    }
}

impl FilesystemObjectReader {
    pub fn inspect_verified(&self, digest: Digest) -> Result<VerifiedObjectSummary, BlobError> {
        let _guard = self.shared().map_err(reader_blob_error)?;
        self.inspect_verified_locked(digest)
    }

    pub fn open_source(&self, request: ObjectRange) -> Result<FilesystemResponseSource, BlobError> {
        let request = ObjectRange::new(request.digest, request.offset, request.length)?;
        let _guard = self.shared().map_err(reader_blob_error)?;
        let summary = self.inspect_verified_locked(request.digest)?;
        let end = request
            .offset
            .checked_add(request.length)
            .ok_or(BlobError::InvalidRange {
                offset: request.offset,
                length: request.length,
                size: Some(summary.size),
            })?;
        if end > summary.size {
            return Err(BlobError::InvalidRange {
                offset: request.offset,
                length: request.length,
                size: Some(summary.size),
            });
        }

        let (_, data_path) = self.object_paths(request.digest);
        require_regular_file(&data_path, "blob-filesystem-source-invalid")
            .map_err(reader_blob_error)?;
        let mut file = File::open(&data_path)
            .map_err(|error| BlobError::driver("blob-filesystem-source-open", error.to_string()))?;
        file.seek(SeekFrom::Start(request.offset))
            .map_err(|error| BlobError::driver("blob-filesystem-source-seek", error.to_string()))?;
        Ok(FilesystemResponseSource {
            file,
            declared: request.length,
            remaining: request.length,
            closed: false,
        })
    }

    fn resolve_object_locked(&self, digest: Digest) -> Result<ResolvedObject, BlobError> {
        let (metadata_path, data_path) = self.object_paths(digest);
        let object_directory = metadata_path.parent().ok_or_else(|| {
            BlobError::driver(
                "blob-filesystem-object-path",
                "object metadata path has no parent",
            )
        })?;
        match fs::symlink_metadata(object_directory) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(BlobError::driver(
                    "blob-filesystem-object-directory-invalid",
                    "object path is not a real directory",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(BlobError::ObjectMissing { digest });
            }
            Err(error) => {
                return Err(BlobError::driver(
                    "blob-filesystem-object-directory-stat",
                    error.to_string(),
                ));
            }
        }

        let metadata_exists = regular_file_exists(&metadata_path).map_err(reader_blob_error)?;
        let data_exists = regular_file_exists(&data_path).map_err(reader_blob_error)?;
        match (metadata_exists, data_exists) {
            (false, false) => return Err(BlobError::ObjectMissing { digest }),
            (false, true) => {
                return Err(BlobError::driver(
                    "blob-filesystem-object-metadata-missing",
                    "object bytes exist without metadata",
                ));
            }
            (true, false) => {
                return Err(BlobError::driver(
                    "blob-filesystem-object-data-missing",
                    "object metadata exists without bytes",
                ));
            }
            (true, true) => {}
        }

        let metadata = self
            .read_object_metadata(&metadata_path)
            .map_err(reader_blob_error)?;
        if metadata.digest != digest {
            return Err(BlobError::DigestMismatch {
                expected: digest,
                actual: metadata.digest,
            });
        }
        if metadata.size == 0 {
            return Err(BlobError::driver(
                "blob-filesystem-object-size-invalid",
                "object metadata declares an empty object",
            ));
        }
        if metadata.size > self.limits.max_object_bytes as u64 {
            return Err(BlobError::ObjectLimitExceeded {
                limit: self.limits.max_object_bytes as u64,
                actual: metadata.size,
            });
        }
        Ok(ResolvedObject {
            metadata,
            data_path,
        })
    }

    fn inspect_verified_locked(&self, digest: Digest) -> Result<VerifiedObjectSummary, BlobError> {
        let resolved = self.resolve_object_locked(digest)?;
        let (actual_digest, actual_size) = hash_object_file(
            &resolved.data_path,
            self.limits.max_object_bytes,
            self.limits.io_chunk_bytes,
        )?;
        if actual_digest != digest {
            return Err(BlobError::DigestMismatch {
                expected: digest,
                actual: actual_digest,
            });
        }
        if actual_size != resolved.metadata.size {
            return Err(BlobError::driver(
                "blob-filesystem-object-size-mismatch",
                "object metadata size does not match stored bytes",
            ));
        }
        Ok(VerifiedObjectSummary {
            digest,
            size: actual_size,
        })
    }
}

fn reader_blob_error(error: Error) -> BlobError {
    BlobError::driver(error.code(), error.to_string())
}

fn value_failure(error: BlobError) -> Failure {
    match error {
        BlobError::ObjectMissing { .. } => Failure::Missing,
        BlobError::ObjectLimitExceeded { .. } => Failure::Maximum,
        BlobError::DigestMismatch { .. } => Failure::Digest,
        _ => Failure::Provider,
    }
}

fn hash_object_file(
    path: &Path,
    maximum: usize,
    chunk_bytes: usize,
) -> Result<(Digest, u64), BlobError> {
    require_regular_file(path, "blob-filesystem-object-data-invalid").map_err(reader_blob_error)?;
    let declared = fs::metadata(path)
        .map_err(|error| BlobError::driver("blob-filesystem-object-stat", error.to_string()))?
        .len();
    if declared > maximum as u64 {
        return Err(BlobError::ObjectLimitExceeded {
            limit: maximum as u64,
            actual: declared,
        });
    }

    let mut file = File::open(path)
        .map_err(|error| BlobError::driver("blob-filesystem-object-open", error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; chunk_bytes.max(1)];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| BlobError::driver("blob-filesystem-object-read", error.to_string()))?;
        if read == 0 {
            break;
        }
        size = size
            .checked_add(read as u64)
            .ok_or_else(|| BlobError::driver("blob-filesystem-object-size", "size overflow"))?;
        if size > maximum as u64 {
            return Err(BlobError::ObjectLimitExceeded {
                limit: maximum as u64,
                actual: size,
            });
        }
        hasher.update(&buffer[..read]);
    }
    if size != declared {
        return Err(BlobError::driver(
            "blob-filesystem-object-race",
            "object length changed while hashing",
        ));
    }
    let actual = hasher.finalize();
    let mut digest = [0_u8; 32];
    digest.copy_from_slice(&actual);
    Ok((Digest::from_bytes(digest), size))
}
