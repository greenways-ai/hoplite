#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    Missing,
    Maximum,
    Digest,
    Provider,
}

impl Failure {
    const fn code(self) -> &'static str {
        match self {
            Self::Missing => OBJECT_MISSING,
            Self::Maximum => MAXIMUM_EXCEEDED,
            Self::Digest => DIGEST_MISMATCH,
            Self::Provider => PROVIDER_FAILURE,
        }
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
                "value-provider-metadata-overflow",
                "metadata length overflow",
            )
        })?;
        let value = self.bytes.get(self.cursor..end).ok_or_else(|| {
            Error::installation("value-provider-metadata-truncated", "metadata is truncated")
        })?;
        self.cursor = end;
        Ok(value)
    }

    fn expect_magic(&mut self, expected: &[u8; 4]) -> Result<(), Error> {
        if self.take(4)? == expected {
            Ok(())
        } else {
            Err(Error::installation(
                "value-provider-metadata-magic",
                "object metadata has an unsupported format",
            ))
        }
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let bytes: [u8; 4] = self.take(4)?.try_into().map_err(|_| {
            Error::installation("value-provider-metadata-u32", "invalid metadata u32")
        })?;
        Ok(u32::from_be_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        let bytes: [u8; 8] = self.take(8)?.try_into().map_err(|_| {
            Error::installation("value-provider-metadata-u64", "invalid metadata u64")
        })?;
        Ok(u64::from_be_bytes(bytes))
    }

    fn array_32(&mut self) -> Result<[u8; 32], Error> {
        self.take(32)?.try_into().map_err(|_| {
            Error::installation("value-provider-metadata-digest", "invalid metadata digest")
        })
    }

    fn sized_bytes(&mut self, limit: usize) -> Result<&'a [u8], Error> {
        let length = usize::try_from(self.u32()?).map_err(|_| {
            Error::installation(
                "value-provider-metadata-text-length",
                "metadata text length does not fit usize",
            )
        })?;
        if length == 0 || length > limit {
            return Err(Error::installation(
                "value-provider-metadata-text-bounds",
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
                "value-provider-metadata-trailing",
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
