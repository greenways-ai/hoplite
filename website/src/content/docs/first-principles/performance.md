---
title: 9. Performance by construction
description: Connect Hoplite and Hara stream design mechanisms to measurable latency, allocation, throughput, and memory behavior.
---

Architecture can make good performance possible, but only measurement shows how
a particular build behaves on a particular workload. This chapter separates the
mechanism, its likely benefit, its cost, and the evidence needed to evaluate it.

## Mechanisms and measurements

| Mechanism | Expected benefit | Cost or risk | Measure |
| --- | --- | --- | --- |
| Prepared handler dispatch | Avoid per-request source parsing and resolution | Startup work and retained prepared calls | Startup phases, request latency, allocations |
| Direct synchronous path | Avoid Promise/work allocation for immediate handlers | Two execution paths to maintain | Sync allocations and latency |
| Coroutine suspension | Many waiting operations without a thread per request | Continuation and Promise retention | Suspended latency, active-work memory |
| Worker-local values | Avoid ordinary cross-worker locking | Explicit external coordination required | Scaling by worker count and marginal RSS |
| Pull-based streams | Work follows downstream demand | Per-item Promise/callback overhead | Throughput, allocations, slow-consumer memory |
| Bounded channels | Predictable burst memory | Producers may wait or reject | Peak memory, wait time, rejected offers |
| Partitioning/batching | Amortize calls, framing, and encoding | Added latency and retained batches | Throughput/latency curve by batch size |
| Relay correlation | Multiplex one connection | Pending-map and codec overhead | In-flight throughput and tail latency |
| Native Nginx transport | Reuse mature event and network machinery | FFI and ownership boundary complexity | End-to-end HTTP/RTC profiles |

These are hypotheses about causality until a report measures them.

## Backpressure protects the memory curve

For a bounded channel with capacity `C` and maximum retained value size `V`, the
channel's payload contribution is bounded approximately by `C × V`, plus queue
and pending-operation overhead. This is not a claim about total process memory;
it is a reason the contribution can be budgeted.

Test the whole chain with a deliberately slow consumer. Record peak worker RSS,
producer wait duration, channel capacity, chunk size, total bytes, completion,
and cancellation cleanup. A fast-loop benchmark does not test backpressure.

## Concurrency is not free parallelism

Coroutines improve utilization when work spends time waiting. They do not make
CPU-bound transformations run in parallel inside one worker. CPU-heavy encoding,
compression, inference, or media work may require batching, native providers,
additional workers, or an external process.

Adding a channel to a CPU-bound pipeline can increase allocations and latency
without improving throughput. Measure before and after the concurrency boundary.

## Buffer-size experiments

Benchmark several capacities, including zero/unbuffered behavior where
supported, a small burst buffer, and a deliberately oversized buffer. Report:

- median and tail latency;
- throughput;
- peak and steady-state RSS;
- allocation counts or bytes where available;
- producer wait time and failed immediate offers;
- shutdown time with a full buffer.

The useful capacity is where expected bursts are absorbed without hiding
sustained overload or violating latency and memory budgets.

## Relay experiments

Measure raw Duplex writes, serial Relay, and correlated Relay separately. Vary
frame size, codec, concurrent in-flight exchanges, timeout rate, and unsolicited
event volume. Include failure cases because cleanup of a large pending map is
part of the service's tail behavior.

## Reproducible Hoplite evidence

Use the existing `hoplite.runtime-measurement/0-alpha` schema. A useful report
records Hoplite and Hara revisions and dirty state, fixture identity, one- and
four-worker configurations, warmups, every raw sample, tool versions, operating
system, kernel, architecture, CPU, and memory.

Required groups cover startup, artifact sizes, worker memory, synchronous and
suspended requests, allocations, and slow-client streaming. Compare only
machine-compatible reports and publish numeric deltas rather than converting
noisy hardware measurements into unexplained universal claims.

## Performance maintenance

Keep deterministic correctness gates in ordinary CI. Run real timing and memory
collection on controlled machines or scheduled release jobs. Retain raw samples
so a future regression can be investigated instead of merely announced.

## Worked example: compare channel capacities

Run the same producer and consumer with capacities `1`, `8`, `64`, and `1024`.
For each run, retain a record such as:

```clojure
{:capacity 64
 :values 100000
 :value-bytes 256
 :producer-wait-ns [...]
 :latency-ns [...]
 :peak-worker-rss-bytes 0
 :failed-offers 0}
```

The zero shown here is a placeholder field, not a result. Populate it from the
measurement tool. Compare the latency distribution and peak memory rather than
selecting the capacity with the largest isolated throughput.

## Worked example: estimate a buffer budget

If a channel holds 64 values whose retained representation is at most 4 KiB,
its payload budget is approximately:

```text
64 × 4096 bytes = 262144 bytes (256 KiB)
```

Queue nodes, Promises, referenced objects, codecs, and allocator behavior add
overhead. Measure worker RSS to validate the complete system; use the estimate to
catch obviously impossible configurations during design.

## Worked example: distinguish waiting from CPU work

```clojure
;; Good suspension candidate: completion depends on external readiness.
(co/await (Host/call "nginx" "sleep" [10]))

;; Still CPU work: wrapping it in go does not create parallel execution.
(async/go (fn [] (encode-large-value value)))
```

Use workers, native providers, batching, or an external process for CPU-heavy
work. Use coroutines to release the event loop while waiting.

Next: [Maintainability by construction](../maintainability/).
