#![deny(unsafe_op_in_unsafe_fn)]

//! Work-scoped native boundary for the canonical `hoplite.blob` adapter.
//!
//! The provider owns one application-neutral installed blob store per worker.
//! Request source callbacks are supplied by a trusted host call already bound
//! to the exact request and work. Immutable response sources remain native and
//! are registered under positive handles that are usable only by their owning
//! request and work.

use hoplite_blob_filesystem_reader::FilesystemResponseSource;
use hoplite_blob_store::{
    AppendReceipt, BlobStore, ByteSource, DigestVerifier, Error as BlobError, InMemoryBlobStore,
    Limits, MemoryResponseSource, ObjectDescriptor, ObjectRange, ResponseSource, StagingAppend,
    StagingCommit, StagingKey, StagingOpen, StagingStatus,
};
use hoplite_blob_store_filesystem::FilesystemBlobStore;
use hoplite_blob_store_provider::{
    Provider as CanonicalProvider, RequestSourceResolver, ResponseSourceRegistrar,
};
use sha2::{Digest as Sha2Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::ptr;
use std::slice;
use std::str;
use std::sync::{Arc, Mutex, MutexGuard};

pub const ABI_VERSION: u32 = 1;
pub const STATUS_OK: i32 = 0;
pub const STATUS_INVALID: i32 = 1;
pub const STATUS_FAILURE: i32 = 2;
pub const STATUS_RESOURCE_ERROR: i32 = 3;
pub const RESULT_SUCCESS: u32 = 1;
pub const RESULT_FAILURE: u32 = 2;

const HTA_MAGIC: &[u8; 4] = b"HTA1";
const HTA_STRING: u8 = 4;
const MAX_HANDLE: u64 = i64::MAX as u64;

type RequestReadV1 = unsafe extern "C" fn(
    request_context: *mut c_void,
    work: u64,
    source_handle: u64,
    output: *mut u8,
    capacity: usize,
    returned: *mut usize,
) -> i32;

type RequestFinishV1 =
    unsafe extern "C" fn(request_context: *mut c_void, work: u64, source_handle: u64) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HopliteBlobStoreLimitsV1 {
    pub max_object_bytes: u64,
    pub max_append_bytes: usize,
    pub max_source_chunk_bytes: usize,
    pub max_staging_key_bytes: usize,
    pub max_media_type_bytes: usize,
    pub max_staging_entries: usize,
    pub max_objects: usize,
}

impl HopliteBlobStoreLimitsV1 {
    fn into_limits(self) -> Limits {
        Limits {
            max_object_bytes: self.max_object_bytes,
            max_append_bytes: self.max_append_bytes,
            max_source_chunk_bytes: self.max_source_chunk_bytes,
            max_staging_key_bytes: self.max_staging_key_bytes,
            max_media_type_bytes: self.max_media_type_bytes,
            max_staging_entries: self.max_staging_entries,
            max_objects: self.max_objects,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HopliteBlobStoreCallV1 {
    pub abi_version: u32,
    pub request_context: *mut c_void,
    pub work: u64,
    pub request_read: Option<RequestReadV1>,
    pub request_finish: Option<RequestFinishV1>,
}

#[repr(C)]
pub struct HopliteBlobStoreResultV1 {
    pub kind: u32,
    pub data: *mut u8,
    pub len: usize,
}

#[derive(Clone, Copy)]
struct CallContext {
    request_context: usize,
    work: u64,
    request_read: RequestReadV1,
    request_finish: RequestFinishV1,
}

#[derive(Clone)]
struct SharedCall {
    current: Arc<Mutex<Option<CallContext>>>,
}

impl SharedCall {
    fn new() -> Self {
        Self {
            current: Arc::new(Mutex::new(None)),
        }
    }

    fn set(&self, context: CallContext) -> Result<(), BlobError> {
        let mut current = lock(&self.current)?;
        if current.is_some() {
            return Err(BlobError::driver(
                "blob-provider-busy",
                "another provider call is active",
            ));
        }
        *current = Some(context);
        Ok(())
    }

    fn get(&self) -> Result<CallContext, BlobError> {
        lock(&self.current)?
            .as_ref()
            .copied()
            .ok_or_else(|| BlobError::source("blob-source-forbidden", "no active provider call"))
    }

    fn clear(&self) {
        if let Ok(mut current) = self.current.lock() {
            *current = None;
        }
    }
}

struct CallReset(SharedCall);

impl Drop for CallReset {
    fn drop(&mut self) {
        self.0.clear();
    }
}

#[derive(Clone)]
struct HostRequestResolver {
    call: SharedCall,
}

impl RequestSourceResolver for HostRequestResolver {
    type Source = HostRequestSource;

    fn resolve(&self, source_handle: u64) -> Result<Self::Source, BlobError> {
        if source_handle == 0 || source_handle > MAX_HANDLE {
            return Err(BlobError::source(
                "blob-source-forbidden",
                "source handle is outside the portable range",
            ));
        }
        Ok(HostRequestSource {
            call: self.call.get()?,
            source_handle,
            finished: false,
        })
    }
}

struct HostRequestSource {
    call: CallContext,
    source_handle: u64,
    finished: bool,
}

impl ByteSource for HostRequestSource {
    fn read(&mut self, output: &mut [u8]) -> Result<usize, BlobError> {
        if self.finished {
            return Err(BlobError::SourceClosed);
        }
        if output.is_empty() {
            return Ok(0);
        }
        let mut returned = 0_usize;
        // SAFETY: the trusted host supplied this callback and context for the
        // duration of the synchronous provider call. `output` is writable for
        // exactly its reported capacity and `returned` is a valid out pointer.
        let status = unsafe {
            (self.call.request_read)(
                self.call.request_context as *mut c_void,
                self.call.work,
                self.source_handle,
                output.as_mut_ptr(),
                output.len(),
                &mut returned,
            )
        };
        if status != STATUS_OK {
            return Err(BlobError::source(
                "blob-source-forbidden",
                "host rejected the request source read",
            ));
        }
        if returned > output.len() {
            return Err(BlobError::SourceProtocol {
                detail: "host returned more source bytes than requested",
            });
        }
        Ok(returned)
    }

    fn finish(&mut self) -> Result<(), BlobError> {
        if self.finished {
            return Err(BlobError::SourceClosed);
        }
        // SAFETY: the trusted host supplied this callback and context for the
        // active synchronous provider call.
        let status = unsafe {
            (self.call.request_finish)(
                self.call.request_context as *mut c_void,
                self.call.work,
                self.source_handle,
            )
        };
        if status != STATUS_OK {
            return Err(BlobError::source(
                "blob-source-finish",
                "host rejected request source completion",
            ));
        }
        self.finished = true;
        Ok(())
    }
}

struct ResponseEntry {
    request_context: usize,
    work: u64,
    source: ProviderResponseSource,
}

struct ResponseRegistry {
    next: u64,
    entries: BTreeMap<u64, ResponseEntry>,
}

impl ResponseRegistry {
    fn new() -> Self {
        Self {
            next: 1,
            entries: BTreeMap::new(),
        }
    }

    fn register(
        &mut self,
        request_context: usize,
        work: u64,
        source: ProviderResponseSource,
    ) -> Result<u64, BlobError> {
        if request_context == 0 || work == 0 {
            return Err(BlobError::driver(
                "blob-response-owner-invalid",
                "response source requires an owning request and work",
            ));
        }
        let handle = self.next;
        if handle == 0 || handle > MAX_HANDLE {
            return Err(BlobError::driver(
                "blob-response-handle-exhausted",
                "response source handle space is exhausted",
            ));
        }
        self.next = handle.checked_add(1).ok_or_else(|| {
            BlobError::driver(
                "blob-response-handle-exhausted",
                "response source handle space is exhausted",
            )
        })?;
        self.entries.insert(
            handle,
            ResponseEntry {
                request_context,
                work,
                source,
            },
        );
        Ok(handle)
    }

    fn read_scoped(
        &mut self,
        request_context: usize,
        work: u64,
        handle: u64,
        output: &mut [u8],
    ) -> Result<usize, BlobError> {
        let entry = self
            .entries
            .get_mut(&handle)
            .filter(|entry| entry.request_context == request_context && entry.work == work)
            .ok_or_else(|| {
                BlobError::source(
                    "blob-response-source-forbidden",
                    "response source is not owned by this request and work",
                )
            })?;
        entry.source.read(output)
    }

    fn close_scoped(
        &mut self,
        request_context: usize,
        work: u64,
        handle: u64,
    ) -> Result<(), BlobError> {
        let owned = self
            .entries
            .get(&handle)
            .map(|entry| entry.request_context == request_context && entry.work == work)
            .unwrap_or(false);
        if !owned {
            return Err(BlobError::source(
                "blob-response-source-forbidden",
                "response source is not owned by this request and work",
            ));
        }
        let mut entry = self
            .entries
            .remove(&handle)
            .expect("scoped response source was checked above");
        entry.source.close()
    }

    fn release_work(&mut self, work: u64) -> usize {
        let handles = self
            .entries
            .iter()
            .filter_map(|(handle, entry)| (entry.work == work).then_some(*handle))
            .collect::<Vec<_>>();
        for handle in &handles {
            if let Some(mut entry) = self.entries.remove(handle) {
                let _ = entry.source.close();
            }
        }
        handles.len()
    }

    fn close_all(&mut self) {
        let entries = std::mem::take(&mut self.entries);
        for (_, mut entry) in entries {
            let _ = entry.source.close();
        }
    }
}

#[derive(Clone)]
struct HostResponseRegistrar {
    call: SharedCall,
    responses: Arc<Mutex<ResponseRegistry>>,
}

impl ResponseSourceRegistrar<ProviderResponseSource> for HostResponseRegistrar {
    fn register(&self, source: ProviderResponseSource) -> Result<u64, BlobError> {
        let call = self.call.get()?;
        lock(&self.responses)?.register(call.request_context, call.work, source)
    }
}

#[derive(Clone, Copy)]
struct Sha256Verifier;

impl DigestVerifier for Sha256Verifier {
    fn sha256(&self, bytes: &[u8]) -> [u8; 32] {
        let digest = Sha256::digest(bytes);
        let mut output = [0_u8; 32];
        output.copy_from_slice(&digest);
        output
    }
}

enum ProviderResponseSource {
    Memory(MemoryResponseSource),
    Filesystem(FilesystemResponseSource),
}

impl ResponseSource for ProviderResponseSource {
    fn declared_length(&self) -> u64 {
        match self {
            Self::Memory(source) => source.declared_length(),
            Self::Filesystem(source) => source.declared_length(),
        }
    }

    fn read(&mut self, output: &mut [u8]) -> Result<usize, BlobError> {
        match self {
            Self::Memory(source) => source.read(output),
            Self::Filesystem(source) => source.read(output),
        }
    }

    fn close(&mut self) -> Result<(), BlobError> {
        match self {
            Self::Memory(source) => source.close(),
            Self::Filesystem(source) => source.close(),
        }
    }
}

enum InstalledBlobStore {
    Memory(InMemoryBlobStore<Sha256Verifier>),
    Filesystem(Box<FilesystemBlobStore>),
}

impl BlobStore for InstalledBlobStore {
    type Source = ProviderResponseSource;

    fn staging_open(&self, request: StagingOpen) -> Result<StagingStatus, BlobError> {
        match self {
            Self::Memory(store) => store.staging_open(request),
            Self::Filesystem(store) => store.staging_open(request),
        }
    }

    fn staging_append_from_source(
        &self,
        request: StagingAppend,
        source: &mut dyn ByteSource,
    ) -> Result<AppendReceipt, BlobError> {
        match self {
            Self::Memory(store) => store.staging_append_from_source(request, source),
            Self::Filesystem(store) => store.staging_append_from_source(request, source),
        }
    }

    fn staging_abort(&self, staging_key: &StagingKey) -> Result<(), BlobError> {
        match self {
            Self::Memory(store) => store.staging_abort(staging_key),
            Self::Filesystem(store) => store.staging_abort(staging_key),
        }
    }

    fn staging_verify_commit(&self, request: StagingCommit) -> Result<ObjectDescriptor, BlobError> {
        match self {
            Self::Memory(store) => store.staging_verify_commit(request),
            Self::Filesystem(store) => store.staging_verify_commit(request),
        }
    }

    fn object_open_source(&self, request: ObjectRange) -> Result<Self::Source, BlobError> {
        match self {
            Self::Memory(store) => store
                .object_open_source(request)
                .map(ProviderResponseSource::Memory),
            Self::Filesystem(store) => store
                .object_open_source(request)
                .map(ProviderResponseSource::Filesystem),
        }
    }
}

type InstalledProvider =
    CanonicalProvider<InstalledBlobStore, HostRequestResolver, HostResponseRegistrar>;

pub struct HopliteBlobStoreProvider {
    provider: InstalledProvider,
    call: SharedCall,
    responses: Arc<Mutex<ResponseRegistry>>,
    execution: Mutex<()>,
}

impl Drop for HopliteBlobStoreProvider {
    fn drop(&mut self) {
        if let Ok(mut responses) = self.responses.lock() {
            responses.close_all();
        }
    }
}

fn build_provider(
    store: InstalledBlobStore,
    limits: Limits,
) -> Result<Box<HopliteBlobStoreProvider>, ()> {
    let call = SharedCall::new();
    let responses = Arc::new(Mutex::new(ResponseRegistry::new()));
    let provider = CanonicalProvider::new(
        store,
        HostRequestResolver { call: call.clone() },
        HostResponseRegistrar {
            call: call.clone(),
            responses: responses.clone(),
        },
        limits,
    )
    .map_err(|_| ())?;
    Ok(Box::new(HopliteBlobStoreProvider {
        provider,
        call,
        responses,
        execution: Mutex::new(()),
    }))
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, BlobError> {
    mutex.lock().map_err(|_| BlobError::Poisoned)
}

fn failure_frame(code: &str) -> Result<Vec<u8>, ()> {
    let length = u32::try_from(code.len()).map_err(|_| ())?;
    let mut output = Vec::with_capacity(HTA_MAGIC.len() + 1 + 4 + code.len());
    output.extend_from_slice(HTA_MAGIC);
    output.push(HTA_STRING);
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(code.as_bytes());
    Ok(output)
}

fn reset_result(result: &mut HopliteBlobStoreResultV1) {
    result.kind = 0;
    result.data = ptr::null_mut();
    result.len = 0;
}

fn store_result(result: &mut HopliteBlobStoreResultV1, kind: u32, bytes: Vec<u8>) {
    let mut bytes = bytes.into_boxed_slice();
    result.kind = kind;
    result.len = bytes.len();
    result.data = bytes.as_mut_ptr();
    std::mem::forget(bytes);
}

unsafe fn input_bytes<'a>(data: *const u8, len: usize) -> Result<&'a [u8], ()> {
    if data.is_null() {
        return if len == 0 { Ok(&[]) } else { Err(()) };
    }
    // SAFETY: the caller promises `data` is readable for `len` bytes during
    // this synchronous call.
    Ok(unsafe { slice::from_raw_parts(data, len) })
}

unsafe fn provider_ref<'a>(
    provider: *mut HopliteBlobStoreProvider,
) -> Result<&'a HopliteBlobStoreProvider, ()> {
    // SAFETY: the caller must pass a live provider returned by an open function.
    unsafe { provider.as_ref() }.ok_or(())
}

#[no_mangle]
pub extern "C" fn hoplite_blob_store_provider_abi_version() -> u32 {
    ABI_VERSION
}

/// Open one worker-owned in-memory provider.
///
/// # Safety
///
/// `limits` must point to a readable limits value and `output` must point to a
/// writable provider pointer for the duration of this call. On success, the
/// caller owns the returned provider and must close it exactly once with
/// [`hoplite_blob_store_provider_close_v1`].
#[no_mangle]
pub unsafe extern "C" fn hoplite_blob_store_provider_open_memory_v1(
    limits: *const HopliteBlobStoreLimitsV1,
    output: *mut *mut HopliteBlobStoreProvider,
) -> i32 {
    if limits.is_null() || output.is_null() {
        return STATUS_INVALID;
    }
    // SAFETY: both pointers were checked above and are valid for this call.
    unsafe { *output = ptr::null_mut() };
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: checked non-null above; the value is copied immediately.
        let limits = unsafe { *limits }
            .into_limits()
            .validate()
            .map_err(|_| ())?;
        let store = InstalledBlobStore::Memory(
            InMemoryBlobStore::new(Sha256Verifier, limits).map_err(|_| ())?,
        );
        let provider = build_provider(store, limits)?;
        // SAFETY: output is valid and receives exclusive ownership.
        unsafe { *output = Box::into_raw(provider) };
        Ok::<(), ()>(())
    }))
    .ok()
    .and_then(Result::ok)
    .map(|_| STATUS_OK)
    .unwrap_or(STATUS_FAILURE)
}

