---
title: Runtime model
description: Build/control ownership, worker-local execution, and the slim production server.
---

Hoplite has two executable surfaces with deliberately different responsibilities.

```text
hoplite
  build · check · REPL · packages · authentication · management

hoplite-server
  Nginx · worker-local Hara runtime · prepared application handlers
```

The all-in-one `hoplite` command remains the development and operational control
surface. `hoplite-server` is the production data plane: it contains the embedded
Nginx/Hara server, replaces itself with Nginx at startup, and does not retain the
compiler, REPL, package tooling, authentication store, or management gateway in
server memory.

Build once, then run the production artifact:

```shell
hoplite serve build --mode prod /path/to/project
hoplite-server /path/to/project
```

The published container image performs the build in its builder stage and runs
only `hoplite-server` in the final image.

## Worker ownership

There is one Hoplite runtime per Nginx worker. Runtime values do not cross worker boundaries.

```text
┌──────────────── Nginx worker ────────────────┐
│ Hoplite runtime                              │
│  ├─ decoded application definitions         │
│  ├─ worker-local router                     │
│  ├─ compiled handler calls                  │
│  ├─ values, fibers, and promises            │
│  └─ handles to explicitly installed hosts   │
└──────────────────────────────────────────────┘
```

## Startup

1. `hoplite serve build` evaluates the selected project profile and validates the application.
2. Build output records applications and routes in `apps.hta` and writes the generated Nginx configuration.
3. `hoplite-server` checks that the build output exists, materializes its embedded server when necessary, and replaces itself with Nginx.
4. Each Nginx worker creates one Hara runtime, loads the application definitions, prepares its router, and compiles every handler call once.

The release build strips the native Nginx/Hara server before embedding it in both
executables. The production artifact therefore carries one stripped serving
plane instead of the full Hoplite control program plus a second unstripped
server executable.

## Request execution

For each request, the worker matches method and path against borrowed Nginx
method/path slices and invokes the cached handler. The default `:request`
adapter exposes a lazy map-like extension: fields and headers are read only
when HAL code asks for them. A synchronous response is retained by the runtime
and referenced directly by Nginx until request cleanup; it does not allocate a
work record or create HTA events.

Only an actual `await` suspension creates a fiber/work record and enters the
Nginx event-loop continuation path. `:request+hta` remains available when a
fully materialized portable value is required.

## Bytecode status

Application bytecode is generated and validated during the current build. The
Nginx bootstrap still uses the HAL source while the bytecode bootstrap ABI is
integrated. Completing HBC-only worker boot is the next major opportunity to
remove parser, compiler, and general extension-host code from the serving plane.
This is an implementation milestone, not a stable compatibility promise.

## Worker defaults

Development mode uses one worker unless `:workers` is specified. Production
builds default to the available parallelism of the build host, while the slim
server may override that value at deployment:

```shell
hoplite-server --workers auto /path/to/project
HOPLITE_WORKERS=4 hoplite-server /path/to/project
```

The published container sets `HOPLITE_WORKERS=auto`, avoiding a worker count
baked in by the image builder. Set the variable to an empty value to retain the
project's generated setting. A `hoplite.internal/config` may set
`:worker-processes` explicitly for deployments that do not apply an override.
