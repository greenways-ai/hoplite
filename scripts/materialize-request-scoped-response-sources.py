from pathlib import Path

RUST = Path("core/abi/blob-store-provider-ffi/src/lib.rs")
SELF = Path(__file__)

source = RUST.read_text()

old = """/// Legacy work-only response read retained for ABI compatibility.
///
/// This entrypoint always fails closed because it cannot prove the opaque
/// request identity that opened the source. Request-serving hosts must call
/// [`hoplite_blob_store_provider_response_read_scoped_v1`].
#[no_mangle]
"""
new = """/// Legacy work-only response read retained for ABI compatibility.
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
"""
if source.count(old) != 1:
    raise SystemExit("expected one legacy read documentation block")
source = source.replace(old, new)

old = """/// Legacy work-only response close retained for ABI compatibility.
///
/// This entrypoint always fails closed because it cannot prove the opaque
/// request identity that opened the source. Request-serving hosts must call
/// [`hoplite_blob_store_provider_response_close_scoped_v1`].
#[no_mangle]
"""
new = """/// Legacy work-only response close retained for ABI compatibility.
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
"""
if source.count(old) != 1:
    raise SystemExit("expected one legacy close documentation block")
source = source.replace(old, new)

RUST.write_text(source)
SELF.unlink()