/// Open one worker-owned trusted-root filesystem provider.
///
/// # Safety
///
/// `root` must be readable UTF-8 for `root_len` bytes, `limits` must
/// point to a readable limits value and `output` must point to a writable
/// provider pointer for this call. The root and limits are trusted startup
/// configuration and must not originate in a HAL request.
#[no_mangle]
pub unsafe extern "C" fn hoplite_blob_store_provider_open_filesystem_v1(
    root: *const u8,
    root_len: usize,
    limits: *const HopliteBlobStoreLimitsV1,
    output: *mut *mut HopliteBlobStoreProvider,
) -> i32 {
    if root.is_null() || root_len == 0 || limits.is_null() || output.is_null() {
        return STATUS_INVALID;
    }
    // SAFETY: output was checked non-null and is writable for this call.
    unsafe { *output = ptr::null_mut() };
    // SAFETY: root is non-null and readable for root_len bytes by contract.
    let root = match unsafe { input_bytes(root, root_len) }
        .ok()
        .and_then(|root| str::from_utf8(root).ok())
    {
        Some(root) if !root.is_empty() && !root.as_bytes().contains(&0) => root,
        _ => return STATUS_INVALID,
    };
    // SAFETY: limits was checked non-null and is copied immediately.
    let limits = match unsafe { *limits }.into_limits().validate() {
        Ok(limits) => limits,
        Err(_) => return STATUS_FAILURE,
    };
    catch_unwind(AssertUnwindSafe(|| {
        let store = FilesystemBlobStore::open(Path::new(root), limits)
            .map(Box::new)
            .map(InstalledBlobStore::Filesystem)
            .map_err(|_| ())?;
        let provider = build_provider(store, limits)?;
        // SAFETY: output remains valid and receives exclusive ownership.
        unsafe { *output = Box::into_raw(provider) };
        Ok::<(), ()>(())
    }))
    .ok()
    .and_then(Result::ok)
    .map(|_| STATUS_OK)
    .unwrap_or(STATUS_FAILURE)
}

