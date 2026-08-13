---
title: Build output
description: Files generated beneath a Hoplite application's .hoplite directory.
---

Build output is isolated beneath the application project's `.hoplite/` directory:

```text
.hoplite/
  app.hbx
  apps.hta
  platform.edn
  platform.hta
  conf/nginx.conf
  openapi/<app-name>.json
  nginx.pid
  access.log
  error.log
```

| Path | Purpose |
| --- | --- |
| `app.hbx` | Deterministic `HAB0` Hoplite application envelope carrying the ordered Hara `HBX0` modules |
| `apps.hta` | Version 2 application/route manifest, including each route's boundary adapter |
| `platform.edn` | Inspectable, canonical module plan compiled from `project.edn` |
| `platform.hta` | HTA-encoded platform plan for worker startup |
| `conf/nginx.conf` | Generated Nginx worker, server, bootstrap, and manifest configuration |
| `openapi/*.json` | Interface descriptions generated from resource operations |
| `nginx.pid` | PID for managed project operations |
| `access.log` | Nginx access log |
| `error.log` | Nginx and Hoplite error log |

:::note[Source-free production]
Production builds contain no `.hal`, `project.edn`, or `hara.extension.edn`
input beneath `.hoplite`. Development builds may additionally emit `app.hal` as
the `hoplite.development-source-projection/0-alpha` inspection projection; it is
not loaded by production workers and is rejected from production output.
:::

`app.hbx` is bound to the exact `apps.hta` digest and is validated before
application preflight. Workers publish a fresh runtime only after every HBX0
module and selected route prepares successfully.

Inspect the generated tree with `hoplite inspect PROJECT` and validate the
required envelope and manifest with `hoplite verify PROJECT`.

No compatibility feature emits authentication, provider, value, blob, or store
artifacts. Those historical products were retired in 0.2.0.
