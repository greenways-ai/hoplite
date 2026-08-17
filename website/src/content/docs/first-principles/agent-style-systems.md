---
title: Agent-style systems
description: Compose durable command admission, sequential agent state, model calls, Relay-backed tools, receipts, and live events without confusing worker-local concurrency with security or durability.
---

An agent-style system is a useful Hoplite application shape, but it is not a
special runtime category. The center can remain an ordinary Hara state
transition owned by one coroutine. Hoplite supplies request handling and host
suspension; standard Hara supplies streams, channels, Duplex values, and Relay.
The application still owns identity, policy, model selection, tool authority,
isolation, persistence, retry, and audit.

:::note[Architecture, not a bundled agent product]
The examples on this page combine current Hara and Hoplite composition
primitives with deliberately named application interfaces such as
`agent-store/...`, `policy/...`, `model/...`, and `tools/...`. Those interfaces
must be implemented by the application or an installed provider.
:::

## Start with one sequential state owner

The smallest useful agent is one state value and one function that applies an
event to that value:

```clojure
(defn apply-event [state event]
  (case (:kind event)
    :command/accepted
    (assoc state
           :active-command (:command-id event)
           :status :running)

    :model/completed
    (assoc state :last-model-result (:result event))

    :tool/completed
    (update state :tool-results
            assoc
            (:call-id event)
            (:result event))

    :command/completed
    (assoc state
           :active-command nil
           :status :idle
           :revision (:revision event))

    :command/failed
    (assoc state
           :active-command nil
           :status :idle
           :last-error (:error event))

    state))
```

The reducer does not open sockets, call a model, authorize tools, or write to a
database. That separation matters because persisted events can rebuild the same
state after a worker restart, and tests can exercise the state machine without
starting Nginx.

## Reference composition

```text
HTTP, Nchan, or Relay command ingress
                    │
          authenticate and authorize
                    │
          durable command admission
                    │
       bounded worker-local wake-up inbox
                    │
             session coroutine
       ┌────────────┼───────────────┐
       │            │               │
 model provider  tool Relay     timers/cancel
   Promise        exchanges        ports
       │            │               │
       └────────────┼───────────────┘
                    │
       events + checkpoint + receipt
                    │
          durable application store
                    │
        SSE/Nchan observation surface
```

The durable store is the authority. The inbox is only a bounded scheduling
hint. A live Relay, channel, Promise, process, socket, or RTC handle belongs to
the worker that created it and is never serialized into agent state.

## Assign one owner to every concern

| Concern | Composition | Durable owner |
| --- | --- | --- |
| Command admission | HTTP handler plus provider Promise | Command journal |
| Sequential state | One session coroutine | Event log or checkpoint store |
| Model invocation | Async host provider | Model provider and quota ledger |
| Tool exchange | Correlated Relay over a Duplex | Tool policy and receipt store |
| External side effects | Transactional outbox | Outbox and idempotent provider |
| Live progress | Stream projection, SSE, or Nchan | Event log, when replay is required |
| Cancellation | Durable cancellation request plus local signal | Command journal |
| Isolation | Separate process, service, or sandbox provider | Deployment policy |

A channel owns none of the durable rows. It coordinates activities that already
have an authoritative owner.

## Use closed command, event, and receipt envelopes

Stable envelopes make idempotency, causation, and replay inspectable. A command
can carry a client-selected idempotency key and an expected state revision:

```clojure
{:protocol "agent.command/0-alpha"
 :agent-id "agent-17"
 :command-id "cmd-018f"
 :idempotency-key "workspace-42:turn-9"
 :expected-revision 41
 :kind :turn
 :input {:role :user
         :content "Summarize the failed checks"}}
```

An event records one accepted transition:

```clojure
{:protocol "agent.event/0-alpha"
 :agent-id "agent-17"
 :event-id "evt-01a2"
 :sequence 42
 :causation-id "cmd-018f"
 :kind :tool/completed
 :call-id "call-7"
 :result {:status :ok
          :receipt-id "tool-receipt-77"}}
```

A completion receipt identifies the durable outcome rather than merely
reporting that a coroutine returned:

```clojure
{:protocol "agent.receipt/0-alpha"
 :agent-id "agent-17"
 :command-id "cmd-018f"
 :status :accepted
 :revision 44
 :result-digest "sha256:..."
 :effect-receipts ["tool-receipt-77"]}
```

