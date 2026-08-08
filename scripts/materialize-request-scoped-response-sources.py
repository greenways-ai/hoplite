from pathlib import Path

RUST = Path("core/abi/blob-store-provider-ffi/src/lib.rs")
RUST_TESTS = Path("core/abi/blob-store-provider-ffi/src/filesystem_tests.rs")
FFI_HEADER = Path("core/abi/blob-store-provider-ffi/include/hoplite_blob_store_provider.h")
HOST_HEADER = Path("core/nginx/hoplite_blob_host_provider.h")
HOST_SOURCE = Path("core/nginx/hoplite_blob_host_provider.c")
HOST_TEST = Path("core/nginx/tests/blob_host_provider.c")
SELF = Path(__file__)

source = RUST.read_text()

old = """struct ResponseEntry {
    work: u64,
    source: ProviderResponseSource,
}
"""
new = """struct ResponseEntry {
    request_context: usize,
    work: u64,
    source: ProviderResponseSource,
}
"""
if source.count(old) != 1:
    raise SystemExit("expected one response entry")
source = source.replace(old, new)

old = """    fn register(&mut self, work: u64, source: ProviderResponseSource) -> Result<u64, BlobError> {
        if work == 0 {
            return Err(BlobError::driver(
                "blob-response-work-invalid",
                "response source requires an owning work",
            ));
        }
"""
new = """    fn register(
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
"""
if source.count(old) != 1:
    raise SystemExit("expected one response registration function")
source = source.replace(old, new)

old = "self.entries.insert(handle, ResponseEntry { work, source });"
new = """self.entries.insert(
            handle,
            ResponseEntry {
                request_context,
                work,
                source,
            },
        );"""
if source.count(old) != 1:
    raise SystemExit("expected one response entry insertion")
source = source.replace(old, new)

read_marker = """    fn close(&mut self, work: u64, handle: u64) -> Result<(), BlobError> {
"""
read_scoped = """    fn read_scoped(
        &mut self,
        request_context: usize,
        work: u64,
        handle: u64,
        output: &mut [u8],
    ) -> Result<usize, BlobError> {
        let entry = self
            .entries
            .get_mut(&handle)
            .filter(|entry| {
                entry.request_context == request_context && entry.work == work
            })
            .ok_or_else(|| {
                BlobError::source(
                    "blob-response-source-forbidden",
                    "response source is not owned by this request and work",
                )
            })?;
        entry.source.read(output)
    }

    fn close(&mut self, work: u64, handle: u64) -> Result<(), BlobError> {
"""
if source.count(read_marker) != 1:
    raise SystemExit("expected one response close marker")
source = source.replace(read_marker, read_scoped)

close_marker = """    fn release_work(&mut self, work: u64) -> usize {
"""
close_scoped = """    fn close_scoped(
        &mut self,
        request_context: usize,
        work: u64,
        handle: u64,
    ) -> Result<(), BlobError> {
        let owned = self
            .entries
            .get(&handle)
            .map(|entry| {
                entry.request_context == request_context && entry.work == work
            })
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
"""
if source.count(close_marker) != 1:
    raise SystemExit("expected one response release marker")
source = source.replace(close_marker, close_scoped)

old = "lock(&self.responses)?.register(call.work, source)"
new = "lock(&self.responses)?.register(call.request_context, call.work, source)"
if source.count(old) != 1:
    raise SystemExit("expected one host response registration")
source = source.replace(old, new)

export_marker = """/// Read from one immutable work-scoped response source.
"""
scoped_exports = """/// Read from one immutable request-and-work-scoped response source.
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
                responses.read_scoped(
                    request_context as usize,
                    work,
                    source_handle,
                    output,
                )
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
                responses.close_scoped(
                    request_context as usize,
                    work,
                    source_handle,
                )
            })
            .map_err(|_| STATUS_RESOURCE_ERROR)?;
        Ok::<(), i32>(())
    }))
    .ok()
    .and_then(Result::ok)
    .map(|_| STATUS_OK)
    .unwrap_or(STATUS_RESOURCE_ERROR)
}

/// Read from one immutable work-scoped response source.
"""
if source.count(export_marker) != 1:
    raise SystemExit("expected one response export marker")
source = source.replace(export_marker, scoped_exports)
RUST.write_text(source)

