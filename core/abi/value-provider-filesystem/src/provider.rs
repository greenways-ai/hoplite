pub struct FilesystemValueProvider {
    root: PathBuf,
    service: ValueService<FilesystemObjectReader>,
}

impl fmt::Debug for FilesystemValueProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FilesystemValueProvider")
            .field("limits", &self.service.limits())
            .finish_non_exhaustive()
    }
}

impl FilesystemValueProvider {
    /// Opens the immutable object layout through the shared read-only blob
    /// filesystem provider. This constructor does not create a second object
    /// store or cache.
    pub fn open(root: impl AsRef<Path>, limits: Limits) -> Result<Self, Error> {
        let limits = limits.validate()?;
        let reader = FilesystemObjectReader::open(
            root,
            ObjectReaderLimits {
                max_object_bytes: limits.max_frame_bytes,
                max_media_type_bytes: limits.max_media_type_bytes,
                io_chunk_bytes: limits.io_chunk_bytes,
            },
        )?;
        let root = reader.root().to_path_buf();
        Ok(Self {
            root,
            service: ValueService::new(reader, limits)?,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub const fn limits(&self) -> Limits {
        self.service.limits()
    }

    /// Executes one canonical host-provider call. Invalid host arguments are
    /// returned as provider errors. Every failure after a valid request has
    /// selected its digest is normalized into a closed `hoplite.value-result/0-alpha`
    /// value containing only a stable `hoplite.value/*` code.
    pub fn execute(&self, operation: &str, arguments_hta: &[u8]) -> Result<Vec<u8>, Error> {
        self.service.execute(operation, arguments_hta)
    }
}

impl ImmutableObjectReader for FilesystemObjectReader {
    fn read_verified(&self, digest: Digest, max_bytes: usize) -> Result<VerifiedObject, Failure> {
        let object = FilesystemObjectReader::read_verified(self, digest, max_bytes)
            .map_err(translate_object_failure)?;
        let object_digest = object.digest();
        Ok(VerifiedObject::new(object_digest, object.into_bytes()))
    }
}

const fn translate_object_failure(failure: ObjectReadFailure) -> Failure {
    match failure {
        ObjectReadFailure::Missing => Failure::Missing,
        ObjectReadFailure::Maximum => Failure::Maximum,
        ObjectReadFailure::Digest => Failure::Digest,
        ObjectReadFailure::Provider => Failure::Provider,
    }
}
