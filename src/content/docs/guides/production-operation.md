---
title: Production operation
description: Build, run, install, inspect, reload, and stop a production application.
---

## Foreground operation

```sh
hoplite serve check --mode prod PROJECT
hoplite serve build --mode prod PROJECT
hoplite serve foreground --mode prod PROJECT
```

Production mode uses available CPU parallelism unless the selected profile or advanced host configuration specifies a worker count. It does not expose the development-only default OpenAPI route.

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

:::note[External Nginx]
Set `HOPLITE_NGINX` only when deliberately selecting an external development Nginx executable. The normal packaged executable embeds its Nginx host.
:::