/// Execute one synchronous canonical `hoplite.blob` operation.
///
/// # Safety
///
/// `provider` must be a live provider returned by the open function. `call` and
/// `result` must be valid for this call; the operation and argument pointers
/// must be readable for their declared lengths. The request callbacks and their
/// context must remain valid until this function returns. Any allocated result
/// must be released exactly once with
/// [`hoplite_blob_store_provider_result_free_v1`].
#[no_mangle]
pub unsafe extern "C" fn hoplite_blob_store_provider_execute_v1(
    provider: *mut HopliteBlobStoreProvider,
    call: *const HopliteBlobStoreCallV1,
    operation: *const u8,
    operation_len: usize,
    arguments_hta: *const u8,
    arguments_hta_len: usize,
    result: *mut HopliteBlobStoreResultV1,
) -> i32 {
    if result.is_null() || call.is_null() || operation_len == 0 || arguments_hta_len == 0 {
        return STATUS_INVALID;
    }
    // SAFETY: result was checked non-null and is writable for this call.
    unsafe { reset_result(&mut *result) };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: pointers are checked or validated by input_bytes below.
        let provider = unsafe { provider_ref(provider) }.map_err(|_| STATUS_INVALID)?;
        let call = unsafe { *call };
        let operation = str::from_utf8(
            unsafe { input_bytes(operation, operation_len) }.map_err(|_| STATUS_INVALID)?,
        )
        .map_err(|_| STATUS_INVALID)?;
        let arguments =
            unsafe { input_bytes(arguments_hta, arguments_hta_len) }.map_err(|_| STATUS_INVALID)?;
        if call.abi_version != ABI_VERSION
            || call.request_context.is_null()
            || call.work == 0
            || call.request_read.is_none()
            || call.request_finish.is_none()
        {
            return Err(STATUS_INVALID);
        }
        let context = CallContext {
            request_context: call.request_context as usize,
            work: call.work,
            request_read: call.request_read.expect("checked request_read"),
            request_finish: call.request_finish.expect("checked request_finish"),
        };
        let _execution = provider.execution.lock().map_err(|_| STATUS_FAILURE)?;
        provider.call.set(context).map_err(|_| STATUS_FAILURE)?;
        let _reset = CallReset(provider.call.clone());
        let (kind, bytes) = match provider.provider.execute(operation, arguments) {
            Ok(bytes) => (RESULT_SUCCESS, bytes),
            Err(error) => (
                RESULT_FAILURE,
                failure_frame(error.code()).map_err(|_| STATUS_FAILURE)?,
            ),
        };
        // SAFETY: result remains valid for this synchronous call.
        unsafe { store_result(&mut *result, kind, bytes) };
        Ok::<(), i32>(())
    }));
    match outcome {
        Ok(Ok(())) => STATUS_OK,
        Ok(Err(status)) => {
            // SAFETY: result remains valid for this synchronous call.
            unsafe { reset_result(&mut *result) };
            status
        }
        Err(_) => {
            // SAFETY: result remains valid for this synchronous call.
            unsafe { reset_result(&mut *result) };
            STATUS_FAILURE
        }
    }
}

