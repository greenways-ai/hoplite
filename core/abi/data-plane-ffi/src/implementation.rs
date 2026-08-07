//! C callback bridge for Hoplite's application-neutral data plane.
//!
//! Nginx and other native hosts retain ownership of request and response byte
//! sources. Rust receives bounded callback descriptors and exposes the existing
//! `RequestBody` and `ResponseBody` contracts without converting large bodies
//! into Hara values or accepting caller-selected paths.

use hoplite_data_plane_abi::{
    BodyAccount, BodyError, BodyLimits, RequestBody, ResponseBody, ResourceHandle,
    ResourceHandleError,
};
use std::ffi::c_void;
use std::fmt;
use std::io;
use std::ptr;

pub const HOPLITE_DATA_PLANE_ABI_VERSION: u32 = 1;
pub const HOPLITE_CALLBACK_OK: i32 = 0;

pub type HopliteReadV1 = unsafe extern "C" fn(
    context: *mut c_void,
    output: *mut u8,
    capacity: usize,
    returned: *mut usize,
) -> i32;

pub type HopliteReadAtV1 = unsafe extern "C" fn(
    context: *mut c_void,
    offset: u64,
    output: *mut u8,
    capacity: usize,
    returned: *mut usize,
) -> i32;

pub type HopliteCloseV1 = unsafe extern "C" fn(context: *mut c_void);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HopliteRequestBodyV1 {
    pub context: *mut c_void,
    pub declared_length: u64,
    /// `0` means unknown and `1` means `declared_length` is authoritative.
    pub has_declared_length: u32,
    pub read: Option<HopliteReadV1>,
    pub close: Option<HopliteCloseV1>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HopliteResponseBodyV1 {
    pub context: *mut c_void,
    pub length: u64,
    pub read_at: Option<HopliteReadAtV1>,
    pub close: Option<HopliteCloseV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorError {
    NullContext,
    MissingReadCallback,
    InvalidDeclaredLengthFlag(u32),
}

impl fmt::Display for DescriptorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NullContext => write!(formatter, "native data-plane context is null"),
            Self::MissingReadCallback => {
                write!(formatter, "native data-plane read callback is missing")
            }
            Self::InvalidDeclaredLengthFlag(value) => write!(
                formatter,
                "native request declared-length flag must be 0 or 1, received {value}"
            ),
        }
    }
}

impl std::error::Error for DescriptorError {}

#[derive(Debug)]
pub enum BridgeError {
    Descriptor(DescriptorError),
    Body(BodyError),
    ResourceHandle(ResourceHandleError),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Descriptor(error) => error.fmt(formatter),
            Self::Body(error) => error.fmt(formatter),
            Self::ResourceHandle(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for BridgeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Descriptor(error) => Some(error),
            Self::Body(error) => Some(error),
            Self::ResourceHandle(error) => Some(error),
        }
    }
}

impl From<DescriptorError> for BridgeError {
    fn from(error: DescriptorError) -> Self {
        Self::Descriptor(error)
    }
}

impl From<BodyError> for BridgeError {
    fn from(error: BodyError) -> Self {
        Self::Body(error)
    }
}

impl From<ResourceHandleError> for BridgeError {
    fn from(error: ResourceHandleError) -> Self {
        Self::ResourceHandle(error)
    }
}

pub fn checked_resource_handle(value: u64) -> Result<ResourceHandle, BridgeError> {
    ResourceHandle::new(value).map_err(BridgeError::from)
}

pub struct FfiRequestBody {
    descriptor: HopliteRequestBodyV1,
    account: BodyAccount,
    closed: bool,
}

impl FfiRequestBody {
    pub fn new(
        descriptor: HopliteRequestBodyV1,
        limits: BodyLimits,
    ) -> Result<Self, BridgeError> {
        validate_context(descriptor.context)?;
        if descriptor.read.is_none() {
            return Err(DescriptorError::MissingReadCallback.into());
        }
        let declared_len = match descriptor.has_declared_length {
            0 => None,
            1 => Some(descriptor.declared_length),
            value => return Err(DescriptorError::InvalidDeclaredLengthFlag(value).into()),
        };
        Ok(Self {
            descriptor,
            account: BodyAccount::new(limits, declared_len)?,
            closed: false,
        })
    }

