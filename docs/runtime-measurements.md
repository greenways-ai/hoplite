# Runtime measurements and comparisons

`hoplite.runtime-measurement/0-alpha` is the non-blocking release evidence for
startup, footprint, memory, allocation, latency, and backpressure. Every report
records complete Hoplite and Hara revisions plus dirty state, the fixture and
request identity, worker counts, warmups, every raw sample, tool versions, and
the machine environment (OS, kernel, architecture, CPU model/count, and total
memory). Units are encoded in field names.

Required raw sample groups are:

- `startupNs`: configuration, bundle validation, modules, routes, and total;
- `sizesBytes`: `hoplite-server`, image, HAB0, manifest, and configuration;
- `memoryBytes`: one worker, four workers, and marginal worker;
- `requests`: synchronous and suspended latency, synchronous allocations, and
  slow-client streaming peak memory.

`hoplite.runtime-comparison/0-alpha` is produced only after both inputs validate.
It reports median numeric deltas, never a pass/fail budget. Machine compatibility
is explicit: OS, kernel, architecture, CPU, logical CPU count, and total memory
must match. An incompatible comparison still reports why it is incompatible but
does not claim a performance regression.

Validate or compare reports with:

```sh
node packaging/scripts/validate-runtime-measurement.mjs report.json
node packaging/scripts/validate-runtime-measurement.mjs baseline.json candidate.json
node packaging/scripts/validate-runtime-measurement.mjs --self-test
```

Real timing and memory collection remains scheduled, manual, and release-tag
evidence. Deterministic schema and comparison self-tests run in library CI.
