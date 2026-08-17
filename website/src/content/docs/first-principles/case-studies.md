---
title: 7. Progressive case studies
description: Grow one operations service from ordinary HTTP handlers into streams, bounded concurrency, process integration, and RTC Relay.
---

The following stages add complexity only when a new requirement demands it.
They are architectural slices rather than a single copy-and-paste deployment;
provider availability and experimental surfaces are called out at each boundary.

## Stage 1: an ordinary HTTP decision

Begin with a pure function and a small handler:

```clojure
(defn severity [reading]
  (cond
    (> (:value reading) 80) :critical
    (> (:value reading) 65) :warning
    :else :normal))

(defn health [_request]
  {:status 200
   :headers {"content-type" "application/json"}
   :body "{\"ready\":true}\n"})
```

Register the handler Var in immutable application data. This stage has no
coroutine, channel, or stream because it does not wait for anything and does not
coordinate independent activity.

**Maintenance payoff:** domain classification can be tested without starting
Nginx. **Performance mechanism:** the handler follows Hoplite's prepared,
synchronous response path.

## Stage 2: wait without blocking the worker

Suppose a readiness response must wait briefly for a host event:

```clojure
(defn readiness [_request]
  (co/await (Host/call "nginx" "sleep" [10]))
  {:status 200 :body "ready\n"})
```

Only the pending operation introduces suspension. The worker returns to its
event loop and resumes the Hara continuation on completion.

**Maintenance payoff:** control flow remains sequential and the failure belongs
to the returned asynchronous result. **Performance mechanism:** waiting does not
reserve an operating-system thread per request.

## Stage 3: transform a reading stream

When readings arrive over time, reuse the original domain function:

```clojure
(def classified
  (stream/map
   (fn [reading]
     (assoc reading :severity (severity reading)))
   readings))

(def urgent
  (stream/filter
   (fn [reading]
     (not (= :normal (:severity reading))))
   classified))
```

No application queue has been introduced. Values are pulled only when the
consumer demands them.

## Stage 4: decouple bounded producers and consumers

A source may receive short bursts while downstream encoding has variable cost:

```clojure
(def inbox (async/from-stream urgent 64))

(defn consume-alerts []
  (async/go
   (fn []
     (loop [processed 0]
       (let [alert (co/await (async/take inbox))]
         (if (nil? alert)
           processed
           (do
             (co/await (persist-alert alert))
             (recur (inc processed)))))))))
```

Capacity 64 is now explicit system policy. After the burst is absorbed,
backpressure reaches the upstream pump.

## Stage 5: expose a streaming response

`hoplite.core/stream` can mark a producer as a logical backpressured response:

```clojure
{:status 200
 :headers {"content-type" "text/event-stream"}
 :body (h/stream encoded-events)}
```

This host contract is experimental. A production design must test slow-client
behavior, cancellation, and closure against the exact Hoplite version. Use a
provider response source instead when serving an already-authorized bounded
native object.

## Stage 6: supervise a process or socket protocol

Adapt the native process or socket to `IStreamDuplex`, add framing, and let Relay
own request matching:

```clojure
(def service
  (relay/relay transport
               (frame/line)
               {:timeout-ms 2000}))
```

Application handlers can exchange values with `service` without knowing whether
the implementation is a subprocess, Unix socket, TCP connection, or test
transport. Process and socket creation remain deployment capabilities rather
than arbitrary authority derived from a request value.

## Stage 7: add a worker-local RTC client

After application signalling completes:

```clojure
(def transport (rtc/connect handle))
(def peer
  (relay/relay transport
               codec
               {:timeout-ms 5000}))
```

The same Relay-facing application logic now runs over RTC. The opaque handle and
transport must stay inside their Nginx worker. Use Nchan or another external
service for signalling and cross-worker fan-out; do not place live RTC handles
in shared application data.

## What the progression demonstrates

Each layer solves one new problem:

| Requirement | Added abstraction |
| --- | --- |
| Transform present values | Standard Hara function |
| Wait for one host event | Promise/coroutine |
| Transform values over time | `IStream` pipeline |
| Decouple bounded pacing | Channel |
| Coordinate several events | `alts` |
| Model bidirectional I/O | `IStreamDuplex` |
| Add application protocol semantics | Relay |

Starting at the lowest sufficient layer reduces allocations and, more
importantly, reduces the lifecycle states future maintainers must understand.

Next: [Application catalogue](../application-catalogue/).
