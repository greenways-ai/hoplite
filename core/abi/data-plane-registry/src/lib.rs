#![deny(unsafe_op_in_unsafe_fn)]

//! Worker-local registry for opaque Hoplite data-plane resources.
//!
//! The registry owns native callback wrappers and assigns process-unique,
//! positive handles. Handles carry no application authority and are valid only
//! while the owning registry retains their resource.

use getrandom::getrandom;
use hoplite_data_plane_abi::{BodyError, BodyLimits, RequestBody, ResourceHandle, ResponseBody};
use hoplite_data_plane_ffi::{
    BridgeError, FfiRequestBody, FfiResponseBody, HopliteRequestBodyV1, HopliteResponseBodyV1,
};
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

const HANDLE_MASK: u64 = i64::MAX as u64;
static NEXT_HANDLE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static HANDLE_KEY: OnceLock<Result<u64, String>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    RequestBody,
    ResponseBody,
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequestBody => write!(formatter, "request body"),
            Self::ResponseBody => write!(formatter, "response body"),
        }
    }
}

#[derive(Debug)]
pub enum RegistryError {
    Bridge(BridgeError),
    Body(BodyError),
    UnknownHandle(u64),
    WrongKind {
        handle: u64,
        expected: ResourceKind,
        actual: ResourceKind,
    },
    Entropy(String),
    HandleExhausted,
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bridge(error) => error.fmt(formatter),
            Self::Body(error) => error.fmt(formatter),
            Self::UnknownHandle(handle) => {
                write!(formatter, "unknown or closed data-plane handle {handle}")
            }
            Self::WrongKind {
                handle,
                expected,
                actual,
            } => write!(
                formatter,
                "data-plane handle {handle} contains {actual}, expected {expected}"
            ),
            Self::Entropy(message) => {
                write!(formatter, "data-plane handle entropy failed: {message}")
            }
            Self::HandleExhausted => {
                write!(formatter, "data-plane handle space is exhausted")
            }
        }
    }
}

impl std::error::Error for RegistryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bridge(error) => Some(error),
            Self::Body(error) => Some(error),
            Self::UnknownHandle(_)
            | Self::WrongKind { .. }
            | Self::Entropy(_)
            | Self::HandleExhausted => None,
        }
    }
}

impl From<BridgeError> for RegistryError {
    fn from(error: BridgeError) -> Self {
        Self::Bridge(error)
    }
}

impl From<BodyError> for RegistryError {
    fn from(error: BodyError) -> Self {
        Self::Body(error)
    }
}

enum ResourceEntry {
    Request(FfiRequestBody),
    Response(FfiResponseBody),
}

impl ResourceEntry {
    fn kind(&self) -> ResourceKind {
        match self {
            Self::Request(_) => ResourceKind::RequestBody,
            Self::Response(_) => ResourceKind::ResponseBody,
        }
    }
}

enum HandleAllocator {
    Global,
    #[cfg(test)]
    Local {
        key: u64,
        next: Option<u64>,
    },
}

impl HandleAllocator {
    fn allocate(&mut self) -> Result<ResourceHandle, RegistryError> {
        loop {
            let (sequence, key) = match self {
                Self::Global => (next_global_sequence()?, process_handle_key()?),
                #[cfg(test)]
                Self::Local { key, next } => {
                    let sequence = next.ok_or(RegistryError::HandleExhausted)?;
                    *next = if sequence >= HANDLE_MASK {
                        None
                    } else {
                        Some(sequence + 1)
                    };
                    (sequence, *key & HANDLE_MASK)
                }
            };
            let value = permute_handle(sequence, key);
            if let Ok(handle) = ResourceHandle::new(value) {
                return Ok(handle);
            }
        }
    }
}

/// A worker-local registry for native request and response body sources.
pub struct ResourceRegistry {
    allocator: HandleAllocator,
    entries: HashMap<ResourceHandle, ResourceEntry>,
}

