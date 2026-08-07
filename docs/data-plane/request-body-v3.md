# Native request bodies through Hoplite ABI V3

Hoplite request ABI V3 lets an application opt into bounded native request bodies without converting their bytes into ordinary Hara values.

## Application declaration

An application may declare one closed body profile:

```clojure
(h/app
  {:name "archive-service"
   :request/body
   {:max-bytes 8388608
    :max-chunk-bytes 65536}
   :resources [...]})
```

Both limits are positive byte counts and `:max-chunk-bytes` may not exceed `:max-bytes`. If the chunk limit is omitted it defaults to the smaller of 64 KiB and the total limit.

The profile is application-wide in this first transport slice. An application that contains a `:request+hta` route cannot enable native bodies because a process-local handle is not a portable HTA value.

## Generated Nginx boundary

The declaration emits server-owned location configuration:

```nginx
client_max_body_size 8388608;
hoplite_request_body on;
hoplite_request_body_max 8388608;
hoplite_request_body_chunk 65536;
```

The module requires an authoritative `Content-Length` for non-empty bodies. Unknown-length or chunked requests fail with `411 Length Required`; declared bodies above the configured limit fail with `413 Request Entity Too Large` before Hara execution.

Nginx may buffer the request in memory or in its own temporary file. The native callback walks those Nginx-owned buffers sequentially and supports both representations. It does not expose a path, file descriptor or callback pointer to Hara.

## Hara projection

A direct `request` or `raw` handler receives only:

```clojure
{:body-handle 17}
```

The handle is positive, process-unique, worker-resolved and bound to the work scope that accepted the request. Copying its integer to another worker or work ID grants no access.

Existing V2 callers and body-free requests retain their previous path. Request ABI V3 is used only when an opted-in application receives a non-empty declared body.

## Lifecycle

```text
Nginx buffers declared request bytes
  -> constructs hoplite_request_body_v1
  -> Hoplite validates server-selected limits
  -> worker registry mints an opaque handle
  -> Hara runs with :body-handle only
  -> synchronous completion, failure, work close or runtime teardown closes it
```

The close callback invalidates the request-scoped context. Nginx continues to own and clean its pool and temporary files.

## Current limitation

This profile is bounded buffering, not streaming backpressure from the client socket. It establishes the production Nginx-to-worker descriptor boundary required by Tahto uploads. A later slice may add no-buffering request flow while preserving the same V3 descriptor and work-scoped handle laws.
