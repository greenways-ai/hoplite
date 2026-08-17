---
title: 8. Application catalogue
description: Map Hoplite, standard Hara, streams, channels, and Relay onto practical system architectures.
---

Hoplite supplies execution and transport, not every product policy. The table
below separates compositions supported by current public surfaces from systems
that require an application database, host provider, or experimental contract.

| System | Core composition | Additional boundary | Maturity note |
| --- | --- | --- | --- |
| JSON/HTML API | Hara handlers and response maps | Optional database provider | Current core path |
| Async orchestration API | Handler coroutine and host Promises | Required service providers | Current suspension model |
| SSE/live feed | Stream transforms and `h/stream` | Event source | Streaming host contract is experimental |
| WebSocket fan-out | Hoplite Nchan declaration | Authorization and durable state | Bounded ephemeral transport |
| Collaborative RTC app | Nchan signalling, RTC Duplex, Relay | Identity, room state, persistence | RTC is experimental and worker-local |
| Telemetry gateway | Socket/process Duplex, framing, channels | Device protocol provider | Composition pattern; deployment-specific |
| Correlated RPC broker | Relay correlated mode | Codec and connection provider | Hara Relay available; transport-specific evidence required |
| Job supervisor | Command channel, process Duplex, `alts` | Process authority and durable queue | Worker-local coordinator, not durable scheduling |
| Language tooling | Process/socket Relay | LSP or custom codec | Strong fit for request IDs and events |
| AI token service | Async provider plus response stream | Model provider and quota policy | Streaming response remains experimental |
| Media pipeline | Partitioned streams and processes | Native media provider | Backpressure boundary must be measured |
| Notification router | Channels, transforms, external sends | Durable outbox/provider | Do not treat in-memory channels as durable queues |
| Device controller | Duplex Relay and session coroutine | Authentication and device registry | Handles remain scoped to their worker |
| Remote agent | Command inbox, Relay tools, event stream | Policy, isolation, persistence | Application architecture, not a built-in agent product |

## Realtime collaboration

Use Nchan for bounded WebSocket/SSE signalling and fan-out, then RTC for direct
worker-local data-channel traffic. Represent each peer as a session coroutine
selecting among inbound messages, room events, timers, and shutdown. Relay is
appropriate when messages need framing, acknowledgements, correlation, or
timeouts.

Hoplite does not provide identity, authorization policy, conflict resolution,
or durable documents. Those remain application concerns behind explicit
interfaces.

## Telemetry and event processing

Decode a source with stream transformations, partition values for efficient
storage, and introduce bounded channels only at concurrency boundaries. This
shape supports sensor ingestion, logs, audit events, incremental indexing, and
live dashboards.

The central operational question is what happens when storage is slower than
ingestion. A bounded design can pause reads, reject producers, aggregate samples,
or spill through a durable external queue. An unbounded in-memory queue merely
postpones the decision until failure.

## RPC and service gateways

Relay can translate a process, TCP socket, or RTC connection into Promise-based
exchanges. Correlated mode supports concurrent in-flight requests; the event
channel carries unsolicited notifications. This fits database proxies, language
servers, legacy adapters, remote tools, and device protocols.

The gateway must still define retry safety, authentication, codec limits, and
connection recovery. Relay deliberately does not invent those domain policies.

## Job and process systems

A channel can be a worker-local command inbox and a process Duplex can provide
input/output. `alts` coordinates commands, process output, timers, and shutdown.
This is useful for build runners, compiler services, media workers, and local
development tools.

An in-memory channel is not a durable job queue. Work that must survive a worker
restart needs an external durable owner and an idempotency policy.

## Agent-style systems

An agent can be modeled as private state owned by one coroutine, a bounded inbox,
one or more Relay connections, and an event stream. This provides sequential
state transitions and explicit cancellation without requiring a separate actor
runtime.

Tool authorization, audit, persistence, retry semantics, and isolation remain
outside `stream.async`. The concurrency primitive should not silently become the
security model.

## Streaming generation

Large exports, live logs, incremental queries, and generated tokens can all be
represented as streams. Transformations remain pull-oriented so downstream
pressure can stop upstream work. Provider response sources are preferable for
bounded native objects that already have authoritative length and ownership.

Choose between a logical stream and a provider source from the data's owner and
lifecycle, not only from its content type.

Next: [Performance by construction](../performance/).
