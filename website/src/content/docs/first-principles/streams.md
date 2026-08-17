---
title: 4. Streams from first principles
description: Learn how asynchronous pull, transformations, EOF, lifecycle, and backpressure differ from collections and iterators.
---

A collection answers “which values exist?” A stream answers “what value becomes
available next?” The distinction matters when values arrive from a socket,
process, RTC session, timer, or producer whose lifetime is longer than one call.

## The smallest useful read interface

`IStream/next` returns a Promise for the next value. A resolved `nil` means
normal end-of-stream. Rejection means failure. The consumer decides when to ask
again, so pull frequency naturally expresses demand.

```clojure
(defn consume [source step initial]
  (async/go
   (fn []
     (loop [state initial]
       (let [value (co/await (IStream/next source))]
         (if (nil? value)
           state
           (recur (step state value))))))))
```

Unlike a conventional synchronous iterator, `next` does not have to block a
thread while waiting. Unlike an eager collection, the entire input does not have
to fit in memory.

## Transform without changing ownership

`std.stream.common` supplies familiar transformations including `map`, `filter`,
`keep`, `remove`, `take`, `drop`, `concat`, `zip`, `interleave`, and partitioning.
Terminal operations include `reduce`, `collect`, `first`, `last`, `count`,
`some`, `every?`, and `find`.

```clojure
(def important
  (stream/take 100
   (stream/filter urgent?
    (stream/map decode-reading source))))
```

This stays declarative: the consumer pulls, each stage requests only what it
needs, and intermediate collections are not required. Closing a transformed
source closes the source it owns.

## Backpressure is demand propagation

If Nginx cannot send another response chunk, it should not request another
application value. If a downstream transform has no demand, it should not keep
reading the upstream socket. Pull-based streams can propagate that pause to the
producer boundary.

Backpressure is not synonymous with buffering. A buffer absorbs a bounded burst;
backpressure controls what happens after that capacity is used.

## Partitioning changes the cost model

Partitioning can amortize framing, serialization, and host-call overhead:

```clojure
(def batches
  (stream/partition-all 64 readings))
```

Larger batches may improve throughput but add latency and retained memory. The
correct size is a measured workload choice, not a universal constant.

## EOF, close, and abort are different

- EOF is an observed normal end: `next` resolves to `nil`.
- `close` asks an owned resource to end normally.
- `abort` ends it with an error and settles pending operations accordingly.

Do not use a magic application value as a shutdown sentinel when the stream
already has lifecycle protocols. Sentinels collide with legitimate values and
usually lose the failure cause.

## When a sequence is still better

Use an ordinary collection or iterator for finite in-memory data when waiting,
backpressure, cancellation, and incremental cleanup are irrelevant. Streams
earn their complexity at temporal or resource boundaries.

## Worked example: decode, filter, and batch

```clojure
(def batches
  (stream/partition-all
   32
   (stream/filter valid-reading?
    (stream/map decode-reading byte-source))))

(def first-batch
  (co/await (IStream/next batches)))
```

One downstream pull may read several upstream chunks until 32 valid readings are
available. Invalid readings are discarded by `filter`; no intermediate
collection of all decoded readings is required.

## Worked example: consume a finite source

```clojure
(def source (stream/from-iterator [1 2 3 4]))

(def total
  (co/await (stream/reduce + 0 source)))
;; total => 10
```

The same terminal operation works when the source is backed by callbacks rather
than an in-memory iterator. The difference is that each step may settle later.

## Worked example: combine sources deliberately

```clojure
(def paired
  (stream/zip device-identifiers live-readings))

(def alternating
  (stream/interleave high-priority normal-priority))
```

`zip` stops when either side ends and produces paired values. `interleave`
alternates pulls; it is ordering policy, not concurrent selection. Use `alts`
when readiness rather than a fixed order should choose the source.

Next: [Channels and `stream.async`](../stream-async/).
