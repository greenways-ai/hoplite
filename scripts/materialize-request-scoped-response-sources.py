from pathlib import Path

RUST = Path("core/abi/blob-store-provider-ffi/src/lib.rs")
FFI_HEADER = Path("core/abi/blob-store-provider-ffi/include/hoplite_blob_store_provider.h")
HOST_TEST = Path("core/nginx/tests/blob_host_provider.c")
SELF = Path(__file__)

source = RUST.read_text()

old = """//! are registered under positive handles that are usable only by their owning
//! work.
"""
new = """//! are registered under positive handles that are usable only by their owning
//! request and work.
"""
if source.count(old) != 1:
    raise SystemExit("expected one response ownership module comment")
source = source.replace(old, new)

legacy_registry_read = """    fn read(&mut self, work: u64, handle: u64, output: &mut [u8]) -> Result<usize, BlobError> {
        let entry = self
            .entries
            .get_mut(&handle)
            .filter(|entry| entry.work == work)
            .ok_or_else(|| {
                BlobError::source(
                    "blob-response-source-forbidden",
                    "response source is not owned by this work",
                )
            })?;
        entry.source.read(output)
    }

"""
if source.count(legacy_registry_read) != 1:
    raise SystemExit("expected one legacy response read path")
source = source.replace(legacy_registry_read, "")

legacy_registry_close = """    fn close(&mut self, work: u64, handle: u64) -> Result<(), BlobError> {
        let owned = self
            .entries
            .get(&handle)
            .map(|entry| entry.work == work)
            .unwrap_or(false);
        if !owned {
            return Err(BlobError::source(
                "blob-response-source-forbidden",
                "response source is not owned by this work",
            ));
        }
        let mut entry = self
            .entries
            .remove(&handle)
            .expect("owned response source was checked above");
        entry.source.close()
    }

"""
if source.count(legacy_registry_close) != 1:
    raise SystemExit("expected one legacy response close path")
source = source.replace(legacy_registry_close, "")

legacy_start = source.index("/// Read from one immutable work-scoped response source.\n")
legacy_end = source.index("/// Close every response source retained by one work.\n", legacy_start)
legacy_exports = """/// Legacy work-only response read retained for ABI compatibility.
///
/// This entrypoint always fails closed because it cannot prove the opaque
/// request identity that opened the source. Request-serving hosts must call
/// [`hoplite_blob_store_provider_response_read_scoped_v1`].
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

"""
source = source[:legacy_start] + legacy_exports + source[legacy_end:]

old_name = "fn runs_the_complete_work_scoped_upload_and_range_flow()"
new_name = "fn runs_the_complete_request_scoped_upload_and_range_flow()"
if source.count(old_name) != 1:
    raise SystemExit("expected one memory response flow test")
source = source.replace(old_name, new_name)

function_start = source.index(new_name)
section_start = source.index("        let handle = source_handle(&opened);\n", function_start)
section_end = source.index(
    "        unsafe { hoplite_blob_store_provider_close_v1(provider) };\n",
    section_start,
)
replacement = """        let handle = source_handle(&opened);
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
"""
source = source[:section_start] + replacement + source[section_end:]

release_test_start = source.index("fn returns_closed_errors_and_releases_sources_by_work()")
old = """        let mut returned = 99;
        assert_eq!(
            unsafe {
                hoplite_blob_store_provider_response_read_v1(
                    provider,
                    call.work,
                    handle,
                    ptr::null_mut(),
                    0,
                    &mut returned,
                )
            },
            STATUS_RESOURCE_ERROR
        );
"""
new = """        let mut returned = 99;
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
"""
release_tail = source[release_test_start:]
if release_tail.count(old) != 1:
    raise SystemExit("expected one released response read assertion")
source = source[:release_test_start] + release_tail.replace(old, new, 1)

RUST.write_text(source)

header = FFI_HEADER.read_text()
old = """/*
 * Read or close an immutable response source registered by object/open-source.
 * The exact owning work must match; a numeric handle alone is never authority.
 */
"""
new = """/*
 * Legacy work-only entrypoints retained for ABI compatibility. They always
 * fail closed because a request identity is required for response-source
 * authority. Request-serving hosts must use the scoped variants below.
 */
"""
if header.count(old) != 1:
    raise SystemExit("expected one legacy response declaration comment")
