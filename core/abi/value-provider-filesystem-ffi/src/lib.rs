#![allow(clippy::missing_safety_doc)]

//! Stable synchronous C boundary for the installed filesystem `hoplite.value`
//! provider. Trusted startup code owns the root and ceilings. Hara supplies
//! only an operation and one standalone HTA0 argument frame.

use hoplite_value_provider_filesystem::{FilesystemValueProvider, Limits};
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::{ptr, slice, str};

const ABI_VERSION: u32 = 0;
const STATUS_OK: i32 = 0;
const STATUS_INVALID: i32 = 1;
const STATUS_OPEN_ERROR: i32 = 2;
const STATUS_PANIC: i32 = 3;

const RESULT_SUCCESS: u32 = 0;
const RESULT_FAILURE: u32 = 1;
const MAGIC: &[u8; 4] = b"HTA0";
const HTA_STRING: u8 = 4;

#[repr(C)]
pub struct HopliteValueResultV1 {
    pub data: *mut u8,
    pub len: usize,
    pub kind: u32,
}

impl Default for HopliteValueResultV1 {
    fn default() -> Self {
        Self {
            data: ptr::null_mut(),
            len: 0,
            kind: RESULT_SUCCESS,
        }
    }
}

pub struct HopliteValueProvider {
    inner: FilesystemValueProvider,
}