    fn invoke_read(&mut self, output: &mut [u8]) -> Result<usize, BodyError> {
        let callback = self
            .descriptor
            .read
            .expect("request descriptor validated at construction");
        let mut returned = 0_usize;
        let status = unsafe {
            callback(
                self.descriptor.context,
                output.as_mut_ptr(),
                output.len(),
                &mut returned,
            )
        };
        if status != HOPLITE_CALLBACK_OK {
            return Err(callback_failure("request-body read", status));
        }
        if returned > output.len() {
            return Err(BodyError::SourceReadPastRequest {
                requested: output.len(),
                returned,
            });
        }
        Ok(returned)
    }

    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        if let Some(close) = self.descriptor.close {
            unsafe { close(self.descriptor.context) };
        }
        self.descriptor.context = ptr::null_mut();
    }
}

impl RequestBody for FfiRequestBody {
    fn declared_len(&self) -> Option<u64> {
        match self.descriptor.has_declared_length {
            1 => Some(self.descriptor.declared_length),
            _ => None,
        }
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
            let read = self.invoke_read(&mut probe)?;
            if read == 0 {
                return Ok(0);
            }
            self.account.account(read)?;
            return Ok(read);
        }
        let capacity = output
            .len()
            .min(self.account.limits().max_chunk_bytes)
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let read = self.invoke_read(&mut output[..capacity])?;
        self.account.account(read)?;
        Ok(read)
    }

    fn finish(&self) -> Result<(), BodyError> {
        self.account.finish()
    }
}

impl Drop for FfiRequestBody {
    fn drop(&mut self) {
        self.close();
    }
}

pub struct FfiResponseBody {
    descriptor: HopliteResponseBodyV1,
    closed: bool,
}

impl FfiResponseBody {
    pub fn new(descriptor: HopliteResponseBodyV1) -> Result<Self, BridgeError> {
        validate_context(descriptor.context)?;
        if descriptor.read_at.is_none() {
            return Err(DescriptorError::MissingReadCallback.into());
        }
        Ok(Self {
            descriptor,
            closed: false,
        })
    }

    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        if let Some(close) = self.descriptor.close {
            unsafe { close(self.descriptor.context) };
        }
        self.descriptor.context = ptr::null_mut();
    }
}

impl ResponseBody for FfiResponseBody {
    fn len(&self) -> u64 {
        self.descriptor.length
    }

    fn read_at(&mut self, offset: u64, output: &mut [u8]) -> Result<usize, BodyError> {
        if output.is_empty() || offset >= self.descriptor.length {
            return Ok(0);
        }
        let remaining = self.descriptor.length - offset;
        let capacity = output
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let callback = self
            .descriptor
            .read_at
            .expect("response descriptor validated at construction");
        let mut returned = 0_usize;
        let status = unsafe {
            callback(
                self.descriptor.context,
                offset,
                output.as_mut_ptr(),
                capacity,
                &mut returned,
            )
        };
        if status != HOPLITE_CALLBACK_OK {
            return Err(callback_failure("response-body read", status));
        }
        if returned > capacity {
            return Err(BodyError::SourceReadPastRequest {
                requested: capacity,
                returned,
            });
        }
        Ok(returned)
    }
}

impl Drop for FfiResponseBody {
    fn drop(&mut self) {
        self.close();
    }
}

fn validate_context(context: *mut c_void) -> Result<(), DescriptorError> {
    if context.is_null() {
        Err(DescriptorError::NullContext)
    } else {
        Ok(())
    }
}

