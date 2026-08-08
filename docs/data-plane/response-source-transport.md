# Source-backed HTTP responses

Hoplite can stream an immutable native source without copying the complete object into a Hara value or an Nginx request-pool allocation.

This transport is intentionally narrow. A handler first obtains a source through `hara.blob/object/open-source`, then returns one exact response body plan:

```clojure
{:status 200
 :headers {"content-type" "application/octet-stream"}
 :body {:protocol "hoplite.response-source/1"
        :source 4271
        :offset 0
        :length 1048576}}
```

The body map must contain exactly four fields:

```text
:protocol  exact string "hoplite.response-source/1"
:source    positive opaque source handle
:offset    non-negative validated source-range metadata
:length    non-negative number of bytes to emit
```

Unknown or duplicate fields, a different protocol, invalid integers, overflow, or a source response status other than `200` or `206` fail before streaming begins.

## Authority

The numeric `:source` value is not authority. Nginx calls the native blob provider with:

```text
opaque request context + work ID + source handle
```

The provider verifies all three against the registration created by the earlier `object/open-source` call. A source cannot be moved to another request, another live work, or a later response by copying its number.

HAL never receives or supplies the request context.

## Lifecycle

For a normal `GET` response, Nginx performs one bounded source read before sending headers. This ensures a stale, closed, wrong-request, or wrong-work handle becomes a `500` response rather than a misleading successful response whose body later fails.

After headers are sent, Nginx:

1. reads at most `hoplite_response_body_chunk` bytes;
2. submits that buffer to the output filter;
3. waits when Nginx or the connection retains buffered output;
4. resumes from the write event only after backpressure clears;
5. stops after exactly `:length` bytes; and
6. finalizes only after the final busy chain has drained.

An early EOF, oversized source read, output failure, timeout, disconnect, request cancellation, or work cleanup closes the source and terminates the response. Close is idempotent at the Nginx layer and exact-once at the provider registry.

## `HEAD`

A `HEAD` handler may return the same source plan as `GET`. Hoplite sends the response status, headers, and declared `Content-Length` without reading source bytes, then closes the source immediately.

This lets application code share object metadata and authorization logic between `GET` and `HEAD` without materializing a body.

## Ranges

Range policy remains in HAL. A handler validates the requested range, calls `object/open-source` with the chosen offset and length, and returns status `206` with an explicit `Content-Range` header.

The response plan's `:offset` is closed metadata describing the already-opened source. Nginx does not seek an arbitrary handle or reinterpret a client `Range` header. The installed blob driver has already validated and positioned the immutable source.

## Ordinary response fast path

Strings, byte values, `nil`, and existing response maps retain the ordinary direct response path. Merely returning a map as a body does not enable native streaming: the exact `:protocol` field is required.

This preserves current handler behaviour while keeping source transport explicit and auditable.

## Configuration

`hoplite_response_body_chunk` is an Nginx location directive with a positive size value. The default is `64k`.

The chunk size bounds each native read and request-pool buffer. It does not change the object length, permit arbitrary paths, or grant source authority.

Durable multi-worker deployments should configure `HOPLITE_HARA_BLOB_ROOT`. The compatibility in-memory driver is worker-local and therefore unsuitable for objects uploaded by one worker and downloaded through another.
