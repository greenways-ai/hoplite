---
title: Durable work and live streams
description: Combine replayable work lifecycles with ephemeral stream processing without confusing their ownership models.
---

Hoplite requests are short-lived, but the operations they begin do not always
fit inside a request. An import may take minutes. A deployment may survive a
worker restart. A payment attempt needs an audit trail even when nobody is
watching it.

`work.base` and `std.stream.async` solve different halves of that problem:

- work records durable identity, status, checkpoints, results, errors, and
  committed lifecycle events;
- streams move live values with backpressure, cancellation, and explicit
  ownership;
- `work/events` creates the bridge without making a durable run itself behave
  like a consumable stream.

This distinction is the central rule: **work owns truth; streams own flow**.

## Submit a live run

Define reusable work as data, create a host, and submit it:

```clojure
(require '[work.base :as work])

(def import-users
  (work/chain
   {:id :users/import}
   [(work/step :decode
      (fn [upload context]
        (decode-csv upload)))
    (work/each
     (work/step :persist-user
       (fn [user context]
         (save-user! user))))]))

(def host (work/durable-runtime {:store operations-store}))
(def run  (work/submit host import-users upload
                       {:id "import-2026-08-17"}))
```

`submit` returns an `IWorkRun` immediately after admission. It does not wait for
the graph to finish. The handle has four independent views:

```clojure
(IWorkRef/work-id run)       ;; durable identity
(IWorkRun/work-status run)   ;; direct live projection
(IWorkRun/work-result run)   ;; one stable Promise
(IWorkRun/work-events run {}) ;; a new IStream cursor
```

Use `work/run` when only the final value matters:

```clojure
(def imported
  (co/await (work/run host import-users upload)))
```

The old `work/start` entry point remains a compatibility operation for code
that expects a completed reference. New application code should choose
`submit` or `run` explicitly.

## Observe without controlling

Every call to `work/events` creates an independent cursor over committed
events. A dashboard and a log exporter can therefore observe the same run at
different speeds without sharing consumption state.

```clojure
(require '[std.stream.async :as async])
(require '[std.stream.common :as stream])

(def dashboard-events
  (async/from-stream
   (work/events run {:after last-rendered-sequence
                     :follow true})
   32))

(async/go
 (fn []
   (loop []
     (when-let [event (co/await (async/take dashboard-events))]
       (render-progress! event)
       (recur)))))
```

`:after` is exclusive. If the saved sequence is 41, the first possible event
is 42. With `:follow true` (the default), the stream waits for future committed
events and closes after terminal cleanup. With `:follow false`, it reads the
available snapshot and ends.

The store is read before an event enters the live stream. A disconnected
browser, full channel, or slow renderer cannot make the durable execution wait.

## Resume a Server-Sent Events response

An HTTP client can reconnect using the last event ID:

```clojure
(defn event-response [run last-event-id]
  (let [source (work/events run
                            {:after (or (parse-long last-event-id) 0)
                             :follow true})]
    {:status 200
     :headers {"content-type" "text/event-stream"
               "cache-control" "no-cache"}
     :body (stream/map encode-sse source)}))
```

Replay and live delivery use one cursor, so the handler has no race between
"load history" and "subscribe now". The response owns its stream and closes it
when the client disconnects; closing the observer does not cancel the work.

## Feed a Relay session

Lifecycle events can also cross a process, socket, or RTC Relay:

```clojure
(defn forward-run [run relay]
  (async/go
   (fn []
     (let [events (work/events run {:follow true})]
       (try
         (loop []
           (when-let [event (co/await (IStream/next events))]
             (co/await
              (IStreamWrite/write
               relay
               {:type :work/event
                :run (IWorkRef/work-id run)
                :event event}))
             (recur)))
         (finally
           (IClose/close events)))))))
```

Relay owns framing and transport backpressure. The work host owns execution and
replay. The forwarding coroutine owns only the temporary connection between
them.

## Cancellation is a lifecycle operation

Closing an event stream means "this observer is finished." Cancelling a run
means "the durable operation should stop." Keep those actions separate:

```clojure
(IClose/close dashboard-events)       ;; detach one observer
(co/await (IWorkRun/work-cancel run   ;; request lifecycle cancellation
                                {:reason :operator-request}))
```

The cancellation Promise resolves after the host records the transition and
refreshes the live handle. Work implementations should check cancellation at
await boundaries, node boundaries, and explicit safe points. Cleanup belongs
in `work/ensure`:

```clojure
(def deploy
  (work/ensure
   (work/step :release deploy-release!)
   (work/step :cleanup
     (fn [outcome context]
       (release-lock! (:input outcome))))))
```

## Parallelism belongs to the host

`work/all`, `work/each`, and `work/batch` describe finite durable structure.
They do not smuggle a scheduler into the graph. The host chooses concurrency,
queue limits, fairness, and deterministic result ordering:

```clojure
(def thumbnails
  (work/each
   {:id :media/thumbnails}
   (work/step :thumbnail render-thumbnail!)))

(def development-host
  (work/local-runtime {:parallelism 1}))

(def production-host
  (work/durable-runtime {:store store
                         :parallelism 8
                         :queue-capacity 128}))
```

The same graph remains replayable in both environments. Stream operators such
as `interleave` and `partition` are appropriate after values become live and
ephemeral; they are not substitutes for durable graph nodes.

## Structured child work

A step may need to start related work. Attached children should be the default:

```clojure
(work/step
 :fan-out
 (fn [accounts context]
   (mapv (fn [account]
           ((:work/spawn! context)
            reconcile-account
            account
            {:attached true}))
         accounts)))
```

Attached children inherit cancellation and must settle before their parent is
fully cleaned up. A detached child is a durable ownership transfer, not a
background convenience; record it explicitly so operators can find its new
owner.

## Persistence boundary

Persist descriptions and values, never live machinery. Channels, streams,
Promises, coroutines, sockets, and native handles have process-local ownership
and cannot be replayed:

```clojure
;; durable input
{:bucket "uploads" :key "users/17.csv"}

;; not durable input
{:body request-stream :completion pending-promise}
```

Store a locator and reopen the resource inside a step. This keeps retries
portable and makes serialization failures happen at admission rather than
during recovery.

## Performance consequences

This composition improves performance through mechanisms that can be measured:

- immediate handles avoid holding an HTTP request open until completion;
- cursor reads avoid rebuilding an observer-specific event history in memory;
- independent consumers eliminate global fan-out locks;
- bounded channels cap live memory while durable storage absorbs long gaps;
- host-owned concurrency prevents every graph from inventing queues and
  executors;
- direct `work-status` reads avoid a store round trip for UI polling.

Benchmark admission latency, committed events per second, cursor catch-up
latency, allocations per event, and cancellation-to-cleanup time. Do not infer
an improvement merely from the presence of coroutines.

## Maintainability consequences

Failures remain local because each layer has one responsibility:

| Symptom | Inspect first |
| --- | --- |
| Missing history | work store and transition commit |
| Slow observer | stream buffer and consumer |
| Duplicate effect | work checkpoint or receipt idempotency |
| Stuck transport | Relay framing and duplex ownership |
| Excess concurrency | host policy and queue limits |
| Leaked resource | `work/ensure` and attached-child cleanup |

A run can be replayed without reconstructing its viewers. A viewer can restart
from a sequence without rerunning the work. A transport can reconnect without
becoming the source of truth. Those separations reduce the number of components
that must be understood during an incident.

Next, [Duplex and Relay](../duplex-relay/) turns bidirectional transports into
portable application protocols.