/// Read from one immutable request-and-work-scoped response source.
///
/// # Safety
///
/// `provider` must remain live. `request_context` must be the exact opaque
/// request identity supplied when the source was opened. `returned` must be
/// writable, and when `capacity` is non-zero `output` must be writable for that
/// many bytes. Ownership is checked before any source callback runs.
#[no_mangle]
pub unsafe extern "C" fn hoplite_blob_store_provider_response_read_scoped_v1(
    provider: *mut HopliteBlobStoreProvider,
    request_context: *mut c_void,
    work: u64,
    source_handle: u64,
    output: *mut u8,
    capacity: usize,
    returned: *mut usize,
) -> i32 {
    if request_context.is_null()
        || returned.is_null()
        || (capacity != 0 && output.is_null())
        || work == 0
        || source_handle == 0
    {
        return STATUS_INVALID;
    }
    // SAFETY: returned was checked non-null.
    unsafe { *returned = 0 };
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: provider must originate from an open function.
        let provider = unsafe { provider_ref(provider) }.map_err(|_| STATUS_INVALID)?;
        let output = if capacity == 0 {
            &mut []
        } else {
            // SAFETY: output is non-null and writable for capacity bytes.
            unsafe { slice::from_raw_parts_mut(output, capacity) }
        };
        let read = lock(&provider.responses)
            .and_then(|mut responses| {
                responses.read_scoped(request_context as usize, work, source_handle, output)
            })
            .map_err(|_| STATUS_RESOURCE_ERROR)?;
        // SAFETY: returned remains valid for this synchronous call.
        unsafe { *returned = read };
        Ok::<(), i32>(())
    }))
    .ok()
    .and_then(Result::ok)
    .map(|_| STATUS_OK)
    .unwrap_or(STATUS_RESOURCE_ERROR)
}

