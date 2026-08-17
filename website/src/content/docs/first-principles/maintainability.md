---
title: 10. Maintainability by construction
description: Use value-oriented design, narrow protocols, bounded queues, and centralized lifecycle handling to reduce system complexity.
---

Maintainability is the cost of answering questions after the original author is
gone: where a value came from, who owns it, what can block, how it fails, and
what must be cleaned up. Hoplite and Hara streams help when they make those
answers local and explicit.

## Pure transformations isolate policy

Business decisions expressed as ordinary functions can be tested with values,
reused in request and stream paths, and reviewed without simulating network
timing. Keep parsing and host calls at the edge; keep classification, validation,
and transition logic in the center.

This does not require every function to be pure. It requires effects to be
visible at boundaries instead of being reachable through ambient global state.

## Protocols prevent transport leakage

Code depending on `IStream` or `IStreamDuplex` does not need separate branches
for Java, Rust, RTC, sockets, processes, or in-memory tests. A new adapter
implements the same behavioral contract while Relay and application code remain
unchanged.

The benefit is not merely fewer type names. It is one lifecycle vocabulary:
read, write, poll, offer, flush, close, abort, and closed state.

## Bounded queues expose overload policy

Capacity becomes a reviewable configuration decision. A full queue has a known
response and can be exercised in a focused test. This is easier to maintain than
an implicit backlog distributed across callbacks, futures, and transport
buffers.

Document why each buffer exists, who chooses its capacity, and what happens when
it fills. A buffer without an overload policy is unfinished design.

## Coroutines preserve local control flow

Sequential asynchronous logic keeps acquisition, await points, and cleanup in
one lexical region. This avoids deeply nested callback state machines. The gain
is strongest when cancellation and errors use the same structured path as normal
completion.

Coroutines can still become unmaintainable if they launch detached work with no
owner. Every `go` result should be returned, retained by an owner, or explicitly
treated as supervised background work.

## Relay centralizes protocol machinery

Framing, pending request correlation, timeout removal, unsolicited event
dispatch, and connection-wide failure are generic protocol concerns. Relay
implements them once. Application code supplies the codec and domain-specific
identifier functions rather than reimplementing a pending map for every
transport.

## Immutable application data improves tooling

Because the resource tree is data, Hoplite can validate it, generate OpenAPI,
prepare handlers, inspect build output, and report configuration errors before
serving. Deterministic bundles also make deployment differences attributable to
specific inputs rather than source discovery at runtime.

## Test at the narrowest boundary

Use a progression of evidence:

1. Value tests for pure transformations.
2. Stream tests for ordering, EOF, close, abort, and backpressure.
3. Relay tests for framing, timeout, correlation, overflow, and failure cleanup.
4. Provider fixtures for ownership and native callbacks.
5. Real-image tests for Nginx cancellation, disconnect, reload, and slow clients.

This structure keeps most failures cheap to reproduce while retaining evidence
at the native boundary where substitution is insufficient.

## Make maturity visible

Experimental surfaces should be labelled in code examples and architecture
decisions. Do not allow an example to turn a provisional interface into an
accidental compatibility promise. Stable concepts—ownership, bounds, lifecycle,
and protocol separation—can still organize code while concrete alpha APIs
evolve.

Next: [Production reasoning](../production/).
