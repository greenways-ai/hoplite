#[derive(Clone, Debug, PartialEq, Eq)]
struct VerifiedObject {
    digest: Digest,
    bytes: Vec<u8>,
}

impl VerifiedObject {
    fn new(digest: Digest, bytes: Vec<u8>) -> Self {
        Self { digest, bytes }
    }

    const fn digest(&self) -> Digest {
        self.digest
    }

    fn byte_length(&self) -> usize {
        self.bytes.len()
    }

    fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

trait ImmutableObjectReader {
    fn read_verified(
        &self,
        digest: Digest,
        max_bytes: usize,
    ) -> Result<VerifiedObject, Failure>;
}

struct ValueService<R> {
    reader: R,
    limits: Limits,
}

impl<R> ValueService<R>
where
    R: ImmutableObjectReader,
{
    fn new(reader: R, limits: Limits) -> Result<Self, Error> {
        Ok(Self {
            reader,
            limits: limits.validate()?,
        })
    }

    const fn limits(&self) -> Limits {
        self.limits
    }

    fn execute(&self, operation: &str, arguments_hta: &[u8]) -> Result<Vec<u8>, Error> {
        let document = Document::parse(arguments_hta)?;
        let arguments = document.root();
        if arguments.kind() != Kind::Vector || arguments.len()? != 1 {
            return Err(Error::InvalidRequest(
                "host arguments must be a vector containing one request map",
            ));
        }
        let request = arguments.get(0)?;
        exact_fields(request, REQUEST_FIELDS)?;
        let request_operation = request_text(request, "operation")?;
        if operation != request_operation {
            return Err(Error::OperationMismatch {
                call: operation.to_owned(),
                request: request_operation.to_owned(),
            });
        }
        if operation != OPERATION {
            return Err(Error::InvalidRequest("operation is not supported"));
        }
        if request_text(request, "protocol")? != REQUEST_PROTOCOL {
            return Err(Error::InvalidRequest("request protocol is not supported"));
        }
        let digest_text = request_text(request, "digest")?;
        let digest = Digest::parse(digest_text)
            .map_err(|_| Error::InvalidRequest("digest must be canonical lowercase SHA-256"))?;
        let max_bytes = request_usize(request, "max-bytes", true)?;
        if max_bytes > hara_hta::MAX_FRAME_BYTES {
            return Err(Error::InvalidRequest(
                "max-bytes exceeds Hara's fixed HTA frame ceiling",
            ));
        }
        if max_bytes > self.limits.max_frame_bytes {
            return failure_result(digest_text, MAXIMUM_EXCEEDED);
        }

        let object = match self.reader.read_verified(digest, max_bytes) {
            Ok(object) => object,
            Err(failure) => return failure_result(digest_text, failure.code()),
        };
        if object.digest() != digest {
            return failure_result(digest_text, PROVIDER_FAILURE);
        }
        if object.byte_length() > max_bytes {
            return failure_result(digest_text, MAXIMUM_EXCEEDED);
        }

        match hara_hta::decode_canonical(object.bytes(), max_bytes) {
            Ok(_) => success_result(digest_text, object.bytes()),
            Err(message) => failure_result(digest_text, classify_hta_error(&message)),
        }
    }
}
