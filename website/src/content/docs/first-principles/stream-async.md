---
title: 5. Channels and stream.async
description: Coordinate bounded producers and consumers with coroutines, channels, non-blocking operations, and selection.
---

Streams separate demand from arrival. `std.stream.async` adds a coordination
point: a channel that can be written by one activity and read by another while
preserving the standard stream protocols.

## A coroutine is a scope for awaiting

`async/go` starts a coroutine and returns a Promise:

```clojure
(defn produce [output]
  (async/go
   (fn []
     (co/await (async/put output {:kind :started}))
     (co/await (async/put output {:kind :ready}))
     (async/close output))))
```

Use `co/await` inside the function passed to `go`. Outside a coroutine, compose
the returned Promise with `promise/then` or return it to an asynchronous caller.
`Test/run` also waits when a test function returns a Promise.

## Capacity is policy

```clojure
(def events (async/chan 32))
```

Capacity 32 means that a producer may get at most 32 values ahead before a
blocking `put` must wait for demand. Choose capacity from acceptable burst size,
value size, latency, and memory—not from a desire to make overload disappear.

## Waiting and immediate operations

`take` and `put` return Promises and may wait:

```clojure
(def value (co/await (async/take events)))
(def accepted (co/await (async/put events next-value)))
```

`poll` and `offer` make immediate attempts:

```clojure
(def available (async/poll events))
(def accepted-now? (async/offer events next-value))
```

Immediate operations suit optional telemetry, schedulers, and event-loop probes.
A failed `offer` is a policy decision point: drop, aggregate, retry elsewhere, or
record overload.

## Select among independent events

`alts` waits for the first available read or write operation:

```clojure
(let [[value source]
      (co/await
       (async/alts [commands shutdown]
                   {:priority false}))]
  (if (= source shutdown)
    (async/close commands)
    (handle-command value)))
```

Selection rotates its starting point by default to avoid permanently favoring
the first operation. Use `:priority true` only when order is intentional, or
`:default` for an immediate fallback. A write alternative is `[channel value]`.

## Bridge streams into concurrent consumers

`async/from-stream` pumps any `IStream` into a channel:

```clojure
(def decoded (stream/map decode-reading source))
(def inbox (async/from-stream decoded 64))
```

This is a deliberate concurrency boundary. Upstream remains a pull pipeline;
the channel permits independent downstream pacing and absorbs at most the
declared burst.

## Structured shutdown

Use `flush` when a sink exposes buffered completion, `close` for normal
shutdown, and `abort` with the original error for failure. A coroutine that owns
a source should close it in cleanup. Channels settle pending readers and writers
instead of leaving invisible background work.

## The actor-like pattern

A coroutine, a private state value, and an inbox form a small actor without a
separate actor runtime. The useful constraint is single ownership of state—not
the name “actor.” External interaction occurs through messages and lifecycle
operations, making transitions serial and testable.

## Worked example: producer and consumer

```clojure
(defn round-trip []
  (let [values (async/chan 2)]
    (async/go
     (fn []
       (co/await (async/put values 10))
       (co/await (async/put values 20))
       (async/close values)))
    (async/go
     (fn []
       [(co/await (async/take values))
        (co/await (async/take values))
        (co/await (async/take values))]))))
;; returned Promise resolves to [10 20 nil]
```

The final `nil` is EOF after close. No sentinel value is placed in the channel.

## Worked example: data or shutdown

```clojure
(defn next-action [inbox shutdown]
  (async/go
   (fn []
     (let [[value source]
           (co/await
            (async/alts [inbox shutdown]
                        {:priority false}))]
       (cond
         (= source shutdown) [:stop value]
         (= source inbox) [:message value]
         :else [:unknown value])))))
```

A timer can be represented by another compatible port. The session loop stays
sequential even though several events may become ready independently.

## Worked example: test asynchronous notation directly

```clojure
(Test/run
 [{:name "channel delivery"
   :test (fn []
           (let [port (async/chan 1)]
             (async/offer port 41)
             (async/go
              (fn []
                (inc (co/await (async/take port)))))))
   :expected 42}])
```

`Test/run` awaits the returned Promise. The test does not need `deref` or a
separate async testing library.

Next: [Duplex transports and Relay](../duplex-relay/).
