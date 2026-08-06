---
title: Development workflow
description: Check, build, run, and inspect a Hoplite application during development.
---

## Repository example

From the Hoplite repository:

```sh
make example-check
make example-build
make example-dev
```

Or work directly in `examples/`:

```sh
make check
make foreground
curl -i http://localhost:8080/hello
```

## Direct CLI workflow

```sh
hoplite serve check PROJECT
hoplite serve build --mode dev PROJECT
hoplite serve foreground --mode dev PROJECT
```

Development mode uses one worker by default and exposes generated OpenAPI at `/openapi.json` unless the application specifies another path. Production defaults differ; see [Production operation](/guides/production-operation/).

## Interactive console

Running `hoplite` with no arguments opens a persistent Hara REPL with project namespaces and the `hoplite.dev` host service installed. Use it to start and inspect explicitly declared application Vars. See [Development console](/guides/development-console/).