#[no_mangle]
pub extern "C" fn hoplite_value_provider_abi_version() -> u32 {
    ABI_VERSION
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_value_provider_open_filesystem_v1(
    root: *const u8,
    root_len: usize,
    max_frame_bytes: usize,
    max_media_type_bytes: usize,
    io_chunk_bytes: usize,
    provider: *mut *mut HopliteValueProvider,
) -> i32 {
    if provider.is_null() {
        return STATUS_INVALID;
    }
    *provider = ptr::null_mut();
    let Some(root) = required_bytes(root, root_len) else {
        return STATUS_INVALID;
    };
    let Ok(root) = str::from_utf8(root) else {
        return STATUS_INVALID;
    };
    if root.is_empty() || root.as_bytes().contains(&0) {
        return STATUS_INVALID;
    }
    let limits = Limits {
        max_frame_bytes,
        max_media_type_bytes,
        io_chunk_bytes,
    };
    if limits.validate().is_err() {
        return STATUS_INVALID;
    }

    match catch_unwind(AssertUnwindSafe(|| {
        FilesystemValueProvider::open(root, limits)
            .map(|inner| Box::new(HopliteValueProvider { inner }))
            .map_err(|_| STATUS_OPEN_ERROR)
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
pub unsafe extern "C" fn hoplite_value_provider_execute_v1(
    provider: *mut HopliteValueProvider,
    operation: *const u8,
    operation_len: usize,
    arguments_hta: *const u8,
    arguments_hta_len: usize,
    result: *mut HopliteValueResultV1,
) -> i32 {
    if result.is_null() {
        return STATUS_INVALID;
    }
    *result = HopliteValueResultV1::default();
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
pub unsafe extern "C" fn hoplite_value_provider_result_free_v1(result: *mut HopliteValueResultV1) {
    if result.is_null() {
        return;
    }
    let result = &mut *result;
    if !result.data.is_null() && result.len != 0 {
        let allocation = ptr::slice_from_raw_parts_mut(result.data, result.len);
        drop(Box::from_raw(allocation));
    }
    *result = HopliteValueResultV1::default();
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_value_provider_close_v1(provider: *mut HopliteValueProvider) {
    if !provider.is_null() {
        drop(Box::from_raw(provider));
    }
}

fn write_result(result: &mut HopliteValueResultV1, kind: u32, frame: Vec<u8>) {
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
pub extern "C" fn hoplite_value_provider_ffi_type_anchor(_: *mut c_void) {}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest as Sha2Digest, Sha256};
    use std::fs::{self, File};
    use std::path::PathBuf;
    use std::process;
    use std::time::{SystemTime, UNIX_EPOCH};

    const NIL: u8 = 0;
    const I64: u8 = 3;
    const STRING: u8 = 4;
    const KEYWORD: u8 = 6;
    const VECTOR: u8 = 9;
    const MAP: u8 = 11;

    struct Fixture {
        root: PathBuf,
        digest: String,
        frame: Vec<u8>,
    }

    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "hoplite-value-provider-ffi-{}-{nonce}",
                process::id()
            ));
            fs::create_dir_all(root.join("objects/sha256")).unwrap();
            File::create(root.join("store.lock")).unwrap();
            let frame = vec![b'H', b'T', b'A', b'1', NIL];
            let digest_bytes = Sha256::digest(&frame);
            let hex = digest_bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let digest = format!("sha256:{hex}");
            let directory = root.join("objects/sha256").join(&hex[..2]);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join(format!("{}.blob", &hex[2..])), &frame).unwrap();
            let media_type = b"application/vnd.hara.hta";
            let mut metadata = b"HBO0".to_vec();
            metadata.extend_from_slice(&digest_bytes);
            metadata.extend_from_slice(&(frame.len() as u64).to_be_bytes());
            metadata.extend_from_slice(&(media_type.len() as u32).to_be_bytes());
            metadata.extend_from_slice(media_type);
            fs::write(directory.join(format!("{}.meta", &hex[2..])), metadata).unwrap();
            Self {
                root,
                digest,
                frame,
            }
        }

        fn request(&self) -> Vec<u8> {
            let request = bare_map(vec![
                ("digest", bare_string(&self.digest)),
                ("max-bytes", bare_i64(self.frame.len() as i64)),
                ("operation", bare_string("object/verify-hta")),
                ("protocol", bare_string("hoplite.value-request/0-alpha")),
            ]);
            let mut frame = MAGIC.to_vec();
            frame.extend_from_slice(&bare_vector(&[request]));
            frame
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
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

    #[test]
    fn opens_executes_and_closes_the_installed_provider() {
        let fixture = Fixture::new();
        let root = fixture.root.to_string_lossy();
        let mut provider = ptr::null_mut();
        assert_eq!(
            unsafe {
                hoplite_value_provider_open_filesystem_v1(
                    root.as_ptr(),
                    root.len(),
                    1024,
                    128,
                    64,
                    &mut provider,
                )
            },
            STATUS_OK
        );
        assert!(!provider.is_null());

        let request = fixture.request();
        let operation = b"object/verify-hta";
        let mut result = HopliteValueResultV1::default();
        assert_eq!(
            unsafe {
                hoplite_value_provider_execute_v1(
                    provider,
                    operation.as_ptr(),
                    operation.len(),
                    request.as_ptr(),
                    request.len(),
                    &mut result,
                )
            },
            STATUS_OK
        );
        assert_eq!(result.kind, RESULT_SUCCESS);
        let bytes = unsafe { slice::from_raw_parts(result.data, result.len) };
        assert!(bytes.starts_with(MAGIC));
        unsafe {
            hoplite_value_provider_result_free_v1(&mut result);
            hoplite_value_provider_close_v1(provider);
        }
        assert!(result.data.is_null());
    }

    #[test]
    fn rejects_untrusted_or_invalid_open_inputs() {
        let mut provider = ptr::null_mut();
        assert_eq!(
            unsafe {
                hoplite_value_provider_open_filesystem_v1(ptr::null(), 0, 1, 1, 1, &mut provider)
            },
            STATUS_INVALID
        );
        let missing = b"/definitely/not/a/hoplite/value/root";
        assert_eq!(
            unsafe {
                hoplite_value_provider_open_filesystem_v1(
                    missing.as_ptr(),
                    missing.len(),
                    1024,
                    128,
                    64,
                    &mut provider,
                )
            },
            STATUS_OPEN_ERROR
        );
        unsafe {
            hoplite_value_provider_result_free_v1(ptr::null_mut());
            hoplite_value_provider_close_v1(ptr::null_mut());
        }
    }
}
