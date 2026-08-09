# Nginx `hoplite.response-source/1` transport

Hoplite recognizes one closed portable response body:

```clojure
{:protocol "hoplite.response-source/1"
 :service "hoplite.blob"
 :source-handle 17
 :offset 4
 :length 9}
```

The value carries no provider, path, credentials, work identifier, HTTP
configuration, or Nginx state. The Nginx host joins those four scalar fields to
the exact opaque request identity and work that created the source handle.

## Streaming law

- The descriptor must be an exact four-key keyword map and pass the Hara safe
  integer and bounded half-open interval rules.
- A source read is authorized by `request identity + work + handle`.
- Nginx allocates one 64 KiB request-pool buffer and never asks the provider for
  more than the descriptor's remaining length.
- The same buffer is not refilled until the previous output chain has been
  consumed and all downstream Nginx buffering has drained.
- A zero read before the declared length, a provider over-read, a stale handle,
  or a close failure aborts the response.
- The source is closed at most once after the final read, on `HEAD`, on output
  failure, on client disconnect, or during request cleanup.
- `HEAD` validates and closes the source without invoking its read callback.
- Application `Content-Length` is ignored; the descriptor length is
  authoritative.
- Ordinary string and byte responses retain the existing small-body fast path.

The portable offset is evidence for the selected immutable range. The
`hoplite.blob` provider has already opened that range, so the Nginx pump reads the
registered source sequentially and does not seek or skip the offset again.
