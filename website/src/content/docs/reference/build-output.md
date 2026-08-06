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
  auth-store.hta
  auth-store.hta.sha256
  native-adapters.edn
  native-adapters.Cargo.toml
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
| `apps.hta` | Version 2 application/route manifest, including each route's boundary adapter |
| `auth-store.hta` | Canonical typed `hoplite-auth-store/1` operation and wire contract |
| `auth-store.hta.sha256` | Digest used to bind adapters and compiled artifacts to the exact HTA contract |
| `native-adapters.edn` | Locked registry of native adapter factories selected by package, export, crate, and ABI |
| `native-adapters.Cargo.toml` | Publication-time Cargo dependency fragment using verified content-addressed HARP paths |
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
