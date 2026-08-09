use super::*;
use hoplite_provider_hta::Document;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

const HTA_I64: u8 = 3;
const HTA_KEYWORD: u8 = 6;
const HTA_VECTOR: u8 = 9;
const HTA_MAP: u8 = 11;

struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "hoplite-blob-provider-filesystem-{}-{nonce}",
            process::id()
        ));
        fs::create_dir_all(&path).expect("create filesystem provider root");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct RequestFixture {
    bytes: Vec<u8>,
    cursor: usize,
    work: u64,
    handle: u64,
    finishes: usize,
}

impl RequestFixture {
    fn empty(work: u64) -> Self {
        Self {
            bytes: Vec::new(),
            cursor: 0,
            work,
            handle: 1,
            finishes: 0,
        }
    }

    fn with_bytes(work: u64, handle: u64, bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            cursor: 0,
            work,
            handle,
            finishes: 0,
        }
    }
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
    // SAFETY: the test passes a live fixture and writable output pointers.
    let fixture = unsafe { &mut *(context as *mut RequestFixture) };
    if work != fixture.work || handle != fixture.handle {
        // SAFETY: returned was checked above.
        unsafe { *returned = 0 };
        return STATUS_RESOURCE_ERROR;
    }
    let amount = capacity.min(fixture.bytes.len().saturating_sub(fixture.cursor));
    if amount != 0 {
        // SAFETY: output is writable for capacity bytes and amount is bounded.
        unsafe {
            ptr::copy_nonoverlapping(fixture.bytes.as_ptr().add(fixture.cursor), output, amount)
        };
    }
    fixture.cursor += amount;
    // SAFETY: returned was checked above.
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
        max_object_bytes: 1024 * 1024,
        max_append_bytes: 128 * 1024,
        max_source_chunk_bytes: 7,
        max_staging_key_bytes: 128,
        max_media_type_bytes: 128,
        max_staging_entries: 32,
        max_objects: 32,
    }
}

fn open_provider(root: &TempRoot) -> *mut HopliteBlobStoreProvider {
    let root = root.path().to_str().expect("temporary root must be UTF-8");
    let limits = limits();
    let mut provider = ptr::null_mut();
    let status = unsafe {
        hoplite_blob_store_provider_open_filesystem_v1(
            root.as_ptr(),
            root.len(),
            &limits,
            &mut provider,
        )
    };
    assert_eq!(status, STATUS_OK);
    assert!(!provider.is_null());
    provider
}

fn execute(
    provider: *mut HopliteBlobStoreProvider,
    fixture: &mut RequestFixture,
    operation: &str,
    arguments: &[u8],
) -> (u32, Vec<u8>) {
    let call = HopliteBlobStoreCallV1 {
        abi_version: ABI_VERSION,
        request_context: fixture as *mut RequestFixture as *mut c_void,
        work: fixture.work,
        request_read: Some(request_read),
        request_finish: Some(request_finish),
    };
    let mut result = HopliteBlobStoreResultV1 {
        kind: 0,
        data: ptr::null_mut(),
        len: 0,
    };
    let status = unsafe {
        hoplite_blob_store_provider_execute_v1(
            provider,
            &call,
            operation.as_ptr(),
            operation.len(),
            arguments.as_ptr(),
            arguments.len(),
            &mut result,
        )
    };
    assert_eq!(status, STATUS_OK);
    assert!(!result.data.is_null());
    assert_ne!(result.len, 0);
    let bytes = unsafe { slice::from_raw_parts(result.data, result.len) }.to_vec();
    let kind = result.kind;
    unsafe { hoplite_blob_store_provider_result_free_v1(&mut result) };
    assert!(result.data.is_null());
    assert_eq!(result.len, 0);
    (kind, bytes)
}

fn digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::from("sha256:");
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("write digest");
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
    let value = i64::try_from(value).expect("test integer must be portable");
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

