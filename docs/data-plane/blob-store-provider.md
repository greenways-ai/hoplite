# Canonical `hara.blob` protocol adapter

`hoplite-blob-store-provider` is the application-neutral boundary between one Hara host call and a generic `BlobStore` driver.

It accepts one canonical argument vector containing one closed `hara.blob-request/1` map and supports:

```text
staging/open
staging/append-from-source
staging/abort
staging/verify-commit
object/open-source
```

Each operation has an exact field set matching the pure-HAL Tahto capability client. The adapter rejects extra fields, protocol or operation mismatch, invalid identifiers, non-canonical digests, negative sizes and offsets, zero lengths, and zero source handles.

## Request-source authority

The adapter does not treat `:source-handle` as authority. It asks a `RequestSourceResolver` that is already bound by the host to the exact owning request and work. The production Hoplite resolver uses:

```text
opaque request context + work ID + source handle
```

before a native callback can run. The generic adapter never receives the Hoplite runtime pointer.

## Response-source authority

`object/open-source` asks the installed blob driver for one immutable bounded source. A trusted `ResponseSourceRegistrar` owns that source and returns a positive opaque handle. The HAL-visible result contains only the handle and the request's validated digest, offset, and length.

The provider records:

```text
opaque request context + work ID + response-source handle
```

A read or close must match all three values before memory or filesystem bytes are touched. A copied numeric handle, even with the correct work ID, grants no authority. The older work-only C symbols remain exported for ABI compatibility but fail closed because they cannot prove the request identity.

Completing or cancelling a work closes every source still retained by that work. Worker shutdown closes any remaining sources.

## Installed drivers

Trusted worker startup chooses one closed installed-store variant:

```text
memory       deterministic development and compatibility mode
filesystem   restart-safe shared byte custody under one trusted root
```

The filesystem root and fixed limits are accepted only at worker startup. HAL requests cannot select a root, path, driver, provider, native library, credential, or command.

The same canonical request adapter, source resolver, result frames, response registry, and ownership checks apply to both drivers.

## Native Hoplite provider

`hoplite-blob-store-provider-ffi` owns one installed store per Nginx worker. The worker registers its immutable descriptor under the exact service name `hara.blob` before bootstrap evaluation begins.

The C boundary:

- receives only the copied operation and one standalone canonical `HTA1` argument frame;
- resolves request sources through exact request-and-work body callbacks;
- registers immutable response sources under exact request-and-work ownership;
- returns either a canonical `hara.blob-result/1` frame or one closed stable error-code string;
- declares request-body and response-body transport capability, but no application, path, bucket, credential, metadata, network, process, or driver-selection authority; and
- releases result frames immediately after the Hoplite completer accepts or rejects them.

## HTTP response transport

A Hara handler can return a closed `hoplite.response-source/1` body plan using the handle returned by `object/open-source`. Nginx validates that plan, reads bounded chunks through the exact request/work/handle boundary, honours output backpressure, and closes the source on completion, `HEAD`, cancellation, timeout, disconnect, or error.

Ordinary string and byte response bodies retain their existing direct fast path. See [`response-source-transport.md`](response-source-transport.md) for the exact plan and lifecycle.

## Domain boundary

The adapter contains no Tahto upload, namespace, quota, graph, manifest, authorization, receipt, or recovery semantics. Tahto remains responsible for creating closed requests and validating generic results in HAL.
