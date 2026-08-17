---
title: Runtime model
description: Build/control ownership, worker-local execution, embedded realtime transport, and the slim production server.
---

Hoplite has two executable surfaces with deliberately different responsibilities.

```text
hoplite
  build · check · REPL · packages · service lifecycle

hoplite-server
  Nginx · Nchan · worker-local Hara runtime · prepared application handlers
```

The all-in-one `hoplite` command remains the development and operational control
surface. `hoplite-server` is the production data plane: it contains the embedded
Nginx/Hara server and Nchan module, replaces itself with Nginx at startup, and
does not retain the compiler, REPL, package tooling, authentication store, or
management gateway in server memory.

The core serving plane owns provider-neutral host and request/response
transport. Storage, blob custody, canonical-value verification, secrets, and
application authorization are installed by a distribution rather than bundled
into the default executable.

Build once, then run the production artifact:

```shell
hoplite serve build --mode prod /path/to/project
hoplite-server /path/to/project
```

The published container image performs the build in its builder stage and runs
only `hoplite-server` in the final image.

## Worker ownership

There is one Hoplite runtime per Nginx worker. Runtime values do not cross worker boundaries.

```text
┌──────────────────── Nginx worker ────────────────────┐
│ Hoplite runtime                                      │
│  ├─ decoded application definitions                 │
│  ├─ worker-local router and compiled handler calls  │
│  ├─ values, fibers, promises, and native streams    │
│  ├─ opaque handles to installed host providers      │
│  └─ worker-local WebRTC sessions and UDP events     │
└──────────────────────────────────────────────────────┘
            │
            └─ bounded Nchan locations are rendered
               into the enclosing Nginx configuration
```

Native providers are registered during trusted worker initialization. Each
service has one exact name and immutable descriptor; application values cannot
register providers or select their paths, drivers, or credentials.

WebRTC session handles are also worker-local. A handle cannot be persisted,
transferred to another worker, or treated as a Tahto identity or grant. The
worker owns each nonblocking UDP socket and the RTC timer driven by the engine's
next timeout.

Nchan channel state belongs to Nginx rather than the Hara runtime. Hoplite emits
a closed, bounded Nchan configuration from validated `hoplite.core/channel`
data; applications cannot inject raw directives, Redis coordinates, or dynamic
upstreams.

## Startup

1. `hoplite serve build` evaluates the selected project profile and validates the application, including routes, fixed proxies, channels, and peer declarations.
2. Build output records applications and prepared routes in `apps.hta` and writes the generated Nginx configuration, including bounded Nchan locations where declared.
3. `hoplite-server` checks that the build output exists, materializes its embedded server when necessary, and replaces itself with Nginx.
4. Each Nginx worker creates one Hara runtime, loads the application definitions, installs trusted host providers, prepares its router, and compiles every handler call once.

The release build strips the native Nginx/Hara server before embedding it in both
executables. The production artifact therefore carries one stripped serving
plane instead of the full Hoplite control program plus a second unstripped
server executable.

## Request execution

For each request, the worker matches method and path against borrowed Nginx
method/path slices and invokes the cached handler. The default `:request`
adapter exposes a lazy map-like extension: fields and headers are read only
when HAL code asks for them. A synchronous response is retained by the runtime
and referenced directly by Nginx until request cleanup; it does not allocate a
work record or create HTA events.

Only an actual `await` suspension creates a fiber/work record and enters the
Nginx event-loop continuation path. `:request+hta` remains available when a
fully materialized portable value is required.

`hoplite.core/stream` uses the same continuation path for real backpressured
HTTP bodies. An `IStream` is retained directly; a finite iterable is converted
to an iterator-backed generated stream whose iterator closes on completion or
failure.

## Realtime execution

A declared channel is a bounded ephemeral WebSocket/SSE fan-out endpoint. Nchan
calls application-owned authorization and publisher-admission handlers through
private prepared routes before accepting traffic. The signalling messages are
application records: Hoplite does not infer their sender, recipient, ordering,
replay policy, or expiry.

`hoplite.rtc/open` creates a session in the current worker. SDP offer/answer
exchange remains application signalling. After negotiation,
`hoplite.rtc/connect` exposes the session as a regular Hara value composed from
the stream Duplex protocols. Its reads, writes, timeouts, UDP readiness,
cancellation, and close are integrated with that worker's Nginx event loop.

One inbound RTC message satisfies a pending read or occupies the bounded receive
slot. Cancelling a pending read detaches it without implicitly closing the
session. Explicit close, provider failure, worker shutdown, and cleanup release
the worker-owned resources.

## Bytecode status

Application bytecode is generated and validated during the build. Production
Nginx workers receive the deterministic `app.hbx` artifact through the alpha
bootstrap ABI and load its eager HBC modules transactionally. A failed module
load rolls the application bundle back instead of exposing a partially
initialized worker. HAL source remains an authoring and development input; it
is not the production worker bootstrap artifact.

## Worker defaults

Development mode uses one worker unless `:workers` is specified. Production
builds default to the available parallelism of the build host, while the slim
server may override that value at deployment:

```shell
hoplite-server --workers auto /path/to/project
HOPLITE_WORKERS=4 hoplite-server /path/to/project
```

The published container sets `HOPLITE_WORKERS=auto`, avoiding a worker count
baked in by the image builder. Set the variable to an empty value to retain the
project's generated setting. A `hoplite.internal/config` may set
`:worker-processes` explicitly for deployments that do not apply an override.