fn callback_failure(operation: &'static str, status: i32) -> BodyError {
    BodyError::Io(io::Error::new(
        io::ErrorKind::Other,
        format!("native {operation} callback failed with status {status}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoplite_data_plane_abi::{RequestBody, ResponseBody, StreamResponse};
    use std::slice;

    struct RequestContext {
        bytes: Vec<u8>,
        cursor: usize,
        close_count: usize,
        status: i32,
        overread: bool,
    }

    unsafe extern "C" fn request_read(
        context: *mut c_void,
        output: *mut u8,
        capacity: usize,
        returned: *mut usize,
    ) -> i32 {
        let context = &mut *(context as *mut RequestContext);
        if context.status != HOPLITE_CALLBACK_OK {
            return context.status;
        }
        let remaining = context.bytes.len().saturating_sub(context.cursor);
        let count = capacity.min(remaining);
        if count != 0 {
            slice::from_raw_parts_mut(output, capacity)[..count]
                .copy_from_slice(&context.bytes[context.cursor..context.cursor + count]);
            context.cursor += count;
        }
        *returned = if context.overread {
            capacity.saturating_add(1)
        } else {
            count
        };
        HOPLITE_CALLBACK_OK
    }

    unsafe extern "C" fn request_close(context: *mut c_void) {
        let context = &mut *(context as *mut RequestContext);
        context.close_count += 1;
    }

    struct ResponseContext {
        bytes: Vec<u8>,
        close_count: usize,
        status: i32,
        overread: bool,
        early_eof_at: Option<u64>,
    }

    unsafe extern "C" fn response_read_at(
        context: *mut c_void,
        offset: u64,
        output: *mut u8,
        capacity: usize,
        returned: *mut usize,
    ) -> i32 {
        let context = &mut *(context as *mut ResponseContext);
        if context.status != HOPLITE_CALLBACK_OK {
            return context.status;
        }
        if context
            .early_eof_at
            .is_some_and(|limit| offset >= limit)
        {
            *returned = 0;
            return HOPLITE_CALLBACK_OK;
        }
        let offset = usize::try_from(offset).unwrap_or(usize::MAX);
        let remaining = context.bytes.len().saturating_sub(offset);
        let count = capacity.min(remaining);
        if count != 0 {
            slice::from_raw_parts_mut(output, capacity)[..count]
                .copy_from_slice(&context.bytes[offset..offset + count]);
        }
        *returned = if context.overread {
            capacity.saturating_add(1)
        } else {
            count
        };
        HOPLITE_CALLBACK_OK
    }

    unsafe extern "C" fn response_close(context: *mut c_void) {
        let context = &mut *(context as *mut ResponseContext);
        context.close_count += 1;
    }

    fn request_descriptor(context: &mut RequestContext, declared_length: u64) -> HopliteRequestBodyV1 {
        HopliteRequestBodyV1 {
            context: context as *mut RequestContext as *mut c_void,
            declared_length,
            has_declared_length: 1,
            read: Some(request_read),
            close: Some(request_close),
        }
    }

    fn response_descriptor(context: &mut ResponseContext) -> HopliteResponseBodyV1 {
        HopliteResponseBodyV1 {
            context: context as *mut ResponseContext as *mut c_void,
            length: context.bytes.len() as u64,
            read_at: Some(response_read_at),
            close: Some(response_close),
        }
    }

    #[test]
    fn request_callback_is_chunk_bounded_and_closed_once() {
        let mut context = RequestContext {
            bytes: b"abcdef".to_vec(),
            cursor: 0,
            close_count: 0,
            status: HOPLITE_CALLBACK_OK,
            overread: false,
        };
        {
            let mut body = FfiRequestBody::new(
                request_descriptor(&mut context, 6),
                BodyLimits {
                    max_body_bytes: 6,
                    max_chunk_bytes: 2,
                    require_declared_length: true,
                },
            )
            .unwrap();
            let mut output = [0_u8; 8];
            assert_eq!(body.read_chunk(&mut output).unwrap(), 2);
            assert_eq!(&output[..2], b"ab");
            assert_eq!(body.read_chunk(&mut output).unwrap(), 2);
            assert_eq!(&output[..2], b"cd");
            assert_eq!(body.read_chunk(&mut output).unwrap(), 2);
            assert_eq!(&output[..2], b"ef");
            assert_eq!(body.read_chunk(&mut output).unwrap(), 0);
            assert_eq!(body.observed_len(), 6);
            body.finish().unwrap();
            body.close();
            body.close();
        }
        assert_eq!(context.close_count, 1);
    }

    #[test]
    fn request_callback_cannot_report_more_than_requested() {
        let mut context = RequestContext {
            bytes: b"abc".to_vec(),
            cursor: 0,
            close_count: 0,
            status: HOPLITE_CALLBACK_OK,
            overread: true,
        };
        let mut body = FfiRequestBody::new(
            request_descriptor(&mut context, 3),
            BodyLimits {
                max_body_bytes: 3,
                max_chunk_bytes: 2,
                require_declared_length: true,
            },
        )
        .unwrap();
        let mut output = [0_u8; 8];
        assert!(matches!(
            body.read_chunk(&mut output),
            Err(BodyError::SourceReadPastRequest {
                requested: 2,
                returned: 3
            })
        ));
    }

    #[test]
    fn callback_failure_becomes_a_body_io_error() {
        let mut context = RequestContext {
            bytes: Vec::new(),
            cursor: 0,
            close_count: 0,
            status: 17,
            overread: false,
        };
        let mut body = FfiRequestBody::new(
            request_descriptor(&mut context, 0),
            BodyLimits {
                max_body_bytes: 1,
                max_chunk_bytes: 1,
                require_declared_length: true,
            },
        )
        .unwrap();
        let mut output = [0_u8; 1];
        assert!(matches!(body.read_chunk(&mut output), Err(BodyError::Io(_))));
    }

    #[test]
    fn response_callback_streams_one_exact_range_and_closes() {
        let mut context = ResponseContext {
            bytes: b"0123456789".to_vec(),
            close_count: 0,
            status: HOPLITE_CALLBACK_OK,
            overread: false,
            early_eof_at: None,
        };
        {
            let body = FfiResponseBody::new(response_descriptor(&mut context)).unwrap();
            let mut response = StreamResponse::new(body, Some("bytes=2-5")).unwrap();
            assert_eq!(response.plan().status, 206);
            assert_eq!(response.plan().content_length, 4);
            let mut output = [0_u8; 3];
            assert_eq!(response.read_next(&mut output).unwrap(), 3);
            assert_eq!(&output, b"234");
            assert_eq!(response.read_next(&mut output).unwrap(), 1);
            assert_eq!(&output[..1], b"5");
            assert_eq!(response.read_next(&mut output).unwrap(), 0);
            response.finish().unwrap();
            let mut body = response.into_inner();
            body.close();
            body.close();
        }
        assert_eq!(context.close_count, 1);
    }

    #[test]
    fn response_callback_cannot_overread_or_end_early() {
        let mut overread = ResponseContext {
            bytes: b"abcd".to_vec(),
            close_count: 0,
            status: HOPLITE_CALLBACK_OK,
            overread: true,
            early_eof_at: None,
        };
        let mut body = FfiResponseBody::new(response_descriptor(&mut overread)).unwrap();
        let mut output = [0_u8; 2];
        assert!(matches!(
            body.read_at(0, &mut output),
            Err(BodyError::SourceReadPastRequest {
                requested: 2,
                returned: 3
            })
        ));

        let mut early = ResponseContext {
            bytes: b"abcd".to_vec(),
            close_count: 0,
            status: HOPLITE_CALLBACK_OK,
            overread: false,
            early_eof_at: Some(2),
        };
        let body = FfiResponseBody::new(response_descriptor(&mut early)).unwrap();
        let mut response = StreamResponse::new(body, None).unwrap();
        assert_eq!(response.read_next(&mut output).unwrap(), 2);
        assert!(matches!(
            response.read_next(&mut output),
            Err(BodyError::UnexpectedEof {
                expected: 4,
                observed: 2
            })
        ));
    }

    #[test]
    fn descriptors_and_resource_handles_fail_closed() {
        let request = HopliteRequestBodyV1 {
            context: ptr::null_mut(),
            declared_length: 0,
            has_declared_length: 2,
            read: None,
            close: None,
        };
        assert!(matches!(
            FfiRequestBody::new(
                request,
                BodyLimits {
                    max_body_bytes: 1,
                    max_chunk_bytes: 1,
                    require_declared_length: false,
                }
            ),
            Err(BridgeError::Descriptor(DescriptorError::NullContext))
        ));
        assert!(checked_resource_handle(0).is_err());
        assert_eq!(checked_resource_handle(42).unwrap().get(), 42);
    }
}
