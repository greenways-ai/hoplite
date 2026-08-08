# Portable `hara.store` provider boundary

`hoplite-value-store-provider-ffi` wraps the tested canonical `hara.store` adapter and SQLite driver behind a small synchronous C ABI.

Trusted worker startup selects:

- the SQLite database path;
- the maximum canonical value frame size; and
- the maximum opaque receipt frame size.

These values are not accepted from Hara calls.

## Execution

The caller supplies one UTF-8 operation name and one standalone canonical `HTA1` argument frame. Normal protocol execution always returns an owned frame:

- `HOPLITE_VALUE_STORE_RESULT_SUCCESS` contains a canonical `hara.store-result/1` frame or nil;
- `HOPLITE_VALUE_STORE_RESULT_FAILURE` contains one HTA string with only the stable application-neutral error code.

Storage paths, SQL text, credentials and opaque application values are not copied into failure frames.

ABI pointer, length, UTF-8 and limit failures return a non-zero status without allocating a result. Panics are contained at the FFI boundary.

## Ownership

One opaque provider context belongs to its worker and must be closed exactly once. Result frames are released with the matching result-free function; freeing a null or already-zero result is safe.

This ABI does not register the service with Nginx. A subsequent provider-registration PR binds it under the exact trusted service name `hara.store` and connects its success/failure frames to the existing Hoplite completer.
