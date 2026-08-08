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
request context + work ID + source handle
```

before a native callback can run. The generic adapter never receives the Hoplite runtime pointer.

## Response sources

`object/open-source` asks the installed blob driver for one immutable bounded source. A trusted `ResponseSourceRegistrar` owns that source and returns a positive opaque handle. The HAL-visible result contains only the handle and the request's validated digest, offset, and length.

A handle remains scoped to its exact work. Cross-work reads and closes fail even when the numeric handle is known. Completing or cancelling a work closes every response source retained by that work; worker shutdown closes any remaining sources.

Nginx backpressure, cancellation, `HEAD`, and range response integration remain the separate response-source transport layer.

## Native Hoplite provider

`hoplite-blob-store-provider-ffi` owns one application-neutral in-memory store per Nginx worker. The worker registers its immutable descriptor under the exact service name `hara.blob` before bootstrap evaluation begins.

The C boundary:

- receives only the copied operation and one standalone canonical `HTA1` argument frame;
- resolves request sources through the existing work-scoped request-body callbacks;
- registers immutable response sources in a provider-owned work-scoped registry;
- returns either a canonical `hara.blob-result/1` frame or one closed stable error-code string;
- declares request-body and response-body transport capability, but no path, bucket, credential, metadata, network, process, or driver-selection authority; and
- releases result frames immediately after the Hoplite completer accepts or rejects them.

The in-memory provider is deliberately the first native implementation. Durable object drivers can implement the same generic `BlobStore` boundary later without changing Tahto's HAL requests or widening the host call.

## Domain boundary

The adapter contains no Tahto upload, namespace, quota, graph, manifest, authorization, receipt, or recovery semantics. Tahto remains responsible for creating the closed request and validating the exact generic result in HAL.
