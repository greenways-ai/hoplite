#![forbid(unsafe_code)]

//! Bounded filesystem-backed provider core for Hara's generic `hoplite.value`
//! canonical-value verification service.
//!
//! The provider accepts one closed generic request, delegates actual immutable
//! object verification to the shared blob filesystem reader, then applies
//! `hara_hta::decode_canonical`. It has no filesystem layout, metadata, locking
//! or hashing implementation of its own.
//!
//! This crate contains no Tahto application, namespace, schema, manifest,
//! package-resolution, authorization, semantic-admission, or mutation logic.

use hoplite_blob_filesystem_reader::{
    Error as ObjectReaderError, Failure as ObjectReadFailure, FilesystemObjectReader,
    Limits as ObjectReaderLimits,
};
use hoplite_blob_store::Digest;
use hoplite_provider_hta::{Document, Error as HtaError, Kind, Node};
use std::fmt;
use std::path::{Path, PathBuf};

pub const SERVICE: &str = "hoplite.value";
pub const OPERATION: &str = "object/verify-hta";
pub const REQUEST_PROTOCOL: &str = "hoplite.value-request/1";
pub const RESULT_PROTOCOL: &str = "hoplite.value-result/1";
pub const PROFILE: &str = "hara.hta/1";

pub const OBJECT_MISSING: &str = "hoplite.value/object-missing";
pub const MAXIMUM_EXCEEDED: &str = "hoplite.value/maximum-exceeded";
pub const DIGEST_MISMATCH: &str = "hoplite.value/digest-mismatch";
pub const HTA_INVALID: &str = "hoplite.value/hta-invalid";
pub const HTA_NONCANONICAL: &str = "hoplite.value/hta-noncanonical";
pub const VALUE_UNSUPPORTED: &str = "hoplite.value/value-unsupported";
pub const PROVIDER_FAILURE: &str = "hoplite.value/provider-failure";

const MAGIC: &[u8; 4] = b"HTA1";
const FALSE: u8 = 1;
const TRUE: u8 = 2;
const I64: u8 = 3;
const STRING: u8 = 4;
const KEYWORD: u8 = 6;
const MAP: u8 = 11;
const REQUEST_FIELDS: &[&str] = &["digest", "max-bytes", "operation", "protocol"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Trusted installation ceiling. Requests may choose a smaller positive
    /// maximum but can never widen this value or Hara's fixed frame ceiling.
    pub max_frame_bytes: usize,
    pub max_media_type_bytes: usize,
    pub io_chunk_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_frame_bytes: 1024 * 1024,
            max_media_type_bytes: 256,
            io_chunk_bytes: 64 * 1024,
        }
    }
}

impl Limits {
    pub fn validate(self) -> Result<Self, Error> {
        if self.max_frame_bytes == 0 || self.max_frame_bytes > hara_hta::MAX_FRAME_BYTES {
            return Err(Error::InvalidLimits(
                "max_frame_bytes must be positive and no greater than Hara's HTA ceiling",
            ));
        }
        if self.max_media_type_bytes == 0 {
            return Err(Error::InvalidLimits(
                "max_media_type_bytes must be positive",
            ));
        }
        if self.io_chunk_bytes == 0 || self.io_chunk_bytes > self.max_frame_bytes {
            return Err(Error::InvalidLimits(
                "io_chunk_bytes must be positive and no greater than max_frame_bytes",
            ));
        }
        Ok(self)
    }
}

#[derive(Debug)]
pub enum Error {
    InvalidLimits(&'static str),
    Hta(HtaError),
    InvalidRequest(&'static str),
    OperationMismatch { call: String, request: String },
    Reader(ObjectReaderError),
}

impl Error {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidLimits(_) => "value-provider-limits-invalid",
            Self::Hta(_) => "value-request-hta",
            Self::InvalidRequest(_) => "value-request-invalid",
            Self::OperationMismatch { .. } => "value-operation-mismatch",
            Self::Reader(error) => error.code(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLimits(message) => {
                write!(formatter, "invalid hoplite.value limits: {message}")
            }
            Self::Hta(error) => write!(formatter, "invalid provider HTA: {error}"),
            Self::InvalidRequest(message) => {
                write!(formatter, "invalid hoplite.value request: {message}")
            }
            Self::OperationMismatch { call, request } => write!(
                formatter,
                "host operation {call:?} does not match request operation {request:?}"
            ),
            Self::Reader(error) => write!(formatter, "blob object reader error: {error}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<HtaError> for Error {
    fn from(error: HtaError) -> Self {
        Self::Hta(error)
    }
}

impl From<ObjectReaderError> for Error {
    fn from(error: ObjectReaderError) -> Self {
        Self::Reader(error)
    }
}

include!("reader.rs");
include!("provider.rs");
include!("hta.rs");

#[cfg(test)]
mod tests;
