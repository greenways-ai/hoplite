#![allow(clippy::missing_safety_doc)]

//! Portable synchronous C boundary for the generic `hoplite.store` provider.
//!
//! Trusted startup code selects the SQLite path and fixed span limits. Hara
//! supplies only an operation and one canonical standalone HTA1 argument frame.
//! Protocol failures are returned as a closed HTA string containing the stable
//! application-neutral error code; paths, SQL text and opaque values are never
//! included in failure frames.

use hoplite_value_store::StoreLimits;
use hoplite_value_store_provider::Provider;
use hoplite_value_store_sqlite::{Sha256Verifier, SqliteValueStore};
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::{ptr, slice, str};

const ABI_VERSION: u32 = 1;
const STATUS_OK: i32 = 0;
const STATUS_INVALID: i32 = 1;
const STATUS_OPEN_ERROR: i32 = 2;
const STATUS_PANIC: i32 = 3;

const RESULT_SUCCESS: u32 = 0;
const RESULT_FAILURE: u32 = 1;

const MAGIC: &[u8; 4] = b"HTA1";
const HTA_STRING: u8 = 4;

#[repr(C)]
pub struct HopliteValueStoreResultV1 {
    pub data: *mut u8,
    pub len: usize,
    pub kind: u32,
}

impl Default for HopliteValueStoreResultV1 {
    fn default() -> Self {
        Self {
            data: ptr::null_mut(),
            len: 0,
            kind: RESULT_SUCCESS,
        }
    }
}

pub struct HopliteValueStoreProvider {
    inner: Provider<SqliteValueStore, Sha256Verifier>,
}