Unknown fields, oversized payloads, stale revisions, and reused identifiers with
different content should be rejected at admission. Repeating the same
idempotency key with the same content should return the original receipt.

## Admit durably before waking a worker

The HTTP handler should not place the only copy of a command into an in-memory
channel. Persist first, then use a bounded channel to wake a local supervisor:

```clojure
(def wakeups (async/chan 64))

(defn ^:async submit-command [request]
  (let [identity
        (co/await
         (identity/authenticate request))
        command
        (command/decode request)
        admitted
        (co/await
         (agent-store/admit-command!
          identity command))]
    ;; A full wake-up channel does not lose the command. The durable poller can
    ;; discover it later, so this immediate notification may be coalesced.
    (when (= :new (:status admitted))
      (async/offer wakeups (:agent-id command)))
    {:status (if (= :new (:status admitted)) 202 200)
     :headers {"content-type" "application/json; charset=utf-8"}
     :body (json/write (:receipt admitted))}))
```

`agent-store/admit-command!` is responsible for authentication context,
idempotency, revision checks, size limits, and durable ordering. `wakeups` only
reduces latency.

## Claim commands with a lease

A worker should claim one command through the durable owner before execution.
The claim includes a generation or lease token so a stale worker cannot commit
a result after another worker has recovered the command:

```clojure
(defn ^:async claim-next [agent-id worker-id]
  (agent-store/claim-next!
   {:agent-id agent-id
    :worker-id worker-id
    :lease-ms 30000}))
```

Every checkpoint, event append, receipt, and acknowledgement includes the claim
generation. If the worker disappears, the lease expires and another worker can
resume from the last committed boundary. This is application durability; the
channel and coroutine remain worker-local.

## Run one worker-local session coroutine

The session owns the current state value and selects among command wake-ups,
tool events, maintenance timers, cancellation, and shutdown:

```clojure
(defn run-session
  [{:keys [agent-id worker-id
           commands tool-events maintenance
           cancellations shutdown tools]}]
  (async/go
   (fn []
     (loop [state
            (co/await
             (agent-store/load-state agent-id))]
       (let [[message source]
             (co/await
              (async/alts
               [commands
                tool-events
                maintenance
                cancellations
                shutdown]
               {:priority false}))]
         (cond
           (= source commands)
           (recur
            (co/await
             (execute-next-command
              agent-id worker-id tools state)))

           (= source tool-events)
           (let [event
                 (co/await
                  (agent-store/append-event!
                   agent-id message))]
             (recur (apply-event state event)))

           (= source maintenance)
           (do
             (co/await
              (agent-store/checkpoint!
               agent-id state))
             (recur state))

           (= source cancellations)
           (recur
            (co/await
             (cancel-active-command
              agent-id state message)))

           :else
           (do
             (co/await
              (agent-store/checkpoint!
               agent-id state))
             state)))))))
```

Only this coroutine mutates the in-memory state value. Other activities send
messages or complete Promises. The durable store remains authoritative for
recovery and cross-worker visibility.

## Execute one turn as explicit boundaries

A turn commonly contains four boundaries: authorize the command, call the
model, execute approved tools, then atomically commit events and a receipt.
Keep those phases visible:

```clojure
(defn ^:async execute-next-command
  [agent-id worker-id tools state]
  (let [claim
        (co/await
         (claim-next agent-id worker-id))]
    (if (nil? claim)
      state
      (let [command (:command claim)
            grant
            (co/await
             (policy/authorize-command
              state command))
            plan
            (co/await
             (model/plan-turn
              state command grant))
            outcome
            (co/await
             (execute-plan
              tools state command grant plan))
            committed
            (co/await
             (agent-store/commit-command!
              {:claim claim
               :events (:events outcome)
               :checkpoint (:state outcome)
               :receipt (:receipt outcome)}))]
        (:state committed)))))
```

`commit-command!` should reject a stale claim generation. Where the backing
store supports transactions, append the final events, update the checkpoint,
write the receipt, and acknowledge the command in one transaction.

## Treat model access as a provider effect

The model request is a value. The provider owns network credentials, connection
pooling, quotas, cancellation, and model-specific protocol details:

```clojure
(defn ^:async complete-model [request]
  (let [response
        (co/await
         (std.native.Host/call
          "model.provider"
          "complete"
          [request]))]
    (model/validate-response response)))
```

Do not place provider credentials or an unrestricted HTTP client in the command
envelope. Select a named provider operation and validate its returned value
before it enters agent state.