fn append_request(key: &str, offset: u64, bytes: &[u8], handle: u64) -> Vec<u8> {
    arguments(bare_map(vec![
        ("length", bare_i64(bytes.len() as u64)),
        ("offset", bare_i64(offset)),
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

fn source_handle(result: &[u8]) -> u64 {
    let document = Document::parse(result).expect("result must be canonical HTA");
    let value = document
        .root()
        .map_get("source-handle")
        .expect("result map must parse")
        .expect("source handle must exist")
        .as_i64()
        .expect("source handle must be an integer");
    u64::try_from(value).expect("source handle must be positive")
}

#[test]
fn durable_provider_survives_restart_and_preserves_work_scoped_sources() {
    let root = TempRoot::new();
    let bytes = b"persistent canonical blob bytes";
    let work = 41;
    let source = 7;

    let provider = open_provider(&root);
    let mut fixture = RequestFixture::empty(work);
    let (kind, _) = execute(
        provider,
        &mut fixture,
        "staging/open",
        &open_request("upload-a", bytes),
    );
    assert_eq!(kind, RESULT_SUCCESS);

    let mut fixture = RequestFixture::with_bytes(work, source, bytes);
    let (kind, _) = execute(
        provider,
        &mut fixture,
        "staging/append-from-source",
        &append_request("upload-a", 0, bytes, source),
    );
    assert_eq!(kind, RESULT_SUCCESS);
    assert_eq!(fixture.finishes, 1);

    let mut fixture = RequestFixture::empty(work);
    let (kind, _) = execute(
        provider,
        &mut fixture,
        "staging/verify-commit",
        &commit_request("upload-a", bytes),
    );
    assert_eq!(kind, RESULT_SUCCESS);
    unsafe { hoplite_blob_store_provider_close_v1(provider) };

    let provider = open_provider(&root);
    let mut fixture = RequestFixture::empty(work);
    let (kind, result) = execute(
        provider,
        &mut fixture,
        "object/open-source",
        &open_source_request(bytes, 3, 14),
    );
    assert_eq!(kind, RESULT_SUCCESS);
    let handle = source_handle(&result);

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
    loop {
        returned = 0;
        let status = unsafe {
            hoplite_blob_store_provider_response_read_scoped_v1(
                provider,
                &mut fixture as *mut RequestFixture as *mut c_void,
                work,
                handle,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut returned,
            )
        };
        assert_eq!(status, STATUS_OK);
        if returned == 0 {
            break;
        }
        output.extend_from_slice(&buffer[..returned]);
    }
    assert_eq!(output, bytes[3..17]);
    assert_eq!(
        unsafe {
            hoplite_blob_store_provider_response_close_scoped_v1(
                provider,
                &mut fixture as *mut RequestFixture as *mut c_void,
                work,
                handle,
            )
        },
        STATUS_OK
    );
    assert_eq!(
        unsafe {
            hoplite_blob_store_provider_response_close_scoped_v1(
                provider,
                &mut fixture as *mut RequestFixture as *mut c_void,
                work,
                handle,
            )
        },
        STATUS_RESOURCE_ERROR
    );
    unsafe { hoplite_blob_store_provider_close_v1(provider) };
}

#[test]
fn filesystem_open_rejects_untrusted_abi_inputs() {
    let limits = limits();
    let mut provider = ptr::null_mut();
    assert_eq!(
        unsafe {
            hoplite_blob_store_provider_open_filesystem_v1(ptr::null(), 0, &limits, &mut provider)
        },
        STATUS_INVALID
    );
    let invalid_utf8 = [0xff_u8];
    assert_eq!(
        unsafe {
            hoplite_blob_store_provider_open_filesystem_v1(
                invalid_utf8.as_ptr(),
                invalid_utf8.len(),
                &limits,
                &mut provider,
            )
        },
        STATUS_INVALID
    );
    let nul_path = b"bad\0root";
    assert_eq!(
        unsafe {
            hoplite_blob_store_provider_open_filesystem_v1(
                nul_path.as_ptr(),
                nul_path.len(),
                &limits,
                &mut provider,
            )
        },
        STATUS_INVALID
    );
    assert!(provider.is_null());
}
