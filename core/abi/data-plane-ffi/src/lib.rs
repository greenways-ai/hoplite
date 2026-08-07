#![deny(unsafe_op_in_unsafe_fn)]

//! C callback bridge for Hoplite's application-neutral data plane.
//!
//! The public descriptor structs mirror the C ABI, but safe Rust cannot turn
//! an arbitrary raw pointer into an active body source. Activating a descriptor
//! transfers ownership through an explicitly unsafe constructor.

// The implementation predates this ownership wrapper and contains callback
// conformance tests whose unsafe operations are already isolated inside unsafe
// extern functions. Keep the stricter lint on the public ownership boundary.
#[allow(unsafe_op_in_unsafe_fn)]
mod implementation;

use hoplite_data_plane_abi::{BodyError, BodyLimits, RequestBody, ResponseBody};
use std::ffi::c_void;

pub use implementation::{
    checked_resource_handle, BridgeError, DescriptorError, HopliteCloseV1,
    HopliteReadAtV1, HopliteReadV1, HopliteRequestBodyV1,
    HopliteResponseBodyV1, HOPLITE_CALLBACK_OK,
    HOPLITE_DATA_PLANE_ABI_VERSION,
};

struct CloseGuard {
    context: *mut c_void,
    close: Option<HopliteCloseV1>,
    armed: bool,
}

impl CloseGuard {
    fn new(context: *mut c_void, close: Option<HopliteCloseV1>) -> Self {
        Self {
            context,
            close,
            armed: !context.is_null(),
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CloseGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(close) = self.close {
            // SAFETY: `from_raw` requires the caller to transfer one valid,
            // exclusively owned context whose close callback is callable once.
            unsafe { close(self.context) };
        }
    }
}

/// Owned, bounded access to one native request body.
pub struct FfiRequestBody(implementation::FfiRequestBody);

impl FfiRequestBody {
    /// Takes ownership of a native request-body descriptor.
    ///
    /// Ownership transfers even when validation fails. A non-null descriptor
    /// with a close callback is closed exactly once on failure, explicit close,
    /// or drop.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the context and callbacks remain valid until
    /// close, that the descriptor is not used to construct another owner, that
    /// callbacks obey the supplied buffer bounds and do not unwind, and that
    /// close invalidates the context and may be invoked exactly once.
    pub unsafe fn from_raw(
        descriptor: HopliteRequestBodyV1,
        limits: BodyLimits,
    ) -> Result<Self, BridgeError> {
        let mut guard = CloseGuard::new(descriptor.context, descriptor.close);
        let body = implementation::FfiRequestBody::new(descriptor, limits)?;
        guard.disarm();
        Ok(Self(body))
    }

    pub fn close(&mut self) {
        self.0.close();
    }
}

impl RequestBody for FfiRequestBody {
    fn declared_len(&self) -> Option<u64> {
        self.0.declared_len()
    }

    fn observed_len(&self) -> u64 {
        self.0.observed_len()
    }

    fn read_chunk(&mut self, output: &mut [u8]) -> Result<usize, BodyError> {
        self.0.read_chunk(output)
    }

    fn finish(&self) -> Result<(), BodyError> {
        self.0.finish()
    }
}

/// Owned, bounded access to one immutable native response source.
pub struct FfiResponseBody(implementation::FfiResponseBody);

impl FfiResponseBody {
    /// Takes ownership of a native response-body descriptor.
    ///
    /// Ownership transfers even when validation fails. A non-null descriptor
    /// with a close callback is closed exactly once on failure, explicit close,
    /// or drop.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the context and callbacks remain valid until
    /// close, that the descriptor is not used to construct another owner, that
    /// callbacks obey the supplied buffer bounds and do not unwind, and that
    /// close invalidates the context and may be invoked exactly once.
    pub unsafe fn from_raw(
        descriptor: HopliteResponseBodyV1,
    ) -> Result<Self, BridgeError> {
        let mut guard = CloseGuard::new(descriptor.context, descriptor.close);
        let body = implementation::FfiResponseBody::new(descriptor)?;
        guard.disarm();
        Ok(Self(body))
    }

    pub fn close(&mut self) {
        self.0.close();
    }
}

impl ResponseBody for FfiResponseBody {
    fn len(&self) -> u64 {
        self.0.len()
    }

    fn read_at(
        &mut self,
        offset: u64,
        output: &mut [u8],
    ) -> Result<usize, BodyError> {
        self.0.read_at(offset, output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::slice;

    struct Context {
        bytes: Vec<u8>,
        cursor: usize,
        close_count: usize,
    }

    unsafe extern "C" fn read(
        context: *mut c_void,
        output: *mut u8,
        capacity: usize,
        returned: *mut usize,
    ) -> i32 {
        // SAFETY: test descriptors point at a live `Context` for the complete
        // owner lifetime and provide buffers of `capacity` bytes.
        let context = unsafe { &mut *(context as *mut Context) };
        let remaining = context.bytes.len().saturating_sub(context.cursor);
        let count = capacity.min(remaining);
        if count != 0 {
            // SAFETY: the bridge supplies a writable buffer of `capacity`.
            let output = unsafe { slice::from_raw_parts_mut(output, capacity) };
            output[..count].copy_from_slice(
                &context.bytes[context.cursor..context.cursor + count],
            );
            context.cursor += count;
        }
        // SAFETY: `returned` is supplied by the bridge and is non-null.
        unsafe { *returned = count };
        HOPLITE_CALLBACK_OK
    }

    unsafe extern "C" fn close(context: *mut c_void) {
        // SAFETY: test descriptors point at a live `Context`.
        let context = unsafe { &mut *(context as *mut Context) };
        context.close_count += 1;
    }

    #[test]
    fn failed_request_construction_closes_transferred_context() {
        let mut context = Context {
            bytes: b"body".to_vec(),
            cursor: 0,
            close_count: 0,
        };
        let descriptor = HopliteRequestBodyV1 {
            context: &mut context as *mut Context as *mut c_void,
            declared_length: 4,
            has_declared_length: 1,
            read: Some(read),
            close: Some(close),
        };
        let result = unsafe {
            FfiRequestBody::from_raw(
                descriptor,
                BodyLimits {
                    max_body_bytes: 0,
                    max_chunk_bytes: 1,
                    require_declared_length: true,
                },
            )
        };
        assert!(result.is_err());
        assert_eq!(context.close_count, 1);
    }

    #[test]
    fn failed_response_construction_closes_transferred_context() {
        let mut context = Context {
            bytes: b"body".to_vec(),
            cursor: 0,
            close_count: 0,
        };
        let descriptor = HopliteResponseBodyV1 {
            context: &mut context as *mut Context as *mut c_void,
            length: 4,
            read_at: None,
            close: Some(close),
        };
        let result = unsafe { FfiResponseBody::from_raw(descriptor) };
        assert!(result.is_err());
        assert_eq!(context.close_count, 1);
    }
}