When a provider returns a token stream, partial tokens are observations. The
canonical state transition should reference the validated final response or a
durable provider receipt. A disconnected browser must not decide whether the
turn committed.

## Narrow every tool call before Relay

Relay can manage framing, correlation, concurrent in-flight requests, timeouts,
and unsolicited events. It does not decide which tool an agent may invoke.
Construct a Relay over a provider-owned Duplex, then place a policy adapter in
front of it:

```clojure
(def tool-relay
  (relay/relay
   tool-transport
   tool-codec
   {:mode :correlated
    :timeout-ms 5000
    :max-inflight 8
    :prepare-request
    (fn [id request]
      [id (assoc request :request-id id)])
    :response-id
    (fn [response]
      (:request-id response))}))

(defn ^:async invoke-tool
  [state command grant call]
  (let [authorized
        (co/await
         (policy/authorize-tool
          state command grant call))
        bounded-request
        (tools/project-request
         authorized call)
        result
        (co/await
         (tools/exchange!
          tool-relay bounded-request))]
    (tools/validate-result authorized result)))
```

`tools/exchange!`, `tools/project-request`, and the policy functions are
application adapters. They should enforce the exact tool name, argument schema,
resource scope, deadline, output limit, and redaction policy granted for this
call.

For high-authority tools, the Duplex should terminate in a separate process,
service, or sandbox. A namespace boundary inside the application runtime is not
a security boundary.

## Record external effects through an outbox

A successful Relay response does not by itself provide exactly-once effects.
For email, repository mutation, payment, or another externally visible action,
first persist an effect intent with a stable idempotency key:

```clojure
(defn ^:async deliver-effect [effect]
  (let [claim
        (co/await
         (outbox/claim! effect))]
    (when claim
      (let [provider-receipt
            (co/await
             (std.native.Host/call
              (:provider effect)
              "deliver"
              [(:request effect)
               {:idempotency-key
                (:idempotency-key effect)}]))]
        (co/await
         (outbox/complete!
          claim provider-receipt))))))
```

The agent receipt references the durable provider receipt. Retry belongs to the
outbox and provider contract, not to an unbounded loop inside the session
coroutine.

## Stream observations without making them authority

A client can observe the durable event log through SSE. The event store supplies
catch-up and ordering; `h/stream` supplies the experimental streaming response
boundary:

```clojure
(defn encode-agent-event [event]
  (str "id: " (:sequence event) "\n"
       "event: " (name (:kind event)) "\n"
       "data: " (json/write event) "\n\n"))

(defn agent-events [request]
  (let [agent-id
        (routing/agent-id (:path request))
        after
        (headers/last-event-id request)
        source
        (agent-store/event-stream
         agent-id {:after after})]
    {:status 200
     :headers {"content-type" "text/event-stream"
               "cache-control" "no-cache"}
     :body
     (h/stream
      (stream/map
       encode-agent-event source))}))
```

Nchan can provide bounded low-latency fan-out, but reconnecting clients should
recover from the durable event log when no event may be lost. Never infer
command completion solely from whether an ephemeral subscriber saw the last
message.

## Make cancellation a durable request

Cancellation has two paths:

1. Persist the cancellation request against the command.
2. Signal the active worker so it can attempt provider cancellation promptly.

```clojure
(defn ^:async cancel-command [request]
  (let [identity
        (co/await
         (identity/authenticate request))
        target
        (command/decode-cancellation request)
        cancellation
        (co/await
         (agent-store/request-cancellation!
          identity target))]
    (async/offer cancellations cancellation)
    {:status 202
     :body (json/write cancellation)}))
```

Provider cancellation is best effort unless the provider contract says
otherwise. If a tool process ignores cancellation, the isolation provider may
terminate that process. The durable command record states whether cancellation
was requested, observed, completed, or overtaken by an already committed result.

## Expose a small Hoplite surface

The HTTP boundary can remain narrow even when the internal composition is rich:

```clojure
(def app
  (h/app
   {:name "agent-service"
    :resources
    [["/agents/:id"
      ["/commands"
       {:post {:name "submit-agent-command"
               :handler #'submit-command}}]
      ["/events"
       {:get {:name "stream-agent-events"
              :handler #'agent-events}}]
      ["/cancel"
       {:post {:name "cancel-agent-command"
               :handler #'cancel-command}}]
      ["/status"
       {:get {:name "agent-status"
              :handler #'agent-status}}]]]}))
```