header = FFI_HEADER.read_text()
marker = """int32_t hoplite_blob_store_provider_response_close_v1(
    hoplite_blob_store_provider_t *provider,
    uint64_t work,
    uint64_t source_handle);
"""
addition = marker + """
/*
 * Stronger scoped variants used by request-serving hosts. The exact opaque
 * request identity and work must match the source-opening call before bytes are
 * read or the source is closed.
 */
int32_t hoplite_blob_store_provider_response_read_scoped_v1(
    hoplite_blob_store_provider_t *provider,
    void *request_context,
    uint64_t work,
    uint64_t source_handle,
    uint8_t *output,
    size_t capacity,
    size_t *returned);

int32_t hoplite_blob_store_provider_response_close_scoped_v1(
    hoplite_blob_store_provider_t *provider,
    void *request_context,
    uint64_t work,
    uint64_t source_handle);
"""
if header.count(marker) != 1:
    raise SystemExit("expected one FFI response close declaration")
FFI_HEADER.write_text(header.replace(marker, addition))

host_header = HOST_HEADER.read_text()
marker = """/* Release immutable response sources retained by one completed work. */
size_t hoplite_blob_host_provider_release_work_v1(uint64_t work);
"""
addition = """/*
 * Read or close an immutable source only through its exact owning request and
 * work. A numeric source handle alone grants no access.
 */
int32_t hoplite_blob_host_provider_response_read_v1(
    void *request_context,
    uint64_t work,
    uint64_t source_handle,
    uint8_t *output,
    size_t capacity,
    size_t *returned);

int32_t hoplite_blob_host_provider_response_close_v1(
    void *request_context,
    uint64_t work,
    uint64_t source_handle);

/* Release immutable response sources retained by one completed work. */
size_t hoplite_blob_host_provider_release_work_v1(uint64_t work);
"""
if host_header.count(marker) != 1:
    raise SystemExit("expected one host release declaration")
HOST_HEADER.write_text(host_header.replace(marker, addition))

host_source = HOST_SOURCE.read_text()
marker = """size_t
hoplite_blob_host_provider_release_work_v1(uint64_t work)
"""
addition = """int32_t
hoplite_blob_host_provider_response_read_v1(
    void *request_context,
    uint64_t work,
    uint64_t source_handle,
    uint8_t *output,
    size_t capacity,
    size_t *returned)
{
    if (returned != NULL) {
        *returned = 0;
    }
    if (hoplite_blob_state != HOPLITE_BLOB_HOST_READY
        || hoplite_blob_provider == NULL
        || request_context == NULL
        || work == 0 || source_handle == 0
        || output == NULL || capacity == 0 || returned == NULL)
    {
        return HOPLITE_BLOB_HOST_PROVIDER_ERROR;
    }
    return hoplite_blob_store_provider_response_read_scoped_v1(
               hoplite_blob_provider,
               request_context,
               work,
               source_handle,
               output,
               capacity,
               returned) == HOPLITE_BLOB_STORE_PROVIDER_OK
        ? HOPLITE_BLOB_HOST_PROVIDER_OK
        : HOPLITE_BLOB_HOST_PROVIDER_ERROR;
}

int32_t
hoplite_blob_host_provider_response_close_v1(
    void *request_context,
    uint64_t work,
    uint64_t source_handle)
{
    if (hoplite_blob_state != HOPLITE_BLOB_HOST_READY
        || hoplite_blob_provider == NULL
        || request_context == NULL
        || work == 0 || source_handle == 0)
    {
        return HOPLITE_BLOB_HOST_PROVIDER_ERROR;
    }
    return hoplite_blob_store_provider_response_close_scoped_v1(
               hoplite_blob_provider,
               request_context,
               work,
               source_handle) == HOPLITE_BLOB_STORE_PROVIDER_OK
        ? HOPLITE_BLOB_HOST_PROVIDER_OK
        : HOPLITE_BLOB_HOST_PROVIDER_ERROR;
}

size_t
hoplite_blob_host_provider_release_work_v1(uint64_t work)
"""
if host_source.count(marker) != 1:
    raise SystemExit("expected one host release implementation")
HOST_SOURCE.write_text(host_source.replace(marker, addition))

