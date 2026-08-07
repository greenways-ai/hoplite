# Hoplite worker-local data-plane registry

The registry owns native request and response body wrappers inside one Hoplite
worker and exposes only positive opaque handles to higher layers.

## Handle allocation

Handles are allocated from one process-wide sequence and transformed through a
process-salted bijection over the positive signed 64-bit range. This gives each
resource a value that is:

- non-zero and representable as a positive Hara integer;
- unique across every registry in the process until the 63-bit sequence is
  exhausted;
- non-monotonic at the application boundary; and
- never reused during the process lifetime.

A handle copied into a different worker registry therefore does not resolve to a
locally active resource. The salt is defense in depth only: a handle remains
transport lookup state rather than application authorization.

## Ownership

`insert_request` and `insert_response` are `unsafe` because they consume raw C
callback descriptors. Ownership transfers to the registry even when validation
or handle allocation fails. A valid context with a close callback is closed
exactly once on failure, removal, `close_all`, or registry drop.

## Operations

Request resources expose declared and observed length, bounded sequential reads,
and finish validation. Response resources expose immutable length and seekable
reads. Every operation verifies that the handle exists and has the expected
resource kind before invoking a callback.

## Authority and request scope

The registry owns process-local transport resources only. It does not own
application grants, management identity, keys, sessions, paths, upstreams, or
payload meaning. Authorization must be completed before a handler receives an
operation-specific handle.

Process-wide handle uniqueness prevents accidental aliasing between worker
registries, but it is not the complete request-scope check. The runtime layer
that exposes a handle must also associate it with the owning request or suspended
work and verify that scope on every native read or finish operation. A handle
learned by another concurrent request must not be accepted merely because it is
present in the same worker registry.
