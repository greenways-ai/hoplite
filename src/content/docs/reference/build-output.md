---
title: Build output
description: Files generated beneath a Hoplite application's .hoplite directory.
---

Build output is isolated beneath the application project's `.hoplite/` directory:

```text
.hoplite/
  app.hal
  app.hbc
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
| `app.hal` | Application bootstrap source used by the current Nginx integration |
| `app.hbc` | Generated and validated Hara bytecode |
| `apps.hta` | Encoded application/route manifest consumed at worker startup |
| `platform.edn` | Inspectable, canonical module and authentication plan compiled from `project.edn` |
| `platform.hta` | HTA-encoded platform plan for worker startup |
| `conf/nginx.conf` | Generated Nginx worker, server, bootstrap, and manifest configuration |
| `openapi/*.json` | Interface descriptions generated from resource operations |
| `nginx.pid` | PID for managed project operations |
| `access.log` | Nginx access log |
| `error.log` | Nginx and Hoplite error log |

:::note[Bytecode transition]
`app.hbc` is generated and validated today. The active Nginx bootstrap still uses HAL while the bytecode bootstrap ABI is integrated.
:::