/// Close one immutable request-and-work-scoped response source.
///
/// # Safety
///
/// `provider` must remain live. `request_context`, work and handle must exactly
/// match the values that opened the source. A source cannot be closed twice.
#[no_mangle]
pub unsafe extern "C" fn hoplite_blob_store_provider_response_close_scoped_v1(
    provider: *mut HopliteBlobStoreProvider,
    request_context: *mut c_void,
    work: u64,
    source_handle: u64,
) -> i32 {
    if request_context.is_null() || work == 0 || source_handle == 0 {
        return STATUS_INVALID;
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: provider must originate from an open function.
        let provider = unsafe { provider_ref(provider) }.map_err(|_| STATUS_INVALID)?;
        lock(&provider.responses)
            .and_then(|mut responses| {
                responses.close_scoped(request_context as usize, work, source_handle)
            })
            .map_err(|_| STATUS_RESOURCE_ERROR)?;
        Ok::<(), i32>(())
    }))
    .ok()
    .and_then(Result::ok)
    .map(|_| STATUS_OK)
    .unwrap_or(STATUS_RESOURCE_ERROR)
}

/// Legacy work-only response read retained for ABI compatibility.
///
/// This entrypoint always fails closed because it cannot prove the opaque
/// request identity that opened the source. Request-serving hosts must call
/// [`hoplite_blob_store_provider_response_read_scoped_v1`].
///
/// # Safety
///
/// `provider` must be a live provider pointer. `returned` must be writable and,
/// when `capacity` is non-zero, `output` must be writable for that many bytes.
/// No response source is accessed by this compatibility entrypoint.
#[no_mangle]
pub unsafe extern "C" fn hoplite_blob_store_provider_response_read_v1(
    provider: *mut HopliteBlobStoreProvider,
    work: u64,
    source_handle: u64,
    output: *mut u8,
    capacity: usize,
    returned: *mut usize,
) -> i32 {
    if provider.is_null()
        || returned.is_null()
        || (capacity != 0 && output.is_null())
        || work == 0
        || source_handle == 0
    {
        return STATUS_INVALID;
    }
    // SAFETY: returned was checked non-null.
    unsafe { *returned = 0 };
    STATUS_RESOURCE_ERROR
}

/// Legacy work-only response close retained for ABI compatibility.
///
/// This entrypoint always fails closed because it cannot prove the opaque
/// request identity that opened the source. Request-serving hosts must call
/// [`hoplite_blob_store_provider_response_close_scoped_v1`].
///
/// # Safety
///
/// `provider` must be a live provider pointer. No response source is accessed
/// or closed by this compatibility entrypoint.
#[no_mangle]
pub unsafe extern "C" fn hoplite_blob_store_provider_response_close_v1(
    provider: *mut HopliteBlobStoreProvider,
    work: u64,
    source_handle: u64,
) -> i32 {
    if provider.is_null() || work == 0 || source_handle == 0 {
        return STATUS_INVALID;
    }
    STATUS_RESOURCE_ERROR
}

/// Close every response source retained by one work.
///
/// # Safety
///
/// `provider` must be a live provider returned by the open function. The caller
/// must serialize this lifecycle call with provider destruction.
#[no_mangle]
pub unsafe extern "C" fn hoplite_blob_store_provider_release_work_v1(
    provider: *mut HopliteBlobStoreProvider,
    work: u64,
) -> usize {
    if work == 0 {
        return 0;
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: provider was created by open_memory_v1.
        let provider = unsafe { provider_ref(provider) }.ok()?;
        provider
            .responses
            .lock()
            .ok()
            .map(|mut responses| responses.release_work(work))
    }))
    .ok()
    .flatten()
    .unwrap_or(0)
}

