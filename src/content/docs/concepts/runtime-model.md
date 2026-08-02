---
title: Runtime model
description: Worker ownership, startup compilation, values, fibers, and request execution.
---

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

1. The selected project profile evaluates to an application or advanced host configuration.
2. Build output records applications and routes in `apps.hta`.
3. The Nginx bootstrap loads the application definitions.
4. Each worker creates its own router and compiles every handler call once.

## Request execution

For each request, the worker matches method and path, translates the request to HTA, executes the cached handler program, then translates the returned logical value to an Nginx response.

## Bytecode status

Application bytecode is generated and validated during the current build. The Nginx bootstrap still uses the HAL source while the bytecode bootstrap ABI is integrated. This is an implementation milestone, not a stable compatibility promise.

## Worker defaults

Development mode uses one worker unless `:workers` is specified. Production mode defaults to the available CPU parallelism. A `hoplite.internal/config` may set `:worker-processes` explicitly.
