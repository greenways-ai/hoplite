# Project direction

## Mission

Hoplite should be the best possible Hara HTTP runtime and application library
for Nginx: small enough to understand, strict enough to embed safely, pleasant
enough for everyday application work, and fast enough that the abstraction is
not the bottleneck.

The project is library-first. A feature belongs in Hoplite when it improves the
application model, runtime, transport, embedding boundary, developer experience,
or evidence for those surfaces.

## What Hoplite owns

Hoplite owns:

- immutable application and route composition;
- handler resolution, preparation, and worker-local dispatch;
- Nginx module integration and production worker lifecycle;
- bounded request-body ownership;
- response maps, streaming response sources, ranges, `HEAD`, and backpressure;
- synchronous and suspended handler execution;
- deterministic application build and bytecode startup;
- public native data-plane and host-provider interfaces;
- the development/control CLI and slim production launcher;
- errors, diagnostics, tracing, tests, documentation, examples, benchmarks, and
  release compatibility for the surfaces above.

These are not merely implementation details. They are the library product.

## What Hoplite does not own

The core project does not own:

- account, identity, session, authorization, or replay semantics for an
  application;
- a database, object store, canonical-value store, or credential product;
- publishing and coordinating a marketplace of provider artifacts;
- downstream application schemas or business rules;
- another repository's integration, release, or acceptance gate;
- workflows whose purpose is to patch branches, publish extension packages, or
  synchronize unrelated repositories after every main-branch push.

A reference extension may live in the repository while its interface is being
proved. It must remain optional, must not widen the core application contract,
and must not determine whether normal Hoplite changes can merge. Mature product
implementations should move to their own package or repository.

## Architectural boundary

```text
Hara application
      |
      v
hoplite.core application API
      |
      v
validated build + deterministic bundle
      |
      v
worker-local Hoplite runtime
      |
      v
Nginx request/response transport

optional host service
      ^
      |
stable Hoplite provider ABI
```

The application chooses semantics. Hoplite supplies execution and transport.
An extension supplies an implementation behind an explicit interface. Trusted
deployment configuration chooses extensions; ordinary request values do not.

## Quality bar

### API clarity

- Public modules and ABIs are explicitly listed and documented.
- Public values have closed shapes, bounded fields, and stable error classes.
- Internal modules are not accidentally promoted to API by examples or CI.
- A breaking change carries a migration note and a version decision.

### Correctness and safety

- Bounds, ownership, cleanup, cancellation, and backpressure are tested at the
  exact boundary where they matter.
- Invalid configuration fails before a worker accepts requests.
- Bundle validation completes before application code mutates worker state.
- Native handles are opaque, scoped, kind-checked, and closed at most once.
- Tests prefer behavioural evidence over source-text assertions.

### Embeddability

- The runtime has a small, versioned host surface.
- The production serving plane excludes development-only authority.
- Host services are replaceable without changing application semantics.
- Errors do not expose build-machine paths, credentials, or native pointers.

### Developer experience

- A useful service starts from a small, readable project.
- Check, build, run, test, inspect, and diagnose commands are predictable.
- Failure output identifies the violated contract and a practical next step.
- Examples exercise supported public APIs rather than internal shortcuts.

### Performance

- Handlers are prepared once and dispatched without per-request source parsing.
- The synchronous path remains allocation-conscious.
- Streaming preserves bounded memory under slow clients.
- Startup, throughput, tail latency, executable size, image size, and worker RSS
  are measured with reproducible metadata.
- Benchmarks inform decisions but do not make noisy hardware results block every
  pull request.

### Release discipline

- The committed dependency lock is reproducible and always tested with
  `--locked`.
- Normal CI never publishes releases or edits branches.
- Releases are tag-driven and validate the same library surfaces as CI.
- Benchmark evidence is retained separately from required merge gates.

### Versioning during alpha

- Distribution artifacts use ordinary pre-1.0 semantic versions such as
  `0.2.x`.
- Evolving portable contracts use explicit alpha identities: Hara `HBC0` and
  `HBX0`, and Hoplite `hoplite.application-bundle/0-alpha` / `HAB0`.
- Native runtime ABI values and suffixes such as `_v1` describe binary call
  shapes and are independent of portable-document maturity.
- No evolving portable surface is promoted to a stable-looking major version
  until that individual contract is deliberately frozen.
- An incompatible alpha change updates code, fixtures, documentation, and a
  migration note together.

The complete rule is documented in
[Versioning during alpha](versioning.md).

## Alpha roadmap

### 1. Restore a trustworthy green baseline

- keep one current Hara pin and one committed lock;
- reduce required CI to library, integration, and documentation evidence;
- remove duplicate, generated, provider-publication, and branch-patching
  workflows from the default branch;
- make every required failure actionable.

### 2. Stabilize the public application and embedding APIs

- inventory public HAL modules, route adapters, native headers, and CLI commands;
- document lifecycle, ownership, error, compatibility, and threading rules;
- add contract fixtures for each published surface;
- establish deprecation and migration rules for the pre-1.0 period.

### 3. Separate the core from historical product implementations — complete in 0.2.0

- remove legacy management, signed-application policy, and storage products from
  the default crate and production image;
- retain only the generic host/provider boundary in core;
- retire historical implementations without a current owner;
- replace negative source greps with positive composition and binary evidence.

### 4. Make operation and diagnosis first-class

- maintain structured startup and request diagnostics;
- expose inspect/verify/doctor commands for generated bundles and configuration;
- provide bounded tracing hooks without forcing one telemetry backend;
- improve cancellation, timeout, disconnect, and partial-startup fixtures.

### 5. Make performance a maintained property

- keep cached dispatch and deterministic bytecode boot on the production path;
- publish reproducible benchmark and footprint reports;
- set regression budgets only for stable, low-noise measurements;
- profile allocations and worker memory as carefully as raw throughput.

## Decision rule for new work

Before adding code, a workflow, or a release gate, answer:

1. Which published Hoplite surface does this protect or improve?
2. Can the behaviour be demonstrated inside this repository with a generic
   application or embedding fixture?
3. Is the check deterministic enough to block unrelated pull requests?
4. Would the feature still belong if every downstream Greenways application
   disappeared?

If the fourth answer is no, the work belongs in an extension or downstream
project rather than Hoplite core.
