---
title: 1. Values and boundaries
description: Derive Hoplite's architecture from ownership, boundedness, explicit capabilities, and lifecycle laws.
---

Frameworks often begin with routing syntax. Hoplite begins with boundaries,
because most production failures occur where ownership or lifetime was assumed
rather than stated.

## Applications are descriptions

A Hoplite application is an immutable Hara value describing resources and
handler Vars. Declaring a route does not run the handler:

```clojure
(def app
  (h/app
   {:name "operations"
    :resources
    [["/health"
      {:get {:name :health
             :handler #'health}}]]]}))
```

This separation permits Hoplite to validate the whole application, resolve its
handlers, generate OpenAPI and Nginx configuration, and prepare dispatch before
the worker accepts traffic. Invalid configuration fails at startup instead of
becoming a rarely exercised request path.

## Ownership is part of correctness

Each production Nginx worker owns one Hara runtime. Request views, native body
handles, RTC sessions, Promises, coroutines, and transport callbacks remain in
that worker. An opaque handle is authority inside its owner; it is not portable
data and must not be persisted or sent to another worker.

This rule removes an entire class of questions from application code. There is
no implicit cross-thread access to runtime values and no hidden migration of a
live socket between workers. Cross-worker coordination must use an explicit
external mechanism such as Nchan, a database, or another service.

## Bounds turn load into behavior

An unbounded queue converts temporary overload into future memory exhaustion.
Hoplite and Hara streams instead make important limits visible:

- request bodies declare maximum total and chunk sizes;
- response sources expose bounded reads;
- channels have explicit buffer capacity;
- Relay has a mailbox limit and request timeouts;
- Nchan locations are bounded, ephemeral fan-out;
- native values have a defined owner and closing event.

Bounds do not eliminate overload. They determine what overload means: wait,
reject, time out, disconnect, or shed optional work. That behavior can be tested.

## Capabilities instead of concrete classes

A value participates in streaming because it satisfies protocols such as
`IStream`, `IStreamWrite`, `IClose`, or `IAbort`. An `IStreamDuplex` composes the
read and write sides without requiring a special native Duplex box.

Code that only reads asks for `IStream`. Code that sends asks for
`IStreamWrite`. Relay asks for `IStreamDuplex` because it needs both directions.
This keeps requirements narrow and makes in-memory channels, RTC sessions,
processes, sockets, and test transports substitutable at the application layer.

## Lifecycle laws

Every long-lived value should answer four questions:

1. Who owns it?
2. What operation completes normally?
3. What operation reports failure?
4. What happens to pending work when it ends?

Streams use EOF for normal exhaustion, `close` for orderly shutdown, and
`abort` for failure. Pending readers and writers must settle rather than remain
orphaned. These laws matter more than the transport used underneath them.

## Worked example: make overload a result

An optional audit event should not delay the request that produced it. Use an
immediate offer and make the full-buffer decision visible:

```clojure
(defn record-audit [audit-events event]
  (if (IStreamOffer/offer audit-events event)
    {:accepted true}
    {:accepted false :reason :audit-buffer-full}))
```

The caller can count the dropped event, aggregate it, or fail the request. The
important property is that overload is a returned result rather than hidden
retained work.

## Worked example: ask only for the capability used

```clojure
(defn close-owned [value]
  (if (satisfies? IClose value)
    (IClose/close value)
    value))
```

This helper does not require a channel or Duplex. Narrow capability checks let
the same cleanup code work for different owners without concrete-type branches.

Next: [Inside a Hoplite worker](../worker-runtime/).
