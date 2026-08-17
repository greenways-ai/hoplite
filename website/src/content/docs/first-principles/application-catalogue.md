---
title: 8. Application catalogue
description: Map Hoplite, standard Hara, streams, channels, and Relay onto practical system architectures.
---

Hoplite supplies execution and transport, not every product policy. The table
below separates compositions supported by current public surfaces from systems
that require an application database, host provider, or experimental contract.

| System | Core composition | Additional boundary | Maturity note | Code |
| --- | --- | --- | --- | --- |
| JSON/HTML API | Hara handlers and response maps | Optional database provider | Current core path | [Example](#json-and-html-api) |
| Async orchestration API | Handler coroutine and host Promises | Required service providers | Current suspension model | [Example](#async-orchestration-api) |
| SSE/live feed | Stream transforms and `h/stream` | Event source | Streaming host contract is experimental | [Example](#sse-and-live-feed) |
| WebSocket fan-out | Hoplite Nchan declaration | Authorization and durable state | Bounded ephemeral transport | [Example](#websocket-fan-out) |
| Collaborative RTC app | Nchan signalling, RTC Duplex, Relay | Identity, room state, persistence | RTC is experimental and worker-local | [Example](#collaborative-rtc-app) |
| Telemetry gateway | Socket/process Duplex, framing, channels | Device protocol provider | Composition pattern; deployment-specific | [Example](#telemetry-gateway) |
| Correlated RPC broker | Relay correlated mode | Codec and connection provider | Hara Relay available; transport-specific evidence required | [Example](#correlated-rpc-broker) |
| Job supervisor | Command channel, process Duplex, `alts` | Process authority and durable queue | Worker-local coordinator, not durable scheduling | [Example](#job-supervisor) |
| Language tooling | Process/socket Relay | LSP or custom codec | Strong fit for request IDs and events | [Example](#language-tooling) |
| AI token service | Async provider plus response stream | Model provider and quota policy | Streaming response remains experimental | [Example](#ai-token-service) |
| Media pipeline | Partitioned streams and processes | Native media provider | Backpressure boundary must be measured | [Example](#media-pipeline) |
| Notification router | Channels, transforms, external sends | Durable outbox/provider | Do not treat in-memory channels as durable queues | [Example](#notification-router) |
| Device controller | Duplex Relay and session coroutine | Authentication and device registry | Handles remain scoped to their worker | [Example](#device-controller) |
| Remote agent | Command inbox, Relay tools, event stream | Policy, isolation, persistence | Application architecture, not a built-in agent product | [Example](#remote-agent) |

## Code examples

The examples below show the smallest useful composition for each row. They
assume the aliases introduced in the preceding chapters: `h`, `co`, `async`,
`stream`, `relay`, `rtc`, `nchan`, `frame`, `json`, and `IStreamWrite`.

Names such as `catalogue-db/...`, `event-source/...`, `policy/...`,
`process-provider/...`, and `agent-store/...` are deliberately visible
application-owned boundaries. They are not additional Hoplite APIs.

### JSON and HTML API

A single application can return HTML from one handler and an encoded JSON
response from another:

```clojure
(defn index [_]
  {:status 200
   :headers {"content-type" "text/html; charset=utf-8"}
   :body "<h1>Catalogue</h1>"})

(defn status [_]
  {:status 200
   :headers {"content-type" "application/json; charset=utf-8"}
   :body
   (json/write
    {:status "ok"
     :revision (catalogue-db/revision)})})

(def app
  (h/app
   {:name "catalogue-api"
    :resources
    [["/" {:get {:name "index"
                 :handler #'index}}]
     ["/status" {:get {:name "status"
                       :handler #'status}}]]}))
```

**Application boundary:** `catalogue-db/revision` is an optional database
adapter. The handlers and response maps are on the current core path.

### Async orchestration API

An async handler can suspend on more than one provider operation without
blocking its worker:

```clojure
(defn ^:async create-release [request]
  (let [plan
        (co/await
         (std.native.Host/call
          "release.catalogue"
          "plan"
          [request]))
        result
        (co/await
         (std.native.Host/call
          "release.executor"
          "start"
          [plan]))]
    {:status 202
     :headers {"content-type" "application/json; charset=utf-8"}
     :body (json/write result)}))
```

**Application boundary:** both provider names and their operations belong to
the deployment. Hoplite supplies the handler suspension model.

### SSE and live feed

Map application events into SSE records, then mark the transformed stream as
the response body:

```clojure
(defn encode-sse [event]
  (str "data: " (json/write event) "\n\n"))

(defn live-events [request]
  (let [source (event-source/open request)
        body (stream/map encode-sse source)]
    {:status 200
     :headers {"content-type" "text/event-stream"
               "cache-control" "no-cache"}
     :body (h/stream body)}))
```

**Application boundary:** `event-source/open` owns event production and
lifecycle. The `h/stream` host response contract remains experimental.

### WebSocket fan-out

Declare a bounded Nchan topic and route authorization through application
callbacks:

```clojure
(defn authorize-publisher [request]
  (policy/authorize request :publish))

(defn authorize-subscriber [request]
  (policy/authorize request :subscribe))

(def room
  (nchan/channel
   {:name :room
    :path "/rooms/:id"
    :publisher
    {:mode :http
     :transports [:http :websocket]}
    :subscriber
    {:transports [:websocket :eventsource]
     :first-message {:mode :oldest}}
    :retention
    {:message-timeout "30s"
     :max-messages 128}
    :buffer {:length 128}
    :authorization
    {:publisher #'authorize-publisher
     :subscriber #'authorize-subscriber}}))

(def app
  (h/app
   {:name "rooms"
    :channels [room]}))
```

**Application boundary:** `policy/authorize` and durable room state are
application-owned. Nchan is bounded ephemeral transport, not a durable store.

### Collaborative RTC app

Use the room transport to exchange offers, then turn the worker-local RTC
connection into a Relay:

```clojure
(defn ^:async join-room [room-id exchange-offer]
  (co/await (rooms/authorize! room-id))
  (let [handle (co/await (rtc/open rtc-options))
        offer (co/await (rtc/offer handle))
        answer
        (co/await
         (exchange-offer room-id offer))]
    (co/await
     (rtc/accept-answer handle answer))
    (relay/relay
     (rtc/connect handle)
     room-codec
     {:timeout-ms 2000
      :max-inflight 16})))
```

**Application boundary:** `exchange-offer` is the Nchan signalling adapter;
identity, room state, persistence, and `room-codec` belong to the application.
The RTC handle remains experimental and scoped to its worker.

### Telemetry gateway

A device provider can expose a control Duplex and an event stream. Relay owns
framed control messages while a bounded channel stages telemetry batches:

```clojure
(def session
  (device-provider/open device-id))

(def control
  (relay/relay
   (:duplex session)
   (frame/line)
   {:timeout-ms 2000}))

(def decoded
  (stream/map
   device-codec/decode
   (:events session)))

(def valid
  (stream/filter
   device-codec/valid?
   decoded))

(def storage-inbox
  (async/from-stream
   (stream/partition-all 128 valid)
   4))
```

**Application boundary:** the provider session and device codec are
deployment-specific. The capacity of four batches is an explicit overload
boundary.

### Correlated RPC broker

Correlated Relay mode attaches request IDs and matches responses while allowing
multiple exchanges to be in flight:

```clojure
(def broker
  (relay/relay
   transport
   codec
   {:mode :correlated
    :timeout-ms 5000
    :max-inflight 64
    :prepare-request
    (fn [id request]
      [id (assoc request
                 :request-id id)])
    :response-id
    (fn [response]
      (:request-id response))}))
```

**Application boundary:** `transport` and `codec` come from the connection
provider. Each concrete transport still needs evidence for timeout, closure,
framing, and recovery behavior.

### Job supervisor

Keep durable ownership outside the worker, and use `alts` only to coordinate
one claimed process execution:

```clojure
(defn supervise
  [job process process-output commands shutdown]
  (async/go
   (fn []
     (co/await
      (jobs/claim! (:id job)))
     (loop []
       (let [[value source]
             (co/await
              (async/alts
               [commands process-output shutdown]
               {:priority false}))]
         (cond
           (= source commands)
           (do
             (co/await
              (IStreamWrite/write
               process value))
             (recur))

           (= source process-output)
           (do
             (co/await
              (jobs/append-event!
               (:id job) value))
             (recur))

           :else
           (co/await
            (process-provider/stop!
             process))))))))
```

**Application boundary:** `jobs/...` is the durable queue and result owner;
`process-provider/...` owns process authority. The coroutine is a worker-local
supervisor, not durable scheduling.

### Language tooling

A process or socket can become a correlated JSON-RPC-style Relay for language
server requests:

```clojure
(def lsp
  (relay/relay
   transport
   lsp-codec
   {:mode :correlated
    :timeout-ms 5000
    :max-inflight 32
    :prepare-request
    (fn [id request]
      [id (assoc request
                 "jsonrpc" "2.0"
                 "id" id)])
    :response-id
    (fn [message]
      (get message "id"))}))
```

**Application boundary:** the process/socket provider and `lsp-codec` own
connection setup and JSON-RPC encoding. Relay supplies request correlation,
timeouts, and unsolicited events.

### AI token service

Suspend while the model provider accepts a request, then stream the provider's
token source as SSE:

```clojure
(defn encode-token [token]
  (str "data: " (json/write token) "\n\n"))

(defn ^:async complete [request]
  (co/await
   (quota/consume! request))
  (let [tokens
        (co/await
         (std.native.Host/call
          "model.provider"
          "stream"
          [request]))]
    {:status 200
     :headers
     {"content-type" "text/event-stream"
      "cache-control" "no-cache"}
     :body
     (h/stream
      (stream/map encode-token tokens))}))
```

**Application boundary:** quota policy and the model provider are external to
Hoplite. Streaming responses retain the experimental host contract.

### Media pipeline

Partition decoded frames into bounded batches and write them sequentially to a
provider-owned encoder Duplex:

```clojure
(def frames
  (stream/map
   media-codec/decode
   media-source))

(def work
  (async/from-stream
   (stream/partition-all 8 frames)
   2))

(def encoder
  (media-provider/start
   {:codec "av1"}))

(defn pump-media []
  (async/go
   (fn []
     (loop []
       (when-let [batch
                  (co/await
                   (async/take work))]
         (co/await
          (IStreamWrite/write
           encoder batch))
         (recur))))))
```

**Application boundary:** the media source, codec, and native provider own
buffers and process lifetime. Measure wait time and occupancy at the capacity
of two batches before choosing production limits.

### Notification router

Read claimed records from a durable outbox into a bounded worker-local channel,
send them through a provider, and acknowledge only successful sends:

```clojure
(def deliveries
  (stream/map
   notification/prepare
   (outbox/claimed-stream)))

(def inbox
  (async/from-stream deliveries 16))

(defn route-notifications []
  (async/go
   (fn []
     (loop []
       (when-let [message
                  (co/await
                   (async/take inbox))]
         (co/await
          (std.native.Host/call
           "notification.provider"
           "send"
           [message]))
         (co/await
          (outbox/ack! (:id message)))
         (recur))))))
```

**Application boundary:** the outbox owns durability, retry, and idempotency;
the provider owns delivery. The channel is only bounded local staging.

### Device controller

Authenticate before constructing a worker-local Relay, then coordinate device
commands and shutdown in one session coroutine:

```clojure
(defn start-session
  [request transport commands shutdown]
  (async/go
   (fn []
     (let [device
           (co/await
            (registry/authenticate!
             request))
           control
           (relay/relay
            transport
            device-codec
            {:mode :correlated
             :timeout-ms 2000})]
       (loop []
         (let [[command source]
               (co/await
                (async/alts
                 [commands shutdown]
                 {:priority false}))]
           (when (= source commands)
             (co/await
              (device/exchange!
               control device command))
             (recur))))))))
```

**Application boundary:** authentication, the device registry, codec, and
`device/exchange!` adapter are application-owned. The transport and Relay
handle remain scoped to the worker.

### Remote agent

Model an agent as a coroutine with a bounded command inbox, Relay-backed tools,
an event stream, and explicit persistence:

```clojure
(defn run-agent
  [inbox tool-relay events shutdown]
  (async/go
   (fn []
     (loop [state
            (co/await
             (agent-store/load))]
       (let [[message source]
             (co/await
              (async/alts
               [inbox events shutdown]
               {:priority false}))]
         (cond
           (= source inbox)
           (let [request
                 (co/await
                  (policy/authorize
                   state message))
                 result
                 (co/await
                  (tools/exchange!
                   tool-relay request))
                 next-state
                 (agent/reduce
                  state message result)]
             (co/await
              (agent-store/save!
               next-state))
             (recur next-state))

           (= source events)
           (recur
            (agent/reduce-event
             state message))

           :else state))))))
```

**Application boundary:** policy, tool permissions, isolation, audit, and
persistence remain explicit application concerns. Hoplite and Hara provide the
composition primitives, not a built-in agent product.

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

## Architecture sketch: realtime room

```text
Nchan signalling ──→ peer negotiation
                           ↓
                    worker-local RTC Duplex
                           ↓
                 correlated Relay + events
                           ↓
                   room session coroutine
```

The room coroutine selects peer messages, application events, and shutdown. A
database owns durable room state; neither the RTC handle nor the in-memory event
channel is persisted.

## Architecture sketch: durable job runner

```text
durable queue → claim job → bounded local command channel
                              ↓
                        process Duplex
                              ↓
                     output stream/Relay
                              ↓
                   persist result or retry
```

The durable queue owns delivery and retry. `stream.async` coordinates one
worker-local execution. If the worker disappears, the queue—not the channel—
makes the job available again.

## Architecture sketch: telemetry gateway

```clojure
(def decoded (stream/map decode-packet packets))
(def valid (stream/filter valid-packet? decoded))
(def batches (stream/partition-all 128 valid))
(def storage-inbox (async/from-stream batches 4))
```

At most four completed batches are buffered at this application boundary. The
storage consumer can report wait time and occupancy, making sustained ingestion
overload observable.

Next: [Performance by construction](../performance/).