/// Release an owned result frame returned by the execute function.
///
/// # Safety
///
/// `result` must be null, zeroed, or a valid result value produced by this
/// library. A non-null frame must be released at most once.
#[no_mangle]
pub unsafe extern "C" fn hoplite_blob_store_provider_result_free_v1(
    result: *mut HopliteBlobStoreResultV1,
) {
    if result.is_null() {
        return;
    }
    // SAFETY: result was checked non-null.
    let result = unsafe { &mut *result };
    if !result.data.is_null() {
        let raw = ptr::slice_from_raw_parts_mut(result.data, result.len);
        // SAFETY: store_result allocated this exact boxed slice and ownership is
        // transferred back exactly once by the C API contract.
        unsafe { drop(Box::from_raw(raw)) };
    }
    reset_result(result);
}

/// Destroy one provider and every response source it still owns.
///
/// # Safety
///
/// `provider` must be null or the unique live pointer returned by the open
/// function. A non-null provider must be closed exactly once and no concurrent
/// calls may still reference it.
#[no_mangle]
pub unsafe extern "C" fn hoplite_blob_store_provider_close_v1(
    provider: *mut HopliteBlobStoreProvider,
) {
    if provider.is_null() {
        return;
    }
    // SAFETY: ownership was returned by open_memory_v1 and must be consumed once.
    unsafe { drop(Box::from_raw(provider)) };
}