host_test = HOST_TEST.read_text()
marker = """size_t
hoplite_blob_store_provider_release_work_v1(
"""
stubs = """int32_t
hoplite_blob_store_provider_response_read_scoped_v1(
    hoplite_blob_store_provider_t *provider,
    void *request_context,
    uint64_t work,
    uint64_t source_handle,
    uint8_t *output,
    size_t capacity,
    size_t *returned)
{
    static const uint8_t source[] = "source";
    size_t amount;

    assert(provider == &fake_provider);
    assert(request_context == &fake_provider);
    assert(work == 71);
    assert(source_handle == 19);
    assert(output != NULL);
    assert(returned != NULL);
    amount = capacity < sizeof(source) - 1 ? capacity : sizeof(source) - 1;
    memcpy(output, source, amount);
    *returned = amount;
    return HOPLITE_BLOB_STORE_PROVIDER_OK;
}

int32_t
hoplite_blob_store_provider_response_close_scoped_v1(
    hoplite_blob_store_provider_t *provider,
    void *request_context,
    uint64_t work,
    uint64_t source_handle)
{
    assert(provider == &fake_provider);
    assert(request_context == &fake_provider);
    assert(work == 71);
    assert(source_handle == 19);
    return HOPLITE_BLOB_STORE_PROVIDER_OK;
}

size_t
hoplite_blob_store_provider_release_work_v1(
"""
if host_test.count(marker) != 1:
    raise SystemExit("expected one host test release stub")
host_test = host_test.replace(marker, stubs)

marker = """    assert(execute_count == 5);
"""
checks = """    {
        uint8_t output[8];
        size_t returned = 99;
        assert(hoplite_blob_host_provider_response_read_v1(
                   &fake_provider, 71, 19,
                   output, sizeof(output), &returned)
               == HOPLITE_BLOB_HOST_PROVIDER_OK);
        assert(returned == sizeof("source") - 1);
        assert(memcmp(output, "source", returned) == 0);
        returned = 99;
        assert(hoplite_blob_host_provider_response_read_v1(
                   NULL, 71, 19,
                   output, sizeof(output), &returned)
               == HOPLITE_BLOB_HOST_PROVIDER_ERROR);
        assert(returned == 0);
        assert(hoplite_blob_host_provider_response_close_v1(
                   &fake_provider, 71, 19)
               == HOPLITE_BLOB_HOST_PROVIDER_OK);
    }

    assert(execute_count == 5);
"""
if host_test.count(marker) != 1:
    raise SystemExit("expected one host test assertion marker")
HOST_TEST.write_text(host_test.replace(marker, checks))

rust_tests = RUST_TESTS.read_text()
old = """    let handle = source_handle(&result);

    let mut buffer = [0_u8; 5];
    let mut returned = 99_usize;
    assert_eq!(
        unsafe {
            hoplite_blob_store_provider_response_read_v1(
                provider,
                work + 1,
                handle,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut returned,
            )
        },
        STATUS_RESOURCE_ERROR
    );
    assert_eq!(returned, 0);

    let mut output = Vec::new();
"""
new = """    let handle = source_handle(&result);

    let mut buffer = [0_u8; 5];
    let mut returned = 99_usize;
    let mut wrong_request = RequestFixture::empty(work);
    assert_eq!(
        unsafe {
            hoplite_blob_store_provider_response_read_scoped_v1(
                provider,
                &mut wrong_request as *mut RequestFixture as *mut c_void,
                work,
                handle,
                buffer.as_mut_ptr(),
                buffer.len(),
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
                &mut fixture as *mut RequestFixture as *mut c_void,
                work + 1,
                handle,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut returned,
            )
        },
        STATUS_RESOURCE_ERROR
    );
    assert_eq!(returned, 0);

    let mut output = Vec::new();
"""
if rust_tests.count(old) != 1:
    raise SystemExit("expected one filesystem response read test block")
rust_tests = rust_tests.replace(old, new)

old = """            hoplite_blob_store_provider_response_read_v1(
                provider,
                work,
                handle,
"""
new = """            hoplite_blob_store_provider_response_read_scoped_v1(
                provider,
                &mut fixture as *mut RequestFixture as *mut c_void,
                work,
                handle,
"""
if rust_tests.count(old) != 1:
    raise SystemExit("expected one correct response read call")
rust_tests = rust_tests.replace(old, new)

old = """        unsafe { hoplite_blob_store_provider_response_close_v1(provider, work, handle) },
"""
new = """        unsafe {
            hoplite_blob_store_provider_response_close_scoped_v1(
                provider,
                &mut fixture as *mut RequestFixture as *mut c_void,
                work,
                handle,
            )
        },
"""
if rust_tests.count(old) != 2:
    raise SystemExit("expected two scoped response close calls")
rust_tests = rust_tests.replace(old, new)
RUST_TESTS.write_text(rust_tests)

SELF.unlink()