#[no_mangle]
pub extern "C" fn hoplite_value_store_provider_abi_version() -> u32 {
    ABI_VERSION
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_value_store_provider_open_sqlite_v1(
    path: *const u8,
    path_len: usize,
    max_value_bytes: usize,
    max_receipt_bytes: usize,
    provider: *mut *mut HopliteValueStoreProvider,
) -> i32 {
    if provider.is_null() {
        return STATUS_INVALID;
    }
    *provider = ptr::null_mut();
    let Some(path) = required_bytes(path, path_len) else {
        return STATUS_INVALID;
    };
    let Ok(path) = str::from_utf8(path) else {
        return STATUS_INVALID;
    };
    if path.is_empty() || path.as_bytes().contains(&0) {
        return STATUS_INVALID;
    }
    let limits = StoreLimits::new(max_value_bytes, max_receipt_bytes);
    if limits.validate().is_err() {
        return STATUS_INVALID;
    }

    match catch_unwind(AssertUnwindSafe(|| {
        let store = SqliteValueStore::open(PathBuf::from(path), limits)
            .map_err(|_| STATUS_OPEN_ERROR)?;
        let inner = Provider::new(store, Sha256Verifier, limits).map_err(|_| STATUS_OPEN_ERROR)?;
        Ok::<_, i32>(Box::new(HopliteValueStoreProvider { inner }))
    })) {
        Ok(Ok(context)) => {
            *provider = Box::into_raw(context);
            STATUS_OK
        }
        Ok(Err(status)) => status,
        Err(_) => STATUS_PANIC,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_value_store_provider_execute_v1(
    provider: *mut HopliteValueStoreProvider,
    operation: *const u8,
    operation_len: usize,
    arguments_hta: *const u8,
    arguments_hta_len: usize,
    result: *mut HopliteValueStoreResultV1,
) -> i32 {
    if result.is_null() {
        return STATUS_INVALID;
    }
    *result = HopliteValueStoreResultV1::default();
    if provider.is_null() {
        return STATUS_INVALID;
    }
    let Some(operation) = required_bytes(operation, operation_len) else {
        return STATUS_INVALID;
    };
    let Ok(operation) = str::from_utf8(operation) else {
        return STATUS_INVALID;
    };
    let Some(arguments_hta) = required_bytes(arguments_hta, arguments_hta_len) else {
        return STATUS_INVALID;
    };

    let execution = catch_unwind(AssertUnwindSafe(|| {
        let provider = &*provider;
        match provider.inner.execute(operation, arguments_hta) {
            Ok(frame) => (RESULT_SUCCESS, frame),
            Err(error) => (RESULT_FAILURE, failure_frame(error.code())),
        }
    }));
    let Ok((kind, frame)) = execution else {
        return STATUS_PANIC;
    };
    write_result(&mut *result, kind, frame);
    STATUS_OK
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_value_store_provider_result_free_v1(
    result: *mut HopliteValueStoreResultV1,
) {
    if result.is_null() {
        return;
    }
    let result = &mut *result;
    if !result.data.is_null() && result.len != 0 {
        let allocation = ptr::slice_from_raw_parts_mut(result.data, result.len);
        drop(Box::from_raw(allocation));
    }
    *result = HopliteValueStoreResultV1::default();
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_value_store_provider_close_v1(
    provider: *mut HopliteValueStoreProvider,
) {
    if !provider.is_null() {
        drop(Box::from_raw(provider));
    }
}

fn write_result(result: &mut HopliteValueStoreResultV1, kind: u32, frame: Vec<u8>) {
    let allocation = frame.into_boxed_slice();
    result.len = allocation.len();
    result.data = Box::into_raw(allocation) as *mut u8;
    result.kind = kind;
}

fn failure_frame(code: &str) -> Vec<u8> {
    let mut output = Vec::with_capacity(MAGIC.len() + 1 + 4 + code.len());
    output.extend_from_slice(MAGIC);
    output.push(HTA_STRING);
    output.extend_from_slice(&(code.len() as u32).to_be_bytes());
    output.extend_from_slice(code.as_bytes());
    output
}

unsafe fn required_bytes<'a>(data: *const u8, len: usize) -> Option<&'a [u8]> {
    if data.is_null() || len == 0 {
        None
    } else {
        Some(slice::from_raw_parts(data, len))
    }
}

#[no_mangle]
pub extern "C" fn hoplite_value_store_provider_ffi_type_anchor(_: *mut c_void) {}

#[cfg(test)]
mod tests {
    use super::*;
    use hoplite_provider_hta::{Document, Kind};
    use hoplite_value_store::{Digest, DigestVerifier, REQUEST_PROTOCOL};
    use std::fs;
    use std::process;
    use std::time::{SystemTime, UNIX_EPOCH};

    const NIL: u8 = 0;
    const I64: u8 = 3;
    const STRING: u8 = 4;
    const KEYWORD: u8 = 6;
    const VECTOR: u8 = 9;
    const MAP: u8 = 11;

    struct TempDatabase {
        root: PathBuf,
        path: PathBuf,
    }

    impl TempDatabase {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "hoplite-value-store-provider-ffi-{}-{nonce}",
                process::id()
            ));
            fs::create_dir_all(&root).expect("create test directory");
            let path = root.join("state.sqlite3");
            Self { root, path }
        }
    }

    impl Drop for TempDatabase {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn open_provider(database: &TempDatabase) -> *mut HopliteValueStoreProvider {
        let path = database.path.to_string_lossy();
        let mut provider = ptr::null_mut();
        let status = unsafe {
            hoplite_value_store_provider_open_sqlite_v1(
                path.as_ptr(),
                path.len(),
                1024 * 1024,
                64 * 1024,
                &mut provider,
            )
        };
        assert_eq!(status, STATUS_OK);
        assert!(!provider.is_null());
        provider
    }

    fn execute(
        provider: *mut HopliteValueStoreProvider,
        operation: &str,
        arguments: &[u8],
    ) -> (u32, Vec<u8>) {
        let mut result = HopliteValueStoreResultV1::default();
        let status = unsafe {
            hoplite_value_store_provider_execute_v1(
                provider,
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
        unsafe { hoplite_value_store_provider_result_free_v1(&mut result) };
        assert!(result.data.is_null());
        assert_eq!(result.len, 0);
        (kind, bytes)
    }

    fn frame(bare: &[u8]) -> Vec<u8> {
        let mut output = MAGIC.to_vec();
        output.extend_from_slice(bare);
        output
    }

    fn bare_text(tag: u8, value: &str) -> Vec<u8> {
        let mut output = vec![tag];
        output.extend_from_slice(&(value.len() as u32).to_be_bytes());
        output.extend_from_slice(value.as_bytes());
        output
    }

    fn bare_string(value: &str) -> Vec<u8> {
        bare_text(STRING, value)
    }

    fn bare_keyword(value: &str) -> Vec<u8> {
        bare_text(KEYWORD, value)
    }

    fn bare_i64(value: i64) -> Vec<u8> {
        let mut output = vec![I64];
        output.extend_from_slice(&value.to_be_bytes());
        output
    }

    fn bare_vector(values: &[Vec<u8>]) -> Vec<u8> {
        let mut output = vec![VECTOR];
        output.extend_from_slice(&(values.len() as u32).to_be_bytes());
        for value in values {
            output.extend_from_slice(value);
        }
        output
    }

    fn bare_map(entries: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
        let mut entries = entries
            .into_iter()
            .map(|(key, value)| (bare_keyword(key), value))
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let mut output = vec![MAP];
        output.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (key, value) in entries {
            output.extend_from_slice(&key);
            output.extend_from_slice(&value);
        }
        output
    }

    fn arguments(request: Vec<u8>) -> Vec<u8> {
        frame(&bare_vector(&[request]))
    }

    fn load_request() -> Vec<u8> {
        arguments(bare_map(vec![
            ("operation", bare_string("load")),
            ("protocol", bare_string(REQUEST_PROTOCOL)),
        ]))
    }

    fn value() -> Vec<u8> {
        frame(&bare_map(vec![
            ("kind", bare_keyword("fixture")),
            ("message", bare_string("hello")),
        ]))
    }

    fn initialize_request(value: &[u8]) -> Vec<u8> {
        let digest = Digest::from_bytes(Sha256Verifier.sha256(value)).to_string();
        arguments(bare_map(vec![
            ("operation", bare_string("initialize")),
            ("protocol", bare_string(REQUEST_PROTOCOL)),
            ("revision", bare_i64(0)),
            ("value", value[MAGIC.len()..].to_vec()),
            ("value-digest", bare_string(&digest)),
        ]))
    }

    #[test]
    fn opens_executes_and_closes_a_durable_provider() {
        let database = TempDatabase::new();
        let provider = open_provider(&database);

        let (kind, absent) = execute(provider, "load", &load_request());
        assert_eq!(kind, RESULT_SUCCESS);
        assert_eq!(absent, frame(&[NIL]));

        let opaque = value();
        let (kind, initialized) = execute(provider, "initialize", &initialize_request(&opaque));
        assert_eq!(kind, RESULT_SUCCESS);
        let document = Document::parse(&initialized).unwrap();
        assert_eq!(
            document
                .root()
                .map_get("value")
                .unwrap()
                .unwrap()
                .standalone_frame(),
            opaque
        );

        let (_, loaded) = execute(provider, "load", &load_request());
        let document = Document::parse(&loaded).unwrap();
        assert_eq!(
            document
                .root()
                .map_get("value")
                .unwrap()
                .unwrap()
                .standalone_frame(),
            opaque
        );

        unsafe { hoplite_value_store_provider_close_v1(provider) };
        unsafe { hoplite_value_store_provider_close_v1(ptr::null_mut()) };
    }

    #[test]
    fn returns_only_a_stable_failure_code_for_protocol_errors() {
        let database = TempDatabase::new();
        let provider = open_provider(&database);
        let malformed = frame(&[NIL]);
        let (kind, failure) = execute(provider, "load", &malformed);
        assert_eq!(kind, RESULT_FAILURE);
        let document = Document::parse(&failure).unwrap();
        assert_eq!(document.root().kind(), Kind::String);
        assert_eq!(document.root().as_text().unwrap(), "store-request-invalid");
        assert!(!document.root().as_text().unwrap().contains("sqlite"));
        unsafe { hoplite_value_store_provider_close_v1(provider) };
    }

    #[test]
    fn rejects_invalid_abi_inputs_and_accepts_null_free() {
        let mut provider = ptr::null_mut();
        assert_eq!(
            unsafe {
                hoplite_value_store_provider_open_sqlite_v1(
                    ptr::null(),
                    0,
                    1,
                    1,
                    &mut provider,
                )
            },
            STATUS_INVALID
        );
        let invalid_utf8 = [0xff_u8];
        assert_eq!(
            unsafe {
                hoplite_value_store_provider_open_sqlite_v1(
                    invalid_utf8.as_ptr(),
                    invalid_utf8.len(),
                    1,
                    1,
                    &mut provider,
                )
            },
            STATUS_INVALID
        );
        assert_eq!(
            unsafe {
                hoplite_value_store_provider_open_sqlite_v1(
                    b"x".as_ptr(),
                    1,
                    0,
                    1,
                    &mut provider,
                )
            },
            STATUS_INVALID
        );
        unsafe {
            hoplite_value_store_provider_result_free_v1(ptr::null_mut());
            hoplite_value_store_provider_close_v1(ptr::null_mut());
        }
    }
}
