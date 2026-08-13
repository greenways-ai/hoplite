---
title: Diagnosing an application
description: Verify, inspect, and diagnose generated Hoplite applications without accidentally executing source.
---

Hoplite separates byte validation, generated-output inspection, and local
environment diagnosis so an operator can choose exactly how much authority a
check receives.

## Verify immutable application bytes

```sh
hoplite verify PROJECT
hoplite verify --manifest FILE BUNDLE
```

`verify` reads the bounded `hoplite.application-bundle/0-alpha` (`HAB0`)
envelope and its exact `apps.hta` manifest. It validates the envelope identity,
runtime ABI, manifest digest, embedded Hara `HBX0` bytes, lengths, checksums, and
canonical manifest encoding without discovering or executing application
source.

## Inspect generated output

```sh
hoplite inspect PROJECT
hoplite inspect --json PROJECT
```

`inspect` applies the same bundle and manifest validation, then reports route
and adapter counts, generated configuration and platform artifacts, OpenAPI
output, byte sizes, SHA-256 identities, and whether source inputs appear beneath
the generated output. JSON output uses `hoplite.inspect/0-alpha`.

Successful output is path-free by default. Add `--show-paths` only when local
filesystem disclosure is intentional.

## Diagnose the local serving environment

```sh
hoplite doctor PROJECT
hoplite doctor --json --strict PROJECT
hoplite doctor --deep PROJECT
```

The default doctor pass is read-only. It checks the current Hoplite executable,
the required embedded Nginx identity, adjacent source-free `hoplite-server`, CA
trust, project metadata, selected profile and main Var, package-lock/platform
consistency, and any existing generated application. JSON output uses
`hoplite.doctor/0-alpha`.

Warnings remain non-fatal unless `--strict` is supplied. `--deep` explicitly
authorizes source compilation, selected application evaluation, HAB0
construction, and complete preflight. Paths remain redacted unless
`--show-paths` is also supplied.

## Production handoff

After the checks succeed, run the slim serving plane:

```sh
hoplite serve build --mode prod PROJECT
hoplite verify PROJECT
hoplite inspect PROJECT
hoplite-server PROJECT
```

The production process consumes generated `.hoplite` artifacts. It does not
fall back to application source or expose the development control surface.

## Startup and request failures

Production workers emit `hoplite.startup-diagnostic/0-alpha` in strict order:
`configuration`, `bundle`, `modules`, `routes`, then `readiness`. A failed stage
contains one stable class and no later stage is emitted. Runtime ABI 4 exposes
the same borrowed JSON documents to native embedders through the `_v2`
application bootstrap callbacks.

Request-time logs use `hoplite.request-failure/0-alpha` classes for routing,
body limits, host suspension, unsupported yield, timeout, cancellation,
disconnect, streaming, and cleanup. `hoplite_request_timeout` defaults to 30
seconds and a timeout returns 504 while closing the owning work.

Scheduled and release evidence uses `hoplite.runtime-measurement/0-alpha` and
`hoplite.runtime-comparison/0-alpha`. Reports retain every raw sample and the
complete revision, fixture, tool, worker, and machine identity; comparisons
state environment compatibility before reporting median numeric deltas.

Failures exit non-zero. Diagnostic classes are stable enough to identify the
failed boundary without printing source text, credentials, signatures, native
pointers, provider internals, or build-machine paths.