impl Default for ResourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceRegistry {
    pub fn new() -> Self {
        Self {
            allocator: HandleAllocator::Global,
            entries: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, handle: ResourceHandle) -> bool {
        self.entries.contains_key(&handle)
    }

    pub fn kind(&self, handle: ResourceHandle) -> Result<ResourceKind, RegistryError> {
        self.entries
            .get(&handle)
            .map(ResourceEntry::kind)
            .ok_or(RegistryError::UnknownHandle(handle.get()))
    }

    /// Takes ownership of a raw native request-body descriptor.
    ///
    /// Ownership transfers even when descriptor validation or handle allocation
    /// fails; a valid close callback is invoked exactly once.
    ///
    /// # Safety
    ///
    /// The descriptor must satisfy `FfiRequestBody::from_raw`'s context,
    /// callback, aliasing, and lifetime requirements and must not be reused.
    pub unsafe fn insert_request(
        &mut self,
        descriptor: HopliteRequestBodyV1,
        limits: BodyLimits,
    ) -> Result<ResourceHandle, RegistryError> {
        // SAFETY: forwarded from this function's documented caller contract.
        let body = unsafe { FfiRequestBody::from_raw(descriptor, limits) }?;
        let handle = self.allocator.allocate()?;
        self.entries.insert(handle, ResourceEntry::Request(body));
        Ok(handle)
    }

    /// Takes ownership of a raw native response-body descriptor.
    ///
    /// Ownership transfers even when descriptor validation or handle allocation
    /// fails; a valid close callback is invoked exactly once.
    ///
    /// # Safety
    ///
    /// The descriptor must satisfy `FfiResponseBody::from_raw`'s context,
    /// callback, aliasing, and lifetime requirements and must not be reused.
    pub unsafe fn insert_response(
        &mut self,
        descriptor: HopliteResponseBodyV1,
    ) -> Result<ResourceHandle, RegistryError> {
        // SAFETY: forwarded from this function's documented caller contract.
        let body = unsafe { FfiResponseBody::from_raw(descriptor) }?;
        let handle = self.allocator.allocate()?;
        self.entries.insert(handle, ResourceEntry::Response(body));
        Ok(handle)
    }

    pub fn request_declared_len(
        &self,
        handle: ResourceHandle,
    ) -> Result<Option<u64>, RegistryError> {
        match self.entry(handle)? {
            ResourceEntry::Request(body) => Ok(body.declared_len()),
            entry => Err(wrong_kind(handle, ResourceKind::RequestBody, entry.kind())),
        }
    }

    pub fn request_observed_len(&self, handle: ResourceHandle) -> Result<u64, RegistryError> {
        match self.entry(handle)? {
            ResourceEntry::Request(body) => Ok(body.observed_len()),
            entry => Err(wrong_kind(handle, ResourceKind::RequestBody, entry.kind())),
        }
    }

    pub fn read_request(
        &mut self,
        handle: ResourceHandle,
        output: &mut [u8],
    ) -> Result<usize, RegistryError> {
        match self.entry_mut(handle)? {
            ResourceEntry::Request(body) => body.read_chunk(output).map_err(RegistryError::from),
            entry => Err(wrong_kind(handle, ResourceKind::RequestBody, entry.kind())),
        }
    }

    pub fn finish_request(&self, handle: ResourceHandle) -> Result<(), RegistryError> {
        match self.entry(handle)? {
            ResourceEntry::Request(body) => body.finish().map_err(RegistryError::from),
            entry => Err(wrong_kind(handle, ResourceKind::RequestBody, entry.kind())),
        }
    }

    pub fn response_len(&self, handle: ResourceHandle) -> Result<u64, RegistryError> {
        match self.entry(handle)? {
            ResourceEntry::Response(body) => Ok(body.len()),
            entry => Err(wrong_kind(handle, ResourceKind::ResponseBody, entry.kind())),
        }
    }

    pub fn read_response(
        &mut self,
        handle: ResourceHandle,
        offset: u64,
        output: &mut [u8],
    ) -> Result<usize, RegistryError> {
        match self.entry_mut(handle)? {
            ResourceEntry::Response(body) => {
                body.read_at(offset, output).map_err(RegistryError::from)
            }
            entry => Err(wrong_kind(handle, ResourceKind::ResponseBody, entry.kind())),
        }
    }

    /// Removes a native resource and invokes its close callback exactly once.
    pub fn remove(&mut self, handle: ResourceHandle) -> Result<ResourceKind, RegistryError> {
        let entry = self
            .entries
            .remove(&handle)
            .ok_or(RegistryError::UnknownHandle(handle.get()))?;
        let kind = entry.kind();
        drop(entry);
        Ok(kind)
    }

    /// Closes all resources retained by this registry.
    pub fn close_all(&mut self) {
        self.entries.clear();
    }

    fn entry(&self, handle: ResourceHandle) -> Result<&ResourceEntry, RegistryError> {
        self.entries
            .get(&handle)
            .ok_or(RegistryError::UnknownHandle(handle.get()))
    }

    fn entry_mut(&mut self, handle: ResourceHandle) -> Result<&mut ResourceEntry, RegistryError> {
        self.entries
            .get_mut(&handle)
            .ok_or(RegistryError::UnknownHandle(handle.get()))
    }

    #[cfg(test)]
    fn with_allocator(key: u64, next: Option<u64>) -> Self {
        Self {
            allocator: HandleAllocator::Local { key, next },
            entries: HashMap::new(),
        }
    }
}

fn next_global_sequence() -> Result<u64, RegistryError> {
    NEXT_HANDLE_SEQUENCE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            if current == 0 {
                None
            } else if current >= HANDLE_MASK {
                Some(0)
            } else {
                Some(current + 1)
            }
        })
        .map_err(|_| RegistryError::HandleExhausted)
}

