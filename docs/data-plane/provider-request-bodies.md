# Work-scoped request bodies for native providers

## Purpose

A registered native provider may need to move an incoming request body into a
generic sink without copying the bytes into Hara values or receiving the Hoplite
runtime pointer.

The provider API therefore exposes two host-owned operations:

```c
hoplite_host_request_body_read_v1(
    request_context, work, handle, output, capacity, returned)

hoplite_host_request_body_finish_v1(
    request_context, work, handle)
```

These functions are transport mechanics. They contain no application-specific
upload, quota, object-graph, manifest, authorization or recovery rules.

## Authority tuple

Request-body access requires all of:

```text
exact opaque request context
exact owning work ID
currently active or retained provider
provider REQUEST_BODY capability flag
live body handle belonging to that work
```

The body handle is not authority by itself. Hoplite checks request and work
scope before calling the runtime resource registry, which independently rejects
foreign, stale and closed handles before invoking the native body descriptor.

A provider cannot use the functions after synchronous completion. A provider
that returns `HOPLITE_HOST_PROVIDER_PENDING` retains access only until it
completes, fails, or the request is cancelled.

## Read contract

A successful bounded read returns:

```text
HOPLITE_HOST_RESOURCE_OK
```

and writes the number of bytes to `returned`. End of input is represented by a
successful read with `returned == 0`.

The call fails closed when:

- request context is null or no longer live;
- work ID does not match the request;
- provider scope is inactive;
- provider omitted `HOPLITE_HOST_PROVIDER_REQUEST_BODY`;
- handle is zero, stale, closed or belongs to another work;
- output, capacity or returned arguments are invalid;
- the runtime enforces a smaller configured maximum chunk.

On failure, `returned` is set to zero when a pointer was supplied.

## Finish contract

`hoplite_host_request_body_finish_v1` consumes and closes the handle for its
owning work. Repeated finish, post-completion finish and foreign-work finish fail.
Request cleanup still closes an abandoned body through the owning work scope, so
a failed provider cannot leak the native descriptor.

A provider that claims to consume a body should explicitly finish it after
checking its own expected length. The generic staged-blob profile will require:

1. read no more than the closed requested length;
2. reject early EOF;
3. probe or otherwise establish that no excess bytes remain under its profile;
4. finish the body exactly once;
5. complete the Hara call only after sink and integrity checks succeed.

## Lifecycle ordering

```text
provider selected
  -> provider becomes active
  -> invoke may read/finish body
  -> synchronous completion clears provider access

provider selected
  -> provider becomes active
  -> invoke returns PENDING
  -> event-loop continuation may read/finish body
  -> completion or cancellation clears provider access
  -> work scope closes
```

Cancellation runs before work-scope closure so a provider can stop native work
and release provider-owned resources. Once provider ownership is cleared, late
body access fails.

## Generic blob integration

Hoplite issue #41 will use this boundary for an application-neutral service such
as:

```text
hara.blob / staging-append-request
```

The provider receives a closed staging ID, offset, length and body handle. It
uses the host-supplied work ID with the handle, moves bounded bytes into trusted
storage, and returns a closed generic result.

Tahto remains a HAL client. Its state machine validates uploads and maps its
closed effects to this generic service; no Tahto-specific native executor is
required.
