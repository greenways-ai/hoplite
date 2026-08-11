struct FilesystemObjectReader {
    objects_dir: PathBuf,
    lock_path: PathBuf,
    limits: Limits,
    process_lock: Mutex<()>,
}

pub struct FilesystemValueProvider {
    root: PathBuf,
    service: ValueService<FilesystemObjectReader>,
}

impl fmt::Debug for FilesystemValueProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FilesystemValueProvider")
            .field("root", &self.root)
            .field("limits", &self.service.limits())
            .finish_non_exhaustive()
    }
}

impl FilesystemValueProvider {
    /// Opens the immutable object layout already owned by the installed
    /// filesystem `hoplite.blob` provider. This constructor does not create a
    /// second object store or cache.
    pub fn open(root: impl AsRef<Path>, limits: Limits) -> Result<Self, Error> {
        let limits = limits.validate()?;
        let root = canonical_real_directory(root.as_ref(), "value-provider-root-invalid")?;
        let objects_root = real_directory(&root.join("objects"), "value-provider-objects-invalid")?;
        let objects_dir = real_directory(
            &objects_root.join("sha256"),
            "value-provider-sha256-root-invalid",
        )?;
        let lock_path = root.join(LOCK_FILE);
        require_regular_file(&lock_path, "value-provider-lock-invalid")?;
        let reader = FilesystemObjectReader {
            objects_dir,
            lock_path,
            limits,
            process_lock: Mutex::new(()),
        };
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
    /// selected its digest is normalized into a closed `hoplite.value-result/1`
    /// value containing only a stable `hoplite.value/*` code.
    pub fn execute(&self, operation: &str, arguments_hta: &[u8]) -> Result<Vec<u8>, Error> {
        self.service.execute(operation, arguments_hta)
    }
}

impl ImmutableObjectReader for FilesystemObjectReader {
    fn read_verified(&self, digest: Digest, max_bytes: usize) -> Result<VerifiedObject, Failure> {
        let _guard = self.exclusive().map_err(|_| Failure::Provider)?;
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
}

impl FilesystemObjectReader {
    fn exclusive(&self) -> Result<OperationGuard<'_>, Error> {
        let process = self.process_lock.lock().map_err(|_| Error::Poisoned)?;
        require_regular_file(&self.lock_path, "value-provider-lock-invalid")?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.lock_path)
            .map_err(|error| io_error("value-provider-lock-open", error))?;
        FileExt::lock_shared(&file)
            .map_err(|error| io_error("value-provider-lock-acquire", error))?;
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
                    "value-provider-metadata-limit",
                    "object metadata limit overflow",
                )
            })?;
        let bytes = read_metadata_file(path, maximum)?;
        ObjectMetadata::decode(&bytes, self.limits.max_media_type_bytes)
    }
}
