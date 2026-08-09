#![forbid(unsafe_code)]

//! Bounded filesystem-backed provider core for Hara's generic `hara.value`
//! canonical-value verification service.
//!
//! The provider reuses the immutable digest-derived object layout owned by the
//! installed `hara.blob` filesystem driver. It accepts only one closed generic
//! request, reads from one trusted installation root, recomputes SHA-256 over
//! the actual bounded bytes, and delegates portable canonical decoding to
//! `hara_hta::decode_canonical`.
//!
//! This crate contains no Tahto application, namespace, schema, manifest,
//! package-resolution, authorization, semantic-admission, or mutation logic.

use fs2::FileExt;
use hoplite_blob_store::Digest;
use hoplite_provider_hta::{Document, Error as HtaError, Kind, Node};
use sha2::{Digest as Sha2Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

pub const SERVICE: &str = "hara.value";
pub const OPERATION: &str = "object/verify-hta";
pub const REQUEST_PROTOCOL: &str = "hara.value-request/1";
pub const RESULT_PROTOCOL: &str = "hara.value-result/1";
pub const PROFILE: &str = "hara.hta/1";

pub const OBJECT_MISSING: &str = "hara.value/object-missing";
pub const MAXIMUM_EXCEEDED: &str = "hara.value/maximum-exceeded";
pub const DIGEST_MISMATCH: &str = "hara.value/digest-mismatch";
pub const HTA_INVALID: &str = "hara.value/hta-invalid";
pub const HTA_NONCANONICAL: &str = "hara.value/hta-noncanonical";
pub const VALUE_UNSUPPORTED: &str = "hara.value/value-unsupported";
pub const PROVIDER_FAILURE: &str = "hara.value/provider-failure";

const MAGIC: &[u8; 4] = b"HTA1";
const OBJECT_MAGIC: &[u8; 4] = b"HBO1";
const LOCK_FILE: &str = "store.lock";
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
    Installation { code: &'static str, detail: String },
    Poisoned,
}

impl Error {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidLimits(_) => "value-provider-limits-invalid",
            Self::Hta(_) => "value-request-hta",
            Self::InvalidRequest(_) => "value-request-invalid",
            Self::OperationMismatch { .. } => "value-operation-mismatch",
            Self::Installation { code, .. } => code,
            Self::Poisoned => "value-provider-lock-poisoned",
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
            Self::InvalidLimits(message) => write!(formatter, "invalid hara.value limits: {message}"),
            Self::Hta(error) => write!(formatter, "invalid provider HTA: {error}"),
            Self::InvalidRequest(message) => {
                write!(formatter, "invalid hara.value request: {message}")
            }
            Self::OperationMismatch { call, request } => write!(
                formatter,
                "host operation {call:?} does not match request operation {request:?}"
            ),
            Self::Installation { code, detail } => write!(formatter, "{code}: {detail}"),
            Self::Poisoned => formatter.write_str("value-provider-lock-poisoned"),
        }
    }
}

impl std::error::Error for Error {}

impl From<HtaError> for Error {
    fn from(error: HtaError) -> Self {
        Self::Hta(error)
    }
}

include!("provider.rs");
include!("metadata.rs");
include!("hta.rs");
include!("filesystem.rs");

#[cfg(test)]
mod tests;
