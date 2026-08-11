---
title: Build output
description: Files generated beneath a Hoplite application's .hoplite directory.
---

Build output is isolated beneath the application project's `.hoplite/` directory:

```text
.hoplite/
  app.hal
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
| `app.hal` | Application bootstrap source used by the current Nginx integration |
| `app.hbx` | Deterministic alpha Hara bytecode bundle |
| `apps.hta` | Version 2 application/route manifest, including each route's boundary adapter |
| `platform.edn` | Inspectable, canonical module plan compiled from `project.edn` |
| `platform.hta` | HTA-encoded platform plan for worker startup |
| `conf/nginx.conf` | Generated Nginx worker, server, bootstrap, and manifest configuration |
| `openapi/*.json` | Interface descriptions generated from resource operations |
| `nginx.pid` | PID for managed project operations |
| `access.log` | Nginx access log |
| `error.log` | Nginx and Hoplite error log |

:::note[Bytecode transition]
`app.hbx` is generated, validated, and loaded transactionally by the active Nginx bootstrap ABI.
:::

The migration-only `legacy-management` feature may emit `auth-store.hta`, its
digest, and native-adapter link-plan files. They are not produced by the default
or published release build.
