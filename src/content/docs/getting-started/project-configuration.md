---
title: Project configuration
description: Select a Hoplite application through a Hara project profile.
---

A Hoplite project selects one qualified application Var through `project.edn`.

```clojure
{:hara/type :project
 :hara/version "1.0.0"
 :project/id example/app
 :project/version "0.1.0"
 :project/source-paths ["."]
 :project/test-paths []
 :project/extension-paths []
 :project/capabilities #{:host/nginx}
 :project/main example.app
 :project/default-profile :server
 :project/profiles
 {:server {:profile/language :hoplite
           :profile/main example.app/app
           :profile/options {:port 8080}}}}
```

## Required profile fields

| Field | Meaning |
| --- | --- |
| `:profile/language` | Must be `:hoplite` |
| `:profile/main` | Qualified Var that evaluates to `hoplite.core/app` or `hoplite.internal/config` |
| `:profile/options` | Optional map containing `:port` and `:workers` |

The command-line `--profile NAME` option overrides `:project/default-profile`.

:::caution[Legacy files are rejected]
Projects containing `server.edn` or `routes.edn` are no longer supported. Move routing into `hoplite.core/app` and select it through `:project/profiles`.
:::

See the complete [project schema reference](/reference/project-schema/).
