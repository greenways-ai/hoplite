---
title: Async handlers
description: Await Nginx host services without blocking the worker.
---

Mark an asynchronous handler with `^:async`, await the host operation, then return a normal logical response.

```clojure
(defn ^:async delayed
  [_request]
  (std.foundation.coroutine/await
    (std.native.Host/call "nginx" "sleep" [25]))
  {:status 200 :body "resumed\n"})
```

Declare it like any other handler:

```clojure
["/delayed"
 {:get {:name :delayed
        :summary "Respond after an asynchronous delay"
        :handler #'delayed}}]
```

## Execution behavior

The promise and fiber remain inside the worker-local Hoplite runtime. Awaiting the host service yields control instead of blocking Nginx's event loop.

:::caution[Capability availability]
Only call host services installed for the current runtime. The interface is pre-release; confirm supported operations against the current source and examples.
:::
