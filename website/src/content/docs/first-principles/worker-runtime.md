---
title: 2. Inside a Hoplite worker
description: Understand prepared handlers, synchronous dispatch, coroutine suspension, worker locality, and cleanup.
---

Hoplite embeds a Hara runtime into each Nginx worker. Nginx owns network event
readiness; Hara owns application values and execution. The boundary between them
is narrow and explicit.

```text
Nginx worker
├─ request routing and socket readiness
├─ one worker-local Hara runtime
│  ├─ prepared handler calls
│  ├─ Promises and suspended coroutines
│  └─ stream and Relay values
└─ trusted host providers
   ├─ request and response data plane
   └─ worker-local RTC sockets and timers
```

## Prepare once, dispatch many times

Production workers load deterministic application artifacts rather than parsing
HAL source for each request. The worker validates the application, resolves
handler Vars, and prepares calls before publishing readiness. Request dispatch
therefore begins from an already known function and route.

This is both a performance mechanism and a maintenance property. Startup owns
configuration failure; request handling owns application behavior. The two
failure domains do not blur together.

## The synchronous path stays synchronous

A handler begins as an ordinary call:

```clojure
(defn health [_request]
  {:status 200
   :headers {"content-type" "application/json"}
   :body "{\"ready\":true}\n"})
```

If it returns without suspension, Hoplite uses the direct response path. It
does not allocate a Promise/work record merely because the runtime also supports
asynchronous handlers.

## Suspension yields the worker

When a handler awaits a pending host operation, Hara retains the continuation
and returns control to the Nginx event loop:

```clojure
(defn delayed [_request]
  (co/await (Host/call "nginx" "sleep" [25]))
  {:status 200 :body "resumed\n"})
```

The worker can service other ready events while the operation is pending. When
the provider completes it, execution resumes in the owning runtime. This is
concurrency without assigning an operating-system thread to every waiting
request.

## Cancellation is not optional cleanup

Client disconnect, timeout, response completion, worker shutdown, and provider
failure must release retained continuations and native handles. A request body
handle cannot outlive its request. An RTC handle cannot outlive its worker. A
streaming response must stop pulling when Nginx can no longer send the result.

Keeping these transitions explicit prevents the common maintenance failure in
which the success path is clear but shutdown behavior is distributed across
callbacks.

## Scaling across workers

Workers scale independent request execution across CPU cores. They do not form a
shared Hara heap. Increase worker count for parallel serving, but place durable
or cross-worker state behind an explicit service. Worker locality avoids locking
inside ordinary application values; it also means that affinity and state
placement must be designed rather than assumed.

## Worked example: direct and suspended handlers

These handlers return the same response shape, but only one can suspend:

```clojure
(defn current-status [_request]
  {:status 200 :body "ready\n"})

(defn status-after-check [_request]
  (co/await (Host/call "nginx" "sleep" [5]))
  {:status 200 :body "ready\n"})
```

`current-status` stays on the direct path. `status-after-check` begins
synchronously and allocates retained asynchronous work only if the host Promise
is pending.

## Worked example: keep portable state separate

```clojure
(def session-summary
  {:peer-id "peer-17"
   :connected-at 1720000000})
```

This map may be serialized or stored. The live RTC handle used to produce it may
not: it belongs to one worker and one native session lifetime. Separating the
portable summary from live authority prevents accidental cross-worker use.

Next: [Standard Hara application design](../standard-hara/).
