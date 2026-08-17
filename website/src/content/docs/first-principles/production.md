---
title: 11. Production reasoning
description: Apply ownership, backpressure, cancellation, diagnostics, security, and measurement to operating Hoplite systems.
---

A production design is complete when normal traffic, overload, failure,
cancellation, reload, and recovery all have owners and observable outcomes.

## Place state deliberately

Worker-local state is appropriate for connection sessions, prepared handlers,
short-lived caches, and coordination that may disappear with the worker. Durable
records, cross-worker identity, room membership, job ownership, and retry history
belong in an external system with explicit consistency semantics.

Nchan provides bounded ephemeral fan-out. RTC provides worker-local direct
traffic. Neither is a database, identity service, or durable queue.

## Design the overload path

For every channel, stream, Relay mailbox, request body, and provider buffer,
record:

- the bound and its unit;
- the owner of retained values;
- behavior at capacity;
- timeout and cancellation behavior;
- metrics or diagnostics emitted;
- how memory is released on shutdown.

Prefer a controlled rejection or pause over an unbounded backlog. If data cannot
be lost and producers cannot be slowed, introduce a durable queue rather than a
larger in-memory channel.

## Treat cancellation as a routine event

Clients disconnect, deployments reload workers, and deadlines expire. Exercise
these events while reads, writes, channel puts, Relay exchanges, and provider
operations are pending. The acceptance criterion is not only that the caller
receives an error; owned native and Hara resources must also return to baseline.

## Separate authority from data

Opaque body, source, and RTC handles are scoped authority. Do not serialize,
log as reusable credentials, or reconstruct them from request data. A request
may select among application-approved operations, but it must not select an
arbitrary provider, filesystem path, process command, or network destination.

Authentication and authorization remain application policy. Stream protocols
standardize lifecycle; they do not grant permission.

## Observe mechanisms, not only symptoms

Useful diagnostics include request identity, worker generation, suspended work,
channel capacity and occupancy, failed offers, Relay pending count, timeout
class, close/abort reason, and provider cleanup. Bound labels and cardinality so
the telemetry system does not become the next unbounded queue.

Hoplite's startup diagnostics distinguish configuration, bundle validation,
module loading, route preparation, and readiness. Preserve those phases so an
operator can tell whether a worker failed before or after application state was
published.

## Scale with evidence

Production defaults may use available CPU parallelism, but worker count should
be selected from throughput, tail latency, marginal RSS, provider limits, and
connection affinity. More workers duplicate worker-local runtime state and do
not accelerate one CPU-heavy request automatically.

Measure representative synchronous, suspended, streaming, and Relay workloads.
Include slow consumers and failure cleanup. Record exact Hara and Hoplite
revisions so capacity decisions survive software changes.

## Deployment checklist

Before release, verify:

1. The application bundle and configuration validate before serving.
2. Every native provider is installed by trusted deployment configuration.
3. Worker-local handles never cross a worker or outlive their work.
4. Buffer, body, mailbox, frame, and timeout bounds are documented.
5. Close, abort, disconnect, timeout, and reload tests release resources.
6. Experimental surfaces are pinned to the tested Hoplite version.
7. Performance reports contain raw samples and compatible machine metadata.
8. Durable application state has an external owner and recovery policy.

## The first-principles result

Hoplite does not need a different abstraction for every kind of application.
Standard Hara handles present values and domain policy. Streams handle values
over time. Channels coordinate independently paced activities. Duplex composes
bidirectional capabilities. Relay adds protocol semantics. Nginx and native
providers own transport readiness.

The architecture performs well when it avoids unnecessary work, stays bounded
under pressure, and yields during waits. It remains maintainable when ownership,
effects, failure, and cleanup can be understood at the boundary where they
occur.

Return to the [book overview](../).
