---
title: 3. Standard Hara application design
description: Use ordinary Hara values, functions, protocols, errors, and effects to keep web applications small and testable.
---

Hoplite does not replace Hara with a framework-specific language. A handler is a
function from a request value to a response value, and most application work can
remain ordinary collection transformation.

## Keep the center made of values

Separate transport interpretation, domain decisions, and response construction:

```clojure
(defn classify-reading [reading]
  (cond
    (> (:temperature reading) 80) :critical
    (> (:temperature reading) 65) :warning
    :else :normal))

(defn reading-response [reading]
  {:status 200
   :headers {"content-type" "application/json"}
   :body (json/write
          (assoc reading :classification
                 (classify-reading reading)))})
```

`classify-reading` has no knowledge of Nginx, channels, or WebRTC. It is cheap to
test and reusable in batch, request, and streaming contexts. The handler at the
edge is responsible for turning a request into the input value and returning a
response.

## Use collection pipelines before concurrency

Maps, vectors, sequences, and collection functions are the right tools while
all required values are already present. Concurrency cannot improve a pure
transformation that has no waiting or independent work; it only adds scheduling
and lifecycle concerns.

A useful progression is:

1. Write the transformation as an ordinary function.
2. Apply it to a stream when inputs arrive over time.
3. Add a channel only when producers and consumers need independent pacing.
4. Add Relay only when a bidirectional transport needs protocol semantics.

## Protocols state requirements

Protocols let a function depend on behavior rather than representation. This is
particularly important at I/O boundaries. A function accepting `IStream` can
read from a channel, native callback source, transformed stream, or transport
adapter without branching on its concrete type.

Keep protocol requirements narrow. A reducer does not need a writable Duplex;
it needs only a readable source. Narrow requirements improve substitution in
tests and reduce the number of lifecycle states a function must handle.

## Errors are values with context

Use structured errors at boundaries:

```clojure
(throw
 (ex-info "Reading is missing a device identifier"
          {:error/type :operations/invalid-reading
           :field :device-id}))
```

The message helps a human; the data supports stable classification. Do not make
downstream code parse error strings. At an asynchronous boundary, rejection or
`abort` should preserve the structured cause.

## Effects stay visible

Host calls return Promises. At the outer application boundary, either compose
those Promises or await them inside a coroutine. Keep domain transformations
synchronous when they do not need to wait. This makes the number and location of
effectful operations apparent during review.

## State follows ownership

An atom is appropriate for small state owned by one runtime value, such as Relay
bookkeeping. It is not a substitute for durable or cross-worker storage. State
that must survive reload or be shared between workers belongs behind an explicit
provider or external system.

## Worked example: reuse one rule in batch and stream code

```clojure
(defn decorate [reading]
  (assoc reading :classification (classify-reading reading)))

(def batch-result
  (map decorate stored-readings))

(def live-result
  (stream/map decorate live-readings))
```

The domain rule is unchanged. Only the container changes: an in-memory
collection for present data, and an `IStream` transformation for values arriving
over time.

## Worked example: translate a structured failure once

```clojure
(defn error-response [error]
  (let [data (ex-data error)]
    (if (= :operations/invalid-reading (:error/type data))
      {:status 400 :body "invalid reading\n"}
      {:status 500 :body "internal error\n"})))
```

Domain code throws structured information; the HTTP boundary chooses the public
status and message. A Relay or batch boundary can translate the same error data
differently without changing the domain function.

Next: [Streams from first principles](../streams/).