FFI_HEADER.write_text(header.replace(old, new))

host_test = HOST_TEST.read_text()
old = """static size_t release_count;
static size_t succeed_count;
"""
new = """static size_t release_count;
static size_t response_read_count;
static size_t response_close_count;
static size_t succeed_count;
"""
if host_test.count(old) != 1:
    raise SystemExit("expected host test counters")
host_test = host_test.replace(old, new)

old = """    assert(provider == &fake_provider);
    assert(request_context == &fake_provider);
    assert(work == 71);
    assert(source_handle == 19);
    assert(output != NULL);
    assert(returned != NULL);
    amount = capacity < sizeof(source) - 1 ? capacity : sizeof(source) - 1;
"""
new = """    assert(provider == &fake_provider);
    assert(output != NULL);
    assert(returned != NULL);
    if (request_context != &fake_provider || work != 71 || source_handle != 19) {
        *returned = 0;
        return HOPLITE_BLOB_STORE_PROVIDER_RESOURCE_ERROR;
    }
    response_read_count++;
    amount = capacity < sizeof(source) - 1 ? capacity : sizeof(source) - 1;
"""
if host_test.count(old) != 1:
    raise SystemExit("expected host scoped read stub")
host_test = host_test.replace(old, new)

old = """    assert(provider == &fake_provider);
    assert(request_context == &fake_provider);
    assert(work == 71);
    assert(source_handle == 19);
    return HOPLITE_BLOB_STORE_PROVIDER_OK;
}

size_t
hoplite_blob_store_provider_release_work_v1(
"""
new = """    assert(provider == &fake_provider);
    if (request_context != &fake_provider || work != 71 || source_handle != 19) {
        return HOPLITE_BLOB_STORE_PROVIDER_RESOURCE_ERROR;
    }
    response_close_count++;
    return HOPLITE_BLOB_STORE_PROVIDER_OK;
}

size_t
hoplite_blob_store_provider_release_work_v1(
"""
if host_test.count(old) != 1:
    raise SystemExit("expected host scoped close stub")
host_test = host_test.replace(old, new)

old_block = """    assert(hoplite_blob_host_provider_release_work_v1(71) == 2);
    assert(release_count == 1);
    assert(released_work == 71);
    assert(hoplite_blob_host_provider_release_work_v1(0) == 0);
    assert(release_count == 1);

    {
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

"""
new_block = """    {
        uint8_t output[8];
        size_t returned = 99;
        assert(hoplite_blob_host_provider_response_read_v1(
                   &fake_provider, 71, 19,
                   output, sizeof(output), &returned)
               == HOPLITE_BLOB_HOST_PROVIDER_OK);
        assert(response_read_count == 1);
        assert(returned == sizeof("source") - 1);
        assert(memcmp(output, "source", returned) == 0);

        returned = 99;
        assert(hoplite_blob_host_provider_response_read_v1(
                   &service, 71, 19,
                   output, sizeof(output), &returned)
               == HOPLITE_BLOB_HOST_PROVIDER_ERROR);
        assert(response_read_count == 1);
        assert(returned == 0);

        returned = 99;
        assert(hoplite_blob_host_provider_response_read_v1(
                   NULL, 71, 19,
                   output, sizeof(output), &returned)
               == HOPLITE_BLOB_HOST_PROVIDER_ERROR);
        assert(response_read_count == 1);
        assert(returned == 0);

        assert(hoplite_blob_host_provider_response_close_v1(
                   &service, 71, 19)
               == HOPLITE_BLOB_HOST_PROVIDER_ERROR);
        assert(response_close_count == 0);
        assert(hoplite_blob_host_provider_response_close_v1(
                   &fake_provider, 71, 19)
               == HOPLITE_BLOB_HOST_PROVIDER_OK);
        assert(response_close_count == 1);
    }

    assert(hoplite_blob_host_provider_release_work_v1(71) == 2);
    assert(release_count == 1);
    assert(released_work == 71);
    assert(hoplite_blob_host_provider_release_work_v1(0) == 0);
    assert(release_count == 1);

"""
if host_test.count(old_block) != 1:
    raise SystemExit("expected host response ownership block")
host_test = host_test.replace(old_block, new_block)
HOST_TEST.write_text(host_test)

SELF.unlink()