Path parameters are not currently projected as a `:path-params` map by the
Hoplite Nginx boundary. The application can parse the validated route shape from
`:path`, or use a routing adapter that supplies a closed command value.

## Scale to several agents without sharing mutable state

Use one logical session owner per active agent. An orchestrator sends durable
commands to those sessions rather than reaching into their state:

```text
                  orchestrator
              ┌───────┼───────┐
              │       │       │
          command A command B command C
              │       │       │
          session A session B session C
              │       │       │
          receipts  receipts  receipts
              └───────┼───────┘
                  durable store
```

Cross-agent messages use the same command envelope, causation identifiers, and
receipts as external commands. Do not persist a channel reference from one agent
inside another agent's state. After recovery, the orchestrator resolves the
target agent by durable identifier and submits a new command.

A workflow that must survive process loss should be owned by a durable work
system. Hoplite can serve its control API and coordinate one local execution,
but `async/go` is not durable workflow replay.

## Choose explicit overload behavior

| Boundary | Bounded mechanism | Overload decision |
| --- | --- | --- |
| HTTP admission | Per-identity quota and durable journal limits | Reject with `429` or `503` |
| Worker wake-ups | Small channel of agent identifiers | Coalesce; durable commands remain discoverable |
| Active commands | One claim per agent, or a declared concurrency limit | Queue durably or reject |
| Tool Relay | `:max-inflight` and request deadline | Delay, reject, or cancel |
| Model provider | Provider quota and deadline | Reject or defer durably |
| Event fan-out | Bounded subscriber buffer | Disconnect and replay from durable sequence |
| Token streaming | Pull-based stream and client cancellation | Stop upstream generation where supported |

An unbounded mailbox hides the policy until memory exhaustion. A bounded
mailbox makes the decision observable and testable.

## Keep the security model outside concurrency primitives

A production design should make these checks visible:

- Authenticate before durable command admission.
- Authorize every command against the current identity and agent policy.
- Authorize every tool call again with a narrow, expiring capability.
- Keep model and tool credentials inside providers, not Hara values from a
  request.
- Run high-authority or untrusted tools in a separate process, service, or
  sandbox with resource limits.
- Redact secrets before writing events, receipts, logs, or model context.
- Bound command size, model context, tool output, event payloads, in-flight
  exchanges, and wall-clock duration.
- Persist identifiers and receipts, never live handles, channels, Relays,
  sockets, processes, or RTC sessions.
- Treat a worker crash, timeout, decode failure, and cancellation failure as
  explicit state transitions with stable error classes.

The sequential session loop provides understandable state ownership. It does
not grant authority and it does not isolate code.

## Test the center without Hoplite

The pure reducer can be verified directly:

```clojure
(Test/run
 [{:name "completed commands advance the revision"
   :test (fn []
           (apply-event
            {:status :running
             :active-command "cmd-1"
             :revision 8}
            {:kind :command/completed
             :command-id "cmd-1"
             :revision 9}))
   :expected
   {:status :idle
    :active-command nil
    :revision 9}}])
```

Then test the asynchronous shell with in-memory channels and fake provider
adapters. Important invariants include:

- replaying the same ordered events produces the same state;
- reusing an idempotency key does not repeat a tool effect;
- a full wake-up channel cannot lose an admitted command;
- a stale claim generation cannot commit;
- a worker restart resumes from the last committed checkpoint;
- cancellation settles or terminates every owned provider operation;
- event subscribers can reconnect from a durable sequence;
- serialization rejects every live handle and capability-bearing runtime value.

Hoplite is needed for the HTTP, Nchan, RTC, and host-provider edges. The state
machine, policy decisions, receipt construction, and recovery rules should
remain testable as ordinary Hara code.

## What the stack supplies

| Layer | Supplies | Does not supply |
| --- | --- | --- |
| Hoplite | HTTP routing, response maps, host suspension, optional streaming/Nchan/RTC boundaries | Agent policy, models, tools, persistence, isolation |
| Standard Hara | Values, functions, protocols, Promises, streams, channels, Duplex, Relay | Durable workflow replay or a security boundary |
| Application/providers | Identity, command journal, checkpoints, quotas, model access, tool registry, sandbox, outbox, receipts | Implicit authority from a request or channel |

The useful design is deliberately unsurprising: durable owners remember,
providers perform effects, one coroutine owns local state, channels coordinate
bounded activity, Relay owns protocol mechanics, and Hoplite exposes a small
application boundary.

Next: [Performance by construction](../performance/).
