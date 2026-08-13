---
title: Production operation
description: Build, run, install, inspect, reload, and stop a production application.
---

## Foreground operation

```sh
hoplite serve check --mode prod PROJECT
hoplite serve build --mode prod PROJECT
hoplite verify PROJECT
hoplite inspect PROJECT
hoplite-server PROJECT
```

Production mode uses available CPU parallelism unless the selected profile or
advanced host configuration specifies a worker count. It does not expose the
development-only default OpenAPI route and it does not start an account,
session, application-authorization, or management service.

`hoplite-server` is the slim production launcher. It consumes the generated
source-free `.hoplite` tree and has no application-source fallback. Use
`hoplite serve foreground` when the development/control CLI is intentionally
part of the operator workflow; use `hoplite-server` for the production serving
plane.

## Preflight and diagnosis

`hoplite verify` validates the HAB0 envelope and exact manifest without
executing source. `hoplite inspect` reports generated routes, adapters,
configuration identities, and source-free status. `hoplite doctor` checks the
complete local runtime and project environment:

```sh
hoplite doctor --strict PROJECT
hoplite verify PROJECT
hoplite inspect --json PROJECT
```

Paths are redacted from inspect and doctor output unless `--show-paths` is
explicitly supplied. See [Diagnosing an application](/guides/diagnostics/).

## Managed background process

```sh
hoplite serve start --mode prod PROJECT
hoplite serve status PROJECT
hoplite serve reload PROJECT
hoplite serve stop PROJECT
```

## macOS LaunchAgent

```sh
hoplite serve install --mode prod PROJECT
hoplite serve uninstall PROJECT
```

`install` and `uninstall` require macOS `launchd`. They are not supported on Linux.

## Logs and process files

The built Nginx configuration places the PID, access log, and error log inside the project's `.hoplite/` directory. Use the development console's `dev/logs` when the application is owned by that console.

The generic host registry remains available to trusted embeddings. Hoplite
0.2.0 ships no concrete blob, store, value, credential, or authorization
implementation and retains no provider-product packaging.

:::note[External Nginx]
Set `HOPLITE_NGINX` only when deliberately selecting an external development Nginx executable. The normal packaged executable embeds its Nginx host.
:::
