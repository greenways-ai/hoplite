---
title: OpenAPI output
description: Generate and expose an OpenAPI description from declared resource operations.
---

Hoplite derives OpenAPI operation entries from the application's flattened resource tree. Operation `:name` and `:summary` metadata improve the generated description.

```clojure
["/hello"
 {:get {:name :hello
        :summary "Return a greeting"
        :handler #'hello}}]
```

## Development default

In development mode, an application defaults to exposing its OpenAPI document at `/openapi.json`.

## Explicit path

Set a path in the application definition when the interface should be exposed deliberately:

```clojure
(h/app {:name "api"
        :openapi {:path "/openapi.json"}
        :resources [...]})
```

Generated files are written under `.hoplite/openapi/<app-name>.json` during the build.

:::caution[Production]
Production mode has no implicit OpenAPI route. Configure `:openapi {:path ...}` when production exposure is intended, and apply appropriate access controls at the surrounding deployment layer.
:::
