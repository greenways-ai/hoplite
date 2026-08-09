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
                "value-provider-object-path-invalid",
                "provider-owned object path is not a regular file",
            ))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error("value-provider-object-stat", error)),
    }
}

fn read_metadata_file(path: &Path, maximum: usize) -> Result<Vec<u8>, Error> {
    require_regular_file(path, "value-provider-metadata-invalid")?;
    let declared = fs::metadata(path)
        .map_err(|error| io_error("value-provider-metadata-stat", error))?
        .len();
    if declared == 0 || declared > maximum as u64 {
        return Err(Error::installation(
            "value-provider-metadata-bounds",
            "object metadata exceeds its bounded profile",
        ));
    }
    let mut file = File::open(path)
        .map_err(|error| io_error("value-provider-metadata-open", error))?;
    let mut bytes = Vec::with_capacity(declared as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| io_error("value-provider-metadata-read", error))?;
    if bytes.len() as u64 != declared || bytes.len() > maximum {
        return Err(Error::installation(
            "value-provider-metadata-race",
            "object metadata length changed while reading",
        ));
    }
    Ok(bytes)
}

fn read_bounded_file(path: &Path, maximum: usize, chunk_bytes: usize) -> Result<Vec<u8>, Failure> {
    if !regular_file_exists(path).map_err(|_| Failure::Provider)? {
        return Err(Failure::Provider);
    }
    let declared = fs::metadata(path)
        .map_err(|_| Failure::Provider)?
        .len();
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
            "value-provider-metadata-media-type",
            "object media type is not UTF-8",
        )
    })?;
    let Some((type_name, subtype)) = value.split_once('/') else {
        return Err(Error::installation(
            "value-provider-metadata-media-type",
            "object media type is invalid",
        ));
    };
    let token = |character: char| {
        character.is_ascii_alphanumeric()
            || matches!(
                character,
                '!' | '#'
                    | '$'
                    | '&'
                    | '\''
                    | '*'
                    | '+'
                    | '-'
                    | '.'
                    | '^'
                    | '_'
                    | '`'
                    | '|'
                    | '~'
            )
    };
    if type_name.is_empty()
        || subtype.is_empty()
        || subtype.contains('/')
        || !type_name.chars().all(token)
        || !subtype.chars().all(token)
    {
        return Err(Error::installation(
            "value-provider-metadata-media-type",
            "object media type is invalid",
        ));
    }
    Ok(())
}

fn io_error(code: &'static str, error: io::Error) -> Error {
    Error::installation(code, error.to_string())
}
