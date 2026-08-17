---
title: 6. Duplex transports and Relay
description: Compose bidirectional transports and add framing, correlation, events, timeouts, and failure propagation.
---

A socket or RTC data channel is bidirectional, but application code should not
need a unique concurrency model for each transport. Hara composes a readable
source with write and lifecycle callbacks into a regular `IStreamDuplex` value.

## Duplex is a capability composition

An `IStreamDuplex` also exposes the capabilities required to read, write, close,
abort, and inspect lifecycle. It is not a special native box. Hoplite RTC uses
the same Hara-level composition as process and socket adapters, while the
underlying native handle remains owned by its runtime.

Application logic can therefore accept a Duplex transport without importing
Hoplite RTC, Java socket classes, or Rust process types. Tests can provide an
in-memory transport satisfying the same protocols.

## Transport is not protocol

A raw byte stream does not define message boundaries, request matching, or
timeouts. Relay adds those application-protocol concerns:

```clojure
(def client
  (relay/relay transport
               (frame/line)
               {:timeout-ms 5000}))
```

Use direct `IStream/next` and `IStreamWrite/write` when transport messages are
already complete application values. Add Relay when the system needs framing,
serialized request/response exchange, concurrent correlation, unsolicited
events, or consistent timeout behavior.

## Serial and correlated exchange

Serial mode permits one active exchange. It is simple and appropriate for a
protocol in which responses strictly follow requests.

Correlated mode permits multiple requests in flight. The caller supplies a
function that adds an identifier to outgoing requests and another that extracts
the identifier from responses:

```text
request 41 ─────────────── response 41
request 42 ─────── response 42
request 43 ─────────────────── response 43
```

Completion order no longer has to match send order. Relay owns the pending map,
timeouts, and response dispatch so that callers receive ordinary Promises.

## Unsolicited events

A frame that is not a pending response is offered to Relay's bounded event
channel. This supports notifications and server-pushed changes without confusing
them with replies. If the event mailbox is full, Relay fails explicitly rather
than accumulating unbounded memory.

## Failure is connection-wide when integrity is lost

EOF, decode failure, transport rejection, event overflow, or explicit abort can
invalidate the whole protocol session. Relay rejects pending exchanges, aborts
its queues, and closes the Duplex transport. Individual request timeouts remove
their own pending entry.

This centralization is a maintenance advantage: protocol cleanup is implemented
once instead of being recreated by every application callback.

## Hoplite RTC ownership

`hoplite.rtc/connect` returns the portable Duplex composition. The session,
nonblocking UDP socket, timer, and opaque handle remain in the Nginx worker that
created them. Nchan can carry bounded signalling and fan-out, but it does not
turn the RTC handle into cross-worker data.

## Worked example: compose a test Duplex

```clojure
(def incoming (async/chan 4))
(def sent (atom []))

(def transport
  (duplex/create
   incoming
   (fn [value]
     (swap! sent conj value)
     (promise/from true))
   (fn [] (async/close incoming))))
```

The readable side is a channel; writes are captured in an atom; closing the
Duplex closes the source. Relay tests can use this value without opening a
socket or RTC session.

## Worked example: direct messages versus Relay

```clojure
;; One RTC message is already one application value.
(co/await (IStreamWrite/write transport {:kind :presence}))

;; A byte transport needs line framing and request timeouts.
(def service
  (relay/relay transport
               (frame/line)
               {:timeout-ms 3000}))
```

Use the first form when the transport already preserves the application's
message boundary. Relay is valuable only when it adds missing protocol behavior.

## Worked example: correlated requests

```clojure
(def correlated
  (relay/relay
   transport
   codec
   {:mode :correlated
    :timeout-ms 5000
    :prepare-request
    (fn [id request]
      [id (assoc request :request-id id)])
    :response-id
    (fn [response] (:request-id response))}))
```

Several calls may now be in flight. Responses are delivered by `:request-id`,
while frames without a matching pending request enter the bounded event channel.

Next: [Progressive case studies](../case-studies/).
