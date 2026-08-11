# Registered native host providers

Hoplite dispatches Hara host calls through fixed-capacity registries owned by one
Nginx worker. Registration happens during trusted worker initialization. Lookup
is an exact, case-sensitive service-name match and allocates no memory on the
steady-state request path.

The built-in `nginx` provider remains an internal implementation. External
native modules use the portable versioned ABI in:

```text
core/nginx/hoplite_host_provider.h
```

That header contains standard C types only. It does not expose Nginx request,
pool, event or logging structures.

## Registration boundary

An external provider installs one immutable descriptor through:

```c
hoplite_host_provider_register_v1(&provider)
```

The descriptor declares:

```text
ABI version
exact service name
invoke callback
optional cancellation callback
capability flags
```

The descriptor, callback pointers and service bytes must outlive the worker.
Registration rejects null callbacks, unsupported ABI versions, duplicate service
names and registry overflow. Application values cannot call the registration
function or select native pointers and libraries.

`hoplite_host_provider_find_v1` exists for native dispatch and conformance. It
does not make the registry mutable from Hara.

## Call boundary

A portable provider receives a `hoplite_host_call_v1_t` containing:

```text
opaque owning-request context
work ID
call ID
operation name
exact standalone HTA0 argument frame
success and failure completers
```

The operation and argument bytes are copied into the owning request pool before
invocation and remain valid until the call completes or the request is
cancelled. The call structure itself is borrowed only for the duration of the
invoke callback; a pending provider retains the individual values it needs, not
the structure pointer.

The work ID is part of the authority boundary. A provider that later resolves a
request-body or response-source handle must use the exact `(work ID, handle)`
pair. A numeric handle alone is transport state and grants no authority.

Arguments remain opaque canonical HTA bytes at the portable boundary. Provider
implementations may decode them using their own reviewed ABI, but Hoplite does
not reinterpret provider-specific records or expose an Nginx-owned decoded
object pointer.

## Completion

A portable provider returns one of:

```text
HOPLITE_HOST_PROVIDER_OK
  The provider invoked exactly one completer synchronously.

HOPLITE_HOST_PROVIDER_PENDING
  The provider retained one request-scoped operation and will invoke exactly
  one completer later.

HOPLITE_HOST_PROVIDER_ERROR
  The native boundary could not continue and no completion is pending.
```

Completers accept an exact standalone HTA0 result or error frame. An empty
successful result retains the existing Hoplite `nil` completion behavior.
Completers feed the result into the owning Hara call and reject duplicate or
post-cleanup delivery.

Pending providers must schedule completion on the owning Nginx worker event
loop. The ABI is request-scoped and does not make the Hara runtime or Nginx
request structures thread-safe. Completion from an arbitrary foreign thread is
outside version 1.

When an operation is retained, Hoplite records the owning provider on the
request. Request completion or cleanup invokes that provider's cancellation
callback before the Hara work scope closes. A provider must resolve, reject or
cancel an operation exactly once.

The first profile permits one retained native operation per request. A second
call while an operation is retained is rejected without terminating the worker.

## Built-in Nginx provider

The existing timer operation remains registered internally as:

```text
service: nginx
operation: sleep
arguments: [milliseconds]
range: 0..3,600,000
```

Zero milliseconds resolves immediately. A positive delay installs an Nginx
timer owned by the request pool. Timer completion clears provider ownership
before delivering the result to Hara. Request cleanup removes a pending timer.

Unknown services, unsupported operations and malformed arguments reject only
the originating call. They do not expose Nginx internals or terminate the worker.

## Capability flags

Descriptors may declare request-body and response-body capabilities:

```text
HOPLITE_HOST_PROVIDER_REQUEST_BODY
HOPLITE_HOST_PROVIDER_RESPONSE_BODY
```

These flags describe a reviewed provider profile; they do not by themselves
resolve a handle or grant authority. Concrete handle access must still be bound
to the owning work scope through the versioned data-plane ABI.

The initial built-in `nginx` provider declares neither capability.

## Registry and lifecycle laws

- service names are non-empty, exact and case-sensitive;
- duplicate registration fails;
- registry capacity is fixed at compile time;
- registered descriptors and service bytes outlive the worker;
- lookup is allocation-free;
- provider-specific arguments cross the boundary as exact HTA0 frames;
- one synchronous call completes before `invoke` returns;
- one pending call completes later on the owning worker event loop;
- cancellation precedes work-scope closure;
- late and duplicate completion fails closed;
- application values never select native pointers, filesystem paths or dynamic libraries; and
- provider registration is static in version 1.

## Tahto integration

The first external service is expected to be:

```text
tahto.metadata
```

It will decode TAHTO-8's closed `load`, `initialize`, `compare-and-swap` and
`receipt` requests, invoke the installed `tahto-metadata-store/0-alpha` provider and
return closed snapshot or receipt HTA frames through the completer.

The database path and provider package must come from trusted installation
configuration, never from the Hara request. Object upload and response streaming
remain separate provider profiles because they additionally require work-scoped
body handles and backpressure.
