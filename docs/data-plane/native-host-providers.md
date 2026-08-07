# Registered native host providers

Hoplite dispatches Hara host calls through a fixed-capacity registry owned by one
Nginx worker. Registration happens once during worker initialization. Lookup is
an exact, case-sensitive service-name match and allocates no memory on the
steady-state request path.

## Call boundary

A provider receives a closed call envelope containing:

```text
owning request context
work ID
call ID
operation name
HTA argument value
request-pool context
Nginx log
```

The work ID is part of the authority boundary. A future provider that resolves a
request-body or response-source handle must use the exact `(work ID, handle)`
pair. A numeric handle alone is transport state and grants no authority.

Provider descriptors declare capability flags for request-body and
response-source access. The initial `nginx` provider declares neither capability.

## Completion

A provider may:

- return `NGX_OK` after resolving or rejecting the call synchronously;
- return `NGX_AGAIN` after retaining one request-scoped operation; or
- return `NGX_ERROR` when the native boundary itself cannot continue.

When an operation is retained, Hoplite records the owning provider on the
request. Request completion or cleanup invokes that provider's cancellation
callback before the work scope closes. A provider must resolve, reject, or cancel
an operation exactly once.

The first profile permits one retained native operation per request. A second
call while an operation is retained is rejected without terminating the worker.

## Built-in Nginx provider

The existing timer operation is registered as:

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

## Registry laws

- service names are non-empty and exact;
- duplicate registration fails worker startup;
- registry capacity is fixed at compile time;
- registered provider objects and service bytes outlive the worker registry;
- lookup is allocation-free;
- provider cancellation precedes work-scope closure;
- application values never select native pointers, filesystem paths or dynamic libraries;
- provider registration is static in the first profile.

## Extending the registry

The first external provider is expected to implement Tahto's closed upload
effects. It must derive storage paths from server configuration and canonical
object identities, and it may read a V3 request body only through the owning
work ID and opaque body handle.
