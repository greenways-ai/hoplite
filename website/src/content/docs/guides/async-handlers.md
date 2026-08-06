---
title: Async handlers
description: Await Nginx host services without blocking the worker.
---

Await the host operation, then return a normal logical response. Hara infers
that the function may suspend; `^:async` remains accepted but is not required.

```clojure
(defn delayed
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

Calling the handler begins synchronously. If it returns without suspension,
Hoplite uses the same direct response path as any other handler and allocates
no promise/work record. Only when `await` observes a pending operation does the
VM retain its continuation and yield control to Nginx's event loop. The promise
and fiber remain inside the worker-local Hoplite runtime.

:::caution[Capability availability]
Only call host services installed for the current runtime. The interface is pre-release; confirm supported operations against the current source and examples.
:::
