use std::fmt;
use std::io::{self, Read};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyLimits {
    pub max_body_bytes: u64,
    pub max_chunk_bytes: usize,
    pub require_declared_length: bool,
}

impl BodyLimits {
    pub fn validate(self) -> Result<Self, BodyError> {
        if self.max_body_bytes == 0 {
            return Err(BodyError::InvalidLimits("max_body_bytes must be positive"));
        }
        if self.max_chunk_bytes == 0 {
            return Err(BodyError::InvalidLimits("max_chunk_bytes must be positive"));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyAccount {
    limits: BodyLimits,
    declared_len: Option<u64>,
    observed: u64,
}

impl BodyAccount {
    pub fn new(limits: BodyLimits, declared_len: Option<u64>) -> Result<Self, BodyError> {
        let limits = limits.validate()?;
        if limits.require_declared_length && declared_len.is_none() {
            return Err(BodyError::LengthRequired);
        }
        if let Some(length) = declared_len {
            if length > limits.max_body_bytes {
                return Err(BodyError::LimitExceeded {
                    limit: limits.max_body_bytes,
                    attempted: length,
                });
            }
        }
        Ok(Self {
            limits,
            declared_len,
            observed: 0,
        })
    }

    pub fn account(&mut self, bytes: usize) -> Result<(), BodyError> {
        let attempted =
            self.observed
                .checked_add(bytes as u64)
                .ok_or(BodyError::LimitExceeded {
                    limit: self.limits.max_body_bytes,
                    attempted: u64::MAX,
                })?;
        if attempted > self.limits.max_body_bytes {
            return Err(BodyError::LimitExceeded {
                limit: self.limits.max_body_bytes,
                attempted,
            });
        }
        if let Some(declared) = self.declared_len {
            if attempted > declared {
                return Err(BodyError::DeclaredLengthExceeded {
                    declared,
                    attempted,
                });
            }
        }
        self.observed = attempted;
        Ok(())
    }

    pub fn finish(&self) -> Result<(), BodyError> {
        if let Some(declared) = self.declared_len {
            if self.observed != declared {
                return Err(BodyError::DeclaredLengthMismatch {
                    declared,
                    observed: self.observed,
                });
            }
        }
        Ok(())
    }

    pub fn remaining_limit(&self) -> u64 {
        self.limits.max_body_bytes.saturating_sub(self.observed)
    }

    pub fn observed(&self) -> u64 {
        self.observed
    }

    pub fn limits(&self) -> BodyLimits {
        self.limits
    }
}

#[derive(Debug)]
pub enum BodyError {
    InvalidLimits(&'static str),
    LengthRequired,
    LimitExceeded { limit: u64, attempted: u64 },
    DeclaredLengthExceeded { declared: u64, attempted: u64 },
    DeclaredLengthMismatch { declared: u64, observed: u64 },
    UnexpectedEof { expected: u64, observed: u64 },
    SourceReadPastRequest { requested: usize, returned: usize },
    Io(std::io::Error),
}

impl fmt::Display for BodyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLimits(message) => write!(formatter, "invalid body limits: {message}"),
            Self::LengthRequired => write!(formatter, "a declared request length is required"),
            Self::LimitExceeded { limit, attempted } => write!(
                formatter,
                "body limit exceeded: attempted {attempted} bytes with limit {limit}"
            ),
            Self::DeclaredLengthExceeded {
                declared,
                attempted,
            } => write!(
                formatter,
                "declared body length exceeded: declared {declared}, attempted {attempted}"
            ),
            Self::DeclaredLengthMismatch { declared, observed } => write!(
                formatter,
                "declared body length mismatch: declared {declared}, observed {observed}"
            ),
            Self::UnexpectedEof { expected, observed } => write!(
                formatter,
                "response source ended early: expected {expected}, observed {observed}"
            ),
            Self::SourceReadPastRequest {
                requested,
                returned,
            } => write!(
                formatter,
                "response source returned {returned} bytes after a request for {requested}"
            ),
            Self::Io(error) => write!(formatter, "body I/O error: {error}"),
        }
    }
}

impl std::error::Error for BodyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for BodyError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub trait RequestBody {
    fn declared_len(&self) -> Option<u64>;
    fn observed_len(&self) -> u64;
    fn read_chunk(&mut self, output: &mut [u8]) -> Result<usize, BodyError>;
    fn finish(&self) -> Result<(), BodyError>;
}

pub struct BoundedBody<R> {
    source: R,
    account: BodyAccount,
}

impl<R: Read> BoundedBody<R> {
    pub fn new(
        source: R,
        declared_len: Option<u64>,
        limits: BodyLimits,
    ) -> Result<Self, BodyError> {
        Ok(Self {
            source,
            account: BodyAccount::new(limits, declared_len)?,
        })
    }

    pub fn into_inner(self) -> R {
        self.source
    }
}

impl<R: Read> RequestBody for BoundedBody<R> {
    fn declared_len(&self) -> Option<u64> {
        self.account.declared_len
    }

    fn observed_len(&self) -> u64 {
        self.account.observed()
    }

    fn read_chunk(&mut self, output: &mut [u8]) -> Result<usize, BodyError> {
        if output.is_empty() {
            return Ok(0);
        }
        let remaining = self.account.remaining_limit();
        if remaining == 0 {
            let mut probe = [0_u8; 1];
            let read = self.source.read(&mut probe)?;
            if read == 0 {
                return Ok(0);
            }
            return Err(BodyError::LimitExceeded {
                limit: self.account.limits.max_body_bytes,
                attempted: self.account.observed().saturating_add(read as u64),
            });
        }
        let capacity = output
            .len()
            .min(self.account.limits.max_chunk_bytes)
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let read = self.source.read(&mut output[..capacity])?;
        self.account.account(read)?;
        Ok(read)
    }

    fn finish(&self) -> Result<(), BodyError> {
        self.account.finish()
    }
}

pub trait ResponseBody {
    fn len(&self) -> u64;
    fn read_at(&mut self, offset: u64, output: &mut [u8]) -> Result<usize, BodyError>;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct SliceBody<'a> {
    bytes: &'a [u8],
}

impl<'a> SliceBody<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

impl ResponseBody for SliceBody<'_> {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn read_at(&mut self, offset: u64, output: &mut [u8]) -> Result<usize, BodyError> {
        let offset = usize::try_from(offset).unwrap_or(usize::MAX);
        if output.is_empty() || offset >= self.bytes.len() {
            return Ok(0);
        }
        let count = output.len().min(self.bytes.len() - offset);
        output[..count].copy_from_slice(&self.bytes[offset..offset + count]);
        Ok(count)
    }
}