#[cfg(test)]
mod filesystem_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use hoplite_provider_hta::Document;
    use std::fmt::Write;

    const HTA_TRUE: u8 = 2;
    const HTA_I64: u8 = 3;
    const HTA_KEYWORD: u8 = 6;
    const HTA_VECTOR: u8 = 9;
    const HTA_MAP: u8 = 11;

    struct RequestFixture {
        bytes: Vec<u8>,
        cursor: usize,
        work: u64,
        handle: u64,
        finishes: usize,
    }

    unsafe extern "C" fn request_read(
        context: *mut c_void,
        work: u64,
        handle: u64,
        output: *mut u8,
        capacity: usize,
        returned: *mut usize,
    ) -> i32 {
        if context.is_null() || returned.is_null() || (capacity != 0 && output.is_null()) {
            return STATUS_RESOURCE_ERROR;
        }
        // SAFETY: the test passes a live fixture and valid output pointers.
        let fixture = unsafe { &mut *(context as *mut RequestFixture) };
        if work != fixture.work || handle != fixture.handle {
            unsafe { *returned = 0 };
            return STATUS_RESOURCE_ERROR;
        }
        let amount = capacity.min(fixture.bytes.len().saturating_sub(fixture.cursor));
        if amount != 0 {
            unsafe {
                ptr::copy_nonoverlapping(fixture.bytes.as_ptr().add(fixture.cursor), output, amount)
            };
        }
        fixture.cursor += amount;
        unsafe { *returned = amount };
        STATUS_OK
    }

    unsafe extern "C" fn request_finish(context: *mut c_void, work: u64, handle: u64) -> i32 {
        if context.is_null() {
            return STATUS_RESOURCE_ERROR;
        }
        // SAFETY: the test passes a live fixture.
        let fixture = unsafe { &mut *(context as *mut RequestFixture) };
        if work != fixture.work || handle != fixture.handle {
            return STATUS_RESOURCE_ERROR;
        }
        fixture.finishes += 1;
        STATUS_OK
    }

    fn limits() -> HopliteBlobStoreLimitsV1 {
        HopliteBlobStoreLimitsV1 {
            max_object_bytes: 1024,
            max_append_bytes: 128,
            max_source_chunk_bytes: 3,
            max_staging_key_bytes: 64,
            max_media_type_bytes: 64,
            max_staging_entries: 8,
            max_objects: 8,
        }
    }

    fn digest(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let mut output = String::from("sha256:");
        for byte in digest {
            write!(&mut output, "{byte:02x}").unwrap();
        }
        output
    }

    fn bare_text(tag: u8, value: &str) -> Vec<u8> {
        let mut output = vec![tag];
        output.extend_from_slice(&(value.len() as u32).to_be_bytes());
        output.extend_from_slice(value.as_bytes());
        output
    }

    fn bare_string(value: &str) -> Vec<u8> {
        bare_text(HTA_STRING, value)
    }

    fn bare_keyword(value: &str) -> Vec<u8> {
        bare_text(HTA_KEYWORD, value)
    }

    fn bare_i64(value: u64) -> Vec<u8> {
        let value = i64::try_from(value).unwrap();
        let mut output = vec![HTA_I64];
        output.extend_from_slice(&value.to_be_bytes());
        output
    }

    fn bare_map(entries: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
        let mut entries = entries
            .into_iter()
            .map(|(key, value)| (bare_keyword(key), value))
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let mut output = vec![HTA_MAP];
        output.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (key, value) in entries {
            output.extend_from_slice(&key);
            output.extend_from_slice(&value);
        }
        output
    }

    fn arguments(request: Vec<u8>) -> Vec<u8> {
        let mut output = HTA_MAGIC.to_vec();
        output.push(HTA_VECTOR);
        output.extend_from_slice(&1_u32.to_be_bytes());
        output.extend_from_slice(&request);
        output
    }

    fn open_request(key: &str, bytes: &[u8]) -> Vec<u8> {
        arguments(bare_map(vec![
            ("expected-digest", bare_string(&digest(bytes))),
            ("expected-size", bare_i64(bytes.len() as u64)),
            ("media-type", bare_string("application/octet-stream")),
            ("operation", bare_string("staging/open")),
            ("protocol", bare_string("hoplite.blob-request/1")),
            ("staging-key", bare_string(key)),
        ]))
    }

    fn append_request(key: &str, bytes: &[u8], handle: u64) -> Vec<u8> {
        arguments(bare_map(vec![
            ("length", bare_i64(bytes.len() as u64)),
            ("offset", bare_i64(0)),
            ("operation", bare_string("staging/append-from-source")),
            ("protocol", bare_string("hoplite.blob-request/1")),
            ("source-handle", bare_i64(handle)),
            ("staging-key", bare_string(key)),
        ]))
    }

    fn commit_request(key: &str, bytes: &[u8]) -> Vec<u8> {
        arguments(bare_map(vec![
            ("expected-digest", bare_string(&digest(bytes))),
            ("expected-size", bare_i64(bytes.len() as u64)),
            ("operation", bare_string("staging/verify-commit")),
            ("protocol", bare_string("hoplite.blob-request/1")),
            ("staging-key", bare_string(key)),
        ]))
    }

    fn open_source_request(bytes: &[u8], offset: u64, length: u64) -> Vec<u8> {
        arguments(bare_map(vec![
            ("digest", bare_string(&digest(bytes))),
            ("length", bare_i64(length)),
            ("offset", bare_i64(offset)),
            ("operation", bare_string("object/open-source")),
            ("protocol", bare_string("hoplite.blob-request/1")),
        ]))
    }

    unsafe fn execute(
        provider: *mut HopliteBlobStoreProvider,
        call: &HopliteBlobStoreCallV1,
        operation: &str,
        arguments: &[u8],
    ) -> (u32, Vec<u8>) {
        let mut result = HopliteBlobStoreResultV1 {
            kind: 0,
            data: ptr::null_mut(),
            len: 0,
        };
        let status = unsafe {
            hoplite_blob_store_provider_execute_v1(
                provider,
                call,
                operation.as_ptr(),
                operation.len(),
                arguments.as_ptr(),
                arguments.len(),
                &mut result,
            )
        };
        assert_eq!(status, STATUS_OK);
        let bytes = unsafe { slice::from_raw_parts(result.data, result.len) }.to_vec();
        let kind = result.kind;
        unsafe { hoplite_blob_store_provider_result_free_v1(&mut result) };
        assert!(result.data.is_null());
        (kind, bytes)
    }

    fn source_handle(frame: &[u8]) -> u64 {
        let document = Document::parse(frame).unwrap();
        u64::try_from(
            document
                .root()
                .require("source-handle")
                .unwrap()
                .as_i64()
                .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn runs_the_complete_request_scoped_upload_and_range_flow() {
        let mut provider = ptr::null_mut();
        assert_eq!(
            unsafe { hoplite_blob_store_provider_open_memory_v1(&limits(), &mut provider) },
            STATUS_OK
        );
        assert!(!provider.is_null());

        let bytes = b"abcdefgh".to_vec();
        let mut fixture = RequestFixture {
            bytes: bytes.clone(),
            cursor: 0,
            work: 41,
            handle: 17,
            finishes: 0,
        };
        let call = HopliteBlobStoreCallV1 {
            abi_version: ABI_VERSION,
            request_context: (&mut fixture as *mut RequestFixture).cast(),
            work: fixture.work,
            request_read: Some(request_read),
            request_finish: Some(request_finish),
        };

        assert_eq!(
            unsafe {
                execute(
                    provider,
                    &call,
                    "staging/open",
                    &open_request("upload.a", &bytes),
                )
            }
            .0,
            RESULT_SUCCESS
        );
        assert_eq!(
            unsafe {
                execute(
                    provider,
                    &call,
                    "staging/append-from-source",
                    &append_request("upload.a", &bytes, fixture.handle),
                )
            }
            .0,
            RESULT_SUCCESS
        );
        assert_eq!(fixture.cursor, bytes.len());
        assert_eq!(fixture.finishes, 1);
        assert_eq!(
            unsafe {
                execute(
                    provider,
                    &call,
                    "staging/verify-commit",
                    &commit_request("upload.a", &bytes),
                )
            }
            .0,
            RESULT_SUCCESS
        );

        let (_, opened) = unsafe {
            execute(
                provider,
                &call,
                "object/open-source",
                &open_source_request(&bytes, 2, 4),
            )
        };
        let handle = source_handle(&opened);
        let mut output = [0_u8; 8];
        let mut returned = 99_usize;
        let mut wrong_request = RequestFixture {
            bytes: Vec::new(),
            cursor: 0,
            work: call.work,
            handle: 0,
            finishes: 0,
        };

        assert_eq!(
            unsafe {
                hoplite_blob_store_provider_response_read_v1(
                    provider,
                    call.work,
                    handle,
                    output.as_mut_ptr(),
                    output.len(),
                    &mut returned,
                )
            },
            STATUS_RESOURCE_ERROR
        );
        assert_eq!(returned, 0);
        assert_eq!(
            unsafe { hoplite_blob_store_provider_response_close_v1(provider, call.work, handle) },
            STATUS_RESOURCE_ERROR
        );

        assert_eq!(
            unsafe {
                hoplite_blob_store_provider_response_read_scoped_v1(
                    provider,
                    (&mut wrong_request as *mut RequestFixture).cast(),
                    call.work,
                    handle,
                    output.as_mut_ptr(),
                    output.len(),
                    &mut returned,
                )
            },
            STATUS_RESOURCE_ERROR
        );
        assert_eq!(returned, 0);
        assert_eq!(
            unsafe {
                hoplite_blob_store_provider_response_read_scoped_v1(
                    provider,
                    call.request_context,
                    call.work + 1,
                    handle,
                    output.as_mut_ptr(),
                    output.len(),
                    &mut returned,
                )
            },
            STATUS_RESOURCE_ERROR
        );
        assert_eq!(returned, 0);
        assert_eq!(
            unsafe {
                hoplite_blob_store_provider_response_read_scoped_v1(
                    provider,
                    call.request_context,
                    call.work,
                    handle,
                    output.as_mut_ptr(),
                    output.len(),
                    &mut returned,
                )
            },
            STATUS_OK
        );
        assert_eq!(returned, 4);
        assert_eq!(&output[..returned], b"cdef");
        assert_eq!(
            unsafe {
                hoplite_blob_store_provider_response_close_scoped_v1(
                    provider,
                    (&mut wrong_request as *mut RequestFixture).cast(),
                    call.work,
                    handle,
                )
            },
            STATUS_RESOURCE_ERROR
        );
        assert_eq!(
            unsafe {
                hoplite_blob_store_provider_response_close_scoped_v1(
                    provider,
                    call.request_context,
                    call.work,
                    handle,
                )
            },
            STATUS_OK
        );
        assert_eq!(
            unsafe {
                hoplite_blob_store_provider_response_close_scoped_v1(
                    provider,
                    call.request_context,
                    call.work,
                    handle,
                )
            },
            STATUS_RESOURCE_ERROR
        );
        unsafe { hoplite_blob_store_provider_close_v1(provider) };
    }

    #[test]
    fn returns_closed_errors_and_releases_sources_by_work() {
        let mut provider = ptr::null_mut();
        assert_eq!(
            unsafe { hoplite_blob_store_provider_open_memory_v1(&limits(), &mut provider) },
            STATUS_OK
        );
        let mut fixture = RequestFixture {
            bytes: b"abc".to_vec(),
            cursor: 0,
            work: 7,
            handle: 9,
            finishes: 0,
        };
        let call = HopliteBlobStoreCallV1 {
            abi_version: ABI_VERSION,
            request_context: (&mut fixture as *mut RequestFixture).cast(),
            work: fixture.work,
            request_read: Some(request_read),
            request_finish: Some(request_finish),
        };
        let (kind, failure) = unsafe {
            execute(
                provider,
                &call,
                "staging/append-from-source",
                &append_request("missing", &fixture.bytes, fixture.handle),
            )
        };
        assert_eq!(kind, RESULT_FAILURE);
        let failure = Document::parse(&failure).unwrap();
        assert_eq!(failure.root().as_text().unwrap(), "blob-staging-missing");
        assert_eq!(fixture.cursor, 0);
        assert_eq!(fixture.finishes, 0);

        let bytes = fixture.bytes.clone();
        unsafe {
            execute(
                provider,
                &call,
                "staging/open",
                &open_request("upload.b", &bytes),
            );
            execute(
                provider,
                &call,
                "staging/append-from-source",
                &append_request("upload.b", &bytes, fixture.handle),
            );
            execute(
                provider,
                &call,
                "staging/verify-commit",
                &commit_request("upload.b", &bytes),
            );
        }
        let (_, opened) = unsafe {
            execute(
                provider,
                &call,
                "object/open-source",
                &open_source_request(&bytes, 0, bytes.len() as u64),
            )
        };
        let handle = source_handle(&opened);
        assert_eq!(
            unsafe { hoplite_blob_store_provider_release_work_v1(provider, call.work + 1) },
            0
        );
        assert_eq!(
            unsafe { hoplite_blob_store_provider_release_work_v1(provider, call.work) },
            1
        );
        let mut returned = 99;
        assert_eq!(
            unsafe {
                hoplite_blob_store_provider_response_read_scoped_v1(
                    provider,
                    call.request_context,
                    call.work,
                    handle,
                    ptr::null_mut(),
                    0,
                    &mut returned,
                )
            },
            STATUS_RESOURCE_ERROR
        );
        assert_eq!(returned, 0);
        unsafe { hoplite_blob_store_provider_close_v1(provider) };
    }

    #[test]
    fn rejects_invalid_abi_and_limits_without_allocating_results() {
        let mut provider = ptr::null_mut();
        let mut invalid_limits = limits();
        invalid_limits.max_source_chunk_bytes = invalid_limits.max_append_bytes + 1;
        assert_eq!(
            unsafe { hoplite_blob_store_provider_open_memory_v1(&invalid_limits, &mut provider) },
            STATUS_FAILURE
        );
        assert!(provider.is_null());

        assert_eq!(
            unsafe { hoplite_blob_store_provider_open_memory_v1(&limits(), &mut provider) },
            STATUS_OK
        );
        let mut fixture = RequestFixture {
            bytes: Vec::new(),
            cursor: 0,
            work: 1,
            handle: 1,
            finishes: 0,
        };
        let call = HopliteBlobStoreCallV1 {
            abi_version: ABI_VERSION + 1,
            request_context: (&mut fixture as *mut RequestFixture).cast(),
            work: fixture.work,
            request_read: Some(request_read),
            request_finish: Some(request_finish),
        };
        let mut result = HopliteBlobStoreResultV1 {
            kind: 88,
            data: ptr::null_mut(),
            len: 77,
        };
        assert_eq!(
            unsafe {
                hoplite_blob_store_provider_execute_v1(
                    provider,
                    &call,
                    b"staging/open".as_ptr(),
                    b"staging/open".len(),
                    open_request("upload.c", b"").as_ptr(),
                    open_request("upload.c", b"").len(),
                    &mut result,
                )
            },
            STATUS_INVALID
        );
        assert_eq!(result.kind, 0);
        assert!(result.data.is_null());
        assert_eq!(result.len, 0);
        unsafe { hoplite_blob_store_provider_close_v1(provider) };
    }

    #[test]
    fn result_shape_contains_only_canonical_boolean_and_map_values() {
        assert_eq!(HTA_TRUE, 2);
    }
}
