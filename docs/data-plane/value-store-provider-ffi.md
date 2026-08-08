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

## Nginx registration

The Nginx module now constructs this provider once during worker initialization and registers its immutable descriptor under the exact service name `hara.store`. The bridge is synchronous, declares no request- or response-body capability, forwards the copied operation and standalone argument frame unchanged, and releases every result immediately after the existing Hoplite completer accepts or rejects it.

Trusted process configuration is read only during worker initialization:

```text
HOPLITE_HARA_STORE_PATH
HOPLITE_HARA_STORE_MAX_VALUE_BYTES
HOPLITE_HARA_STORE_MAX_RECEIPT_BYTES
```

The path enables the provider. If it is absent, `hara.store` remains deliberately unregistered. The two positive decimal limits are optional and default to 8 MiB and 1 MiB. A malformed limit, ABI mismatch, database-open failure, or duplicate registration prevents the worker from starting. Hara values cannot select the driver, path, limits, provider ABI, credentials, or service identity.

The production container binds the provider to `/var/lib/hoplite/hara-store.sqlite`, inside the existing persistent Hoplite volume. Other deployments must supply an equally trusted path or leave the provider disabled.