fn process_handle_key() -> Result<u64, RegistryError> {
    match HANDLE_KEY.get_or_init(|| {
        let mut bytes = [0_u8; 8];
        getrandom(&mut bytes).map_err(|error| error.to_string())?;
        Ok(u64::from_le_bytes(bytes) & HANDLE_MASK)
    }) {
        Ok(key) => Ok(*key),
        Err(message) => Err(RegistryError::Entropy(message.clone())),
    }
}

/// A bijection over the 63-bit handle domain. Odd multiplication and right-xor
/// steps are invertible modulo 2^63, so distinct process-wide sequences cannot
/// collide. The process key removes the monotonic shape from exposed handles.
fn permute_handle(mut value: u64, key: u64) -> u64 {
    value = (value ^ key) & HANDLE_MASK;
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9) & HANDLE_MASK;
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb) & HANDLE_MASK;
    value ^= value >> 31;
    value & HANDLE_MASK
}

fn wrong_kind(
    handle: ResourceHandle,
    expected: ResourceKind,
    actual: ResourceKind,
) -> RegistryError {
    RegistryError::WrongKind {
        handle: handle.get(),
        expected,
        actual,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoplite_data_plane_ffi::{
        HopliteCloseV1, HopliteReadAtV1, HopliteReadV1, HOPLITE_CALLBACK_OK,
    };
    use std::ffi::c_void;
    use std::slice;

    struct RequestContext {
        bytes: Vec<u8>,
        cursor: usize,
        close_count: usize,
    }

    unsafe extern "C" fn request_read(
        context: *mut c_void,
        output: *mut u8,
        capacity: usize,
        returned: *mut usize,
    ) -> i32 {
        // SAFETY: tests keep the context and output buffer live.
        let context = unsafe { &mut *(context as *mut RequestContext) };
        let remaining = context.bytes.len().saturating_sub(context.cursor);
        let count = capacity.min(remaining);
        if count != 0 {
            // SAFETY: the bridge supplies a writable buffer of `capacity`.
            let output = unsafe { slice::from_raw_parts_mut(output, capacity) };
            output[..count].copy_from_slice(&context.bytes[context.cursor..context.cursor + count]);
            context.cursor += count;
        }
        // SAFETY: the bridge supplies a valid return-count pointer.
        unsafe { *returned = count };
        HOPLITE_CALLBACK_OK
    }

    unsafe extern "C" fn request_close(context: *mut c_void) {
        // SAFETY: tests keep the context live through close.
        let context = unsafe { &mut *(context as *mut RequestContext) };
        context.close_count += 1;
    }

    struct ResponseContext {
        bytes: Vec<u8>,
        close_count: usize,
    }

    unsafe extern "C" fn response_read_at(
        context: *mut c_void,
        offset: u64,
        output: *mut u8,
        capacity: usize,
        returned: *mut usize,
    ) -> i32 {
        // SAFETY: tests keep the context and output buffer live.
        let context = unsafe { &mut *(context as *mut ResponseContext) };
        let offset = usize::try_from(offset).unwrap_or(usize::MAX);
        let remaining = context.bytes.len().saturating_sub(offset);
        let count = capacity.min(remaining);
        if count != 0 {
            // SAFETY: the bridge supplies a writable buffer of `capacity`.
            let output = unsafe { slice::from_raw_parts_mut(output, capacity) };
            output[..count].copy_from_slice(&context.bytes[offset..offset + count]);
        }
        // SAFETY: the bridge supplies a valid return-count pointer.
        unsafe { *returned = count };
        HOPLITE_CALLBACK_OK
    }

    unsafe extern "C" fn response_close(context: *mut c_void) {
        // SAFETY: tests keep the context live through close.
        let context = unsafe { &mut *(context as *mut ResponseContext) };
        context.close_count += 1;
    }

    fn request_descriptor(context: &mut RequestContext) -> HopliteRequestBodyV1 {
        HopliteRequestBodyV1 {
            context: context as *mut RequestContext as *mut c_void,
            declared_length: context.bytes.len() as u64,
            has_declared_length: 1,
            read: Some(request_read as HopliteReadV1),
            close: Some(request_close as HopliteCloseV1),
        }
    }

    fn response_descriptor(context: &mut ResponseContext) -> HopliteResponseBodyV1 {
        HopliteResponseBodyV1 {
            context: context as *mut ResponseContext as *mut c_void,
            length: context.bytes.len() as u64,
            read_at: Some(response_read_at as HopliteReadAtV1),
            close: Some(response_close as HopliteCloseV1),
        }
    }

    fn limits() -> BodyLimits {
        BodyLimits {
            max_body_bytes: 16,
            max_chunk_bytes: 2,
            require_declared_length: true,
        }
    }

    #[test]
    fn handles_are_positive_process_unique_and_kind_checked() {
        let mut request_context = RequestContext {
            bytes: b"abcd".to_vec(),
            cursor: 0,
            close_count: 0,
        };
        let mut response_context = ResponseContext {
            bytes: b"response".to_vec(),
            close_count: 0,
        };
        let mut registry = ResourceRegistry::new();
        let request =
            unsafe { registry.insert_request(request_descriptor(&mut request_context), limits()) }
                .unwrap();
        let response =
            unsafe { registry.insert_response(response_descriptor(&mut response_context)) }
                .unwrap();
        assert_ne!(request, response);
        assert!(request.get() <= HANDLE_MASK);
        assert!(response.get() <= HANDLE_MASK);
        assert_eq!(registry.kind(request).unwrap(), ResourceKind::RequestBody);
        assert_eq!(registry.kind(response).unwrap(), ResourceKind::ResponseBody);
        assert!(matches!(
            registry.response_len(request),
            Err(RegistryError::WrongKind {
                expected: ResourceKind::ResponseBody,
                actual: ResourceKind::RequestBody,
                ..
            })
        ));
    }

    #[test]
    fn a_foreign_registry_handle_never_aliases_a_local_resource() {
        let mut first_context = RequestContext {
            bytes: b"first".to_vec(),
            cursor: 0,
            close_count: 0,
        };
        let mut second_context = RequestContext {
            bytes: b"second".to_vec(),
            cursor: 0,
            close_count: 0,
        };
        let mut first = ResourceRegistry::new();
        let mut second = ResourceRegistry::new();
        let first_handle =
            unsafe { first.insert_request(request_descriptor(&mut first_context), limits()) }
                .unwrap();
        let second_handle =
            unsafe { second.insert_request(request_descriptor(&mut second_context), limits()) }
                .unwrap();
        assert_ne!(first_handle, second_handle);
        assert!(matches!(
            second.kind(first_handle),
            Err(RegistryError::UnknownHandle(_))
        ));
    }

    #[test]
    fn request_reads_remain_bounded_through_the_registry() {
        let mut context = RequestContext {
            bytes: b"abcd".to_vec(),
            cursor: 0,
            close_count: 0,
        };
        let mut registry = ResourceRegistry::new();
        let handle =
            unsafe { registry.insert_request(request_descriptor(&mut context), limits()) }.unwrap();
        let mut output = [0_u8; 8];
        assert_eq!(registry.request_declared_len(handle).unwrap(), Some(4));
        assert_eq!(registry.read_request(handle, &mut output).unwrap(), 2);
        assert_eq!(&output[..2], b"ab");
        assert_eq!(registry.read_request(handle, &mut output).unwrap(), 2);
        assert_eq!(&output[..2], b"cd");
        assert_eq!(registry.read_request(handle, &mut output).unwrap(), 0);
        assert_eq!(registry.request_observed_len(handle).unwrap(), 4);
        registry.finish_request(handle).unwrap();
    }

    #[test]
    fn response_reads_are_seekable_and_kind_isolated() {
        let mut context = ResponseContext {
            bytes: b"0123456789".to_vec(),
            close_count: 0,
        };
        let mut registry = ResourceRegistry::new();
        let handle =
            unsafe { registry.insert_response(response_descriptor(&mut context)) }.unwrap();
        let mut output = [0_u8; 3];
        assert_eq!(registry.response_len(handle).unwrap(), 10);
        assert_eq!(registry.read_response(handle, 4, &mut output).unwrap(), 3);
        assert_eq!(&output, b"456");
        assert!(matches!(
            registry.read_request(handle, &mut output),
            Err(RegistryError::WrongKind {
                expected: ResourceKind::RequestBody,
                actual: ResourceKind::ResponseBody,
                ..
            })
        ));
    }

    #[test]
    fn remove_closes_once_and_stale_handles_fail_closed() {
        let mut context = RequestContext {
            bytes: b"x".to_vec(),
            cursor: 0,
            close_count: 0,
        };
        let mut registry = ResourceRegistry::new();
        let handle =
            unsafe { registry.insert_request(request_descriptor(&mut context), limits()) }.unwrap();
        assert_eq!(registry.remove(handle).unwrap(), ResourceKind::RequestBody);
        assert_eq!(context.close_count, 1);
        assert!(matches!(
            registry.remove(handle),
            Err(RegistryError::UnknownHandle(_))
        ));
        assert_eq!(context.close_count, 1);
    }

    #[test]
    fn registry_drop_closes_every_remaining_descriptor() {
        let mut request_context = RequestContext {
            bytes: b"request".to_vec(),
            cursor: 0,
            close_count: 0,
        };
        let mut response_context = ResponseContext {
            bytes: b"response".to_vec(),
            close_count: 0,
        };
        {
            let mut registry = ResourceRegistry::new();
            unsafe {
                registry
                    .insert_request(request_descriptor(&mut request_context), limits())
                    .unwrap();
                registry
                    .insert_response(response_descriptor(&mut response_context))
                    .unwrap();
            }
            assert_eq!(registry.len(), 2);
        }
        assert_eq!(request_context.close_count, 1);
        assert_eq!(response_context.close_count, 1);
    }

    #[test]
    fn exhausted_handle_space_closes_an_unregistered_descriptor() {
        let mut first_context = RequestContext {
            bytes: b"a".to_vec(),
            cursor: 0,
            close_count: 0,
        };
        let mut second_context = RequestContext {
            bytes: b"b".to_vec(),
            cursor: 0,
            close_count: 0,
        };
        let mut registry = ResourceRegistry::with_allocator(0x1234, Some(HANDLE_MASK));
        unsafe {
            registry
                .insert_request(request_descriptor(&mut first_context), limits())
                .unwrap();
        }
        assert!(matches!(
            unsafe { registry.insert_request(request_descriptor(&mut second_context), limits()) },
            Err(RegistryError::HandleExhausted)
        ));
        assert_eq!(second_context.close_count, 1);
        assert_eq!(first_context.close_count, 0);
        registry.close_all();
        assert_eq!(first_context.close_count, 1);
    }
}
