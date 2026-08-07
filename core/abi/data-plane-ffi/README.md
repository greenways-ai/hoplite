# Hoplite native data-plane callback bridge

This crate binds Hoplite's application-neutral streaming contracts to native
request and response sources without turning large bodies into Hara values.

```text
Nginx or another native host
  owns request buffers, response sources and lifecycle
        │ C callback descriptors
        ▼
hoplite-data-plane-ffi
  validates descriptors, bounds each read and closes once
        │ RequestBody / ResponseBody
        ▼
hoplite-data-plane-abi
  declared-length accounting, total limits, range planning and EOF laws
```

## Rust ownership boundary

The C-compatible descriptor structs are inert data. Safe Rust cannot activate
them. `FfiRequestBody::from_raw` and `FfiResponseBody::from_raw` are `unsafe`
because the caller transfers exclusive ownership of a native context and its
callbacks into the wrapper.

The transfer applies whether construction succeeds or fails. A valid non-null
context with a close callback is closed exactly once on validation failure,
explicit close, or drop. The descriptor must not be copied into another owning
wrapper or used again after the call.

The caller must guarantee that:

- the context remains valid until close;
- callbacks obey their signatures and do not unwind;
- the read callback may write only within the supplied output buffer;
- callback state is exclusively owned or externally synchronized; and
- close invalidates the context and is safe to call exactly once.

## Request descriptor

`hoplite_request_body_v1` contains an opaque native context, optional
authoritative declared length, sequential bounded read callback, and optional
close callback. The bridge rejects a null context, missing read callback,
invalid length flag, callback failure, over-read, body-limit overflow, and
declared-length mismatch.

The caller chooses `BodyLimits`; callbacks cannot enlarge them. Each callback
receives no more than `max_chunk_bytes` and no more than the remaining body
limit.

## Response descriptor

`hoplite_response_body_v1` contains an opaque native context, immutable total
length, seekable `read_at` callback, and optional close callback. The descriptor
implements `ResponseBody`, so the existing `ResponsePlan` and `StreamResponse`
contracts provide complete responses and one exact, open-ended, or suffix byte
range. Callback failure, over-read, and early EOF fail closed.

## Authority boundary

A native context or later resource handle is transport state, not application
authorization. Neither may encode a path, URL, upstream, credential, management
principal, or portable application value. Authentication and application grants
are evaluated separately before a handler receives an operation-specific
handle.

## Callback law

A read callback returns `HOPLITE_CALLBACK_OK` on success and writes the number
of bytes produced through `returned`:

```text
0 <= returned <= capacity
```

Any non-zero status becomes a body I/O error. A callback must not retain the
borrowed output pointer after returning.
