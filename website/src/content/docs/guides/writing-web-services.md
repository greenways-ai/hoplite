---
title: Writing web services
description: Build, run, inspect, and operate a complete Hoplite HTTP service.
---

This guide takes one service from an empty directory to a running Nginx-backed
Hara application. It also describes the HTTP boundary that is implemented
today, so the examples do not depend on planned APIs.

Hoplite applications have three parts:

1. Hara functions that accept a request map and return a response map.
2. An immutable resource tree that maps methods and paths to handler Vars.
3. A `project.edn` profile that selects the application and its host options.

At build time, Hoplite validates the project, evaluates the selected application,
flattens its routes, prepares its handlers, generates OpenAPI output, and writes
an Nginx configuration.

```text
HTTP request
    ↓
Nginx
    ↓
HTA request value
    ↓
cached Hara handler
    ↓
HTA response value
    ↓
Nginx response
```

:::note[Before you begin]
Hoplite is pre-release and currently macOS-first. Follow [Installation](/getting-started/installation/)
to build the standalone executable from sibling Hara and Hoplite checkouts.
:::

## Create the project

Create a directory with an application source file and a Hara project manifest:

```text
hello-service/
├── app.hal
└── project.edn
```

## Define the handlers and routes

Create `app.hal`:

```clojure
(ns hello.service
  (:require [hoplite.core :as h]))

(defn hello
  [_request]
  {:status 200
   :headers {"content-type" "text/plain; charset=utf-8"}
   :body "Hello from Hoplite\n"})

(defn health
  [_request]
  {:status 200
   :headers {"content-type" "application/json; charset=utf-8"}
   :body "{\"status\":\"ok\"}\n"})

(defn echo-path
  [request]
  {:status 200
   :headers {"content-type" "text/plain; charset=utf-8"}
   :body (:path request)})

(defn ^:async delayed
  [_request]
  (std.foundation.coroutine/await
    (std.native.Host/call "nginx" "sleep" [25]))
  {:status 200
   :headers {"content-type" "text/plain; charset=utf-8"}
   :body "Coroutine resumed\n"})

(def app
  (h/app
    {:name "hello-service"
     :resources
     [["/api"
       ["/hello"
        {:get {:name :hello
               :summary "Return a greeting"
               :handler #'hello}}]

       ["/health"
        {:get {:name :health
               :summary "Report service health"
               :handler #'health}}]

       ["/echo/:id"
        {:get {:name :echo-path
               :summary "Return the requested path"
               :handler #'echo-path}}]

       ["/delay"
        {:get {:name :delay
               :summary "Resume after an Nginx timer"
               :handler #'delayed}}]]]}))
```

`h/app` describes immutable application data. Each operation stores a handler
Var such as `#'hello`; it does not call the handler while the application is
being declared. Hoplite resolves and prepares those Vars when an Nginx worker
starts.

The supported operation keys are `:get`, `:post`, `:put`, `:patch`, `:delete`,
`:head`, and `:options`. Every operation requires `:handler`; `:name` and
`:summary` improve the generated OpenAPI document.

## Select the application

Create `project.edn`:

```clojure
{:hara/type :project
 :hara/version "1.0.0"
 :project/id hello/service
 :project/version "0.1.0"
 :project/source-paths ["."]
 :project/test-paths []
 :project/extension-paths []
 :project/capabilities #{:host/nginx}
 :project/main hello.service
 :project/default-profile :server
 :project/profiles
 {:server
  {:profile/language :hoplite
   :profile/main hello.service/app
   :profile/options {:port 8080}}}}
```

The two main entries have different roles:

- `:project/main` names the project namespace.
- `:profile/main` names the exact application Var Hoplite will serve.

The selected profile must use `:profile/language :hoplite`. Its options may set
`:port` and `:workers`. Development mode defaults to one worker; production mode
defaults to the available CPU parallelism.

The default server does not add an authentication realm or management surface.
Authentication and authorization are explicit application policy; see
[Application authentication](/guides/authentication/).

## Check and run the service

From `hello-service/`, validate the project before starting it:

```sh
hoplite serve check --mode dev --profile server .
```

Run it in the foreground:

```sh
hoplite serve foreground --mode dev --profile server .
```

In another terminal, call the routes:

```sh
curl -i http://127.0.0.1:8080/api/hello
curl -i http://127.0.0.1:8080/api/health
curl -i http://127.0.0.1:8080/api/echo/42
curl -i http://127.0.0.1:8080/api/delay
```

The first response is:

```http
HTTP/1.1 200 OK
Content-Type: text/plain; charset=utf-8

Hello from Hoplite
```

Use `Ctrl-C` to stop foreground operation.

## Understand the request map

The current Nginx transport gives each handler a map with six fields:

```clojure
{:method "GET"
 :uri "/api/echo/42?verbose=true"
 :path "/api/echo/42"
 :query-string "verbose=true"
 :remote-address "127.0.0.1"
 :headers {"Host" "127.0.0.1:8080"
           "User-Agent" "curl/8.7.1"
           "Accept" "*/*"}}
```

Use `:uri` when the original URI is required, `:path` for routing-oriented
logic, and `:query-string` when parsing query input. Header names and values are
transported as strings.

An application that opts into bounded native bodies adds a closed profile to
`h/app`:

```clojure
{:name "upload-service"
 :request/body {:max-bytes 8388608
                :max-chunk-bytes 65536}
 :resources [...]}
```

For a non-empty declared request, the handler receives a positive
`:body-handle`. The handle is opaque, belongs to the current request and work,
and is usable only by an installed provider with request-body capability. It is
not a path or transferable authority. Native-body applications cannot contain
`:request+hta` routes. See [Requests and responses](/concepts/requests-responses/)
for the transport rules.

:::caution[Path parameters]
Segments beginning with `:` are matched, but their values are not yet bound into
a `:path-params` map. The `/api/echo/:id` example therefore reads the original
`:path`. Wildcards beginning with `*` match the remaining path and have the same
current limitation.
:::

## Return an HTTP response

A handler returns a response map:

```clojure
{:status 200
 :headers {"content-type" "text/plain; charset=utf-8"
           "cache-control" "no-store"}
 :body "Response body\n"}
```

The boundary currently behaves as follows:

- `:status` defaults to `200` and must be between `100` and `599`.
- `:headers` is optional; names and values should be text.
- `:body` may be a string, bytes, `nil`, or omitted.
- Nginx supplies `text/plain` when no content type is returned.
- Nginx calculates content length; a returned `content-length` is ignored.
- A non-map result, or a body that is not text or bytes, becomes a `500` response.

`hoplite.core/response` is available when a tagged logical response is useful:

```clojure
(h/response 200 "ok\n")
```

JSON is not encoded automatically. Serialize it before returning and set the
content type explicitly, as the `health` handler does above.

## Organize larger route trees

Child resources inherit their parent's path:

```clojure
["/users"
 {:get {:name :list-users
        :handler #'list-users}}
 ["/:id"
  {:get {:name :get-user
         :handler #'get-user}
   :delete {:name :delete-user
            :handler #'delete-user}}]]
```

Literal segments are more specific than parameter segments. A final segment
beginning with `*` can match the remaining path:

```clojure
["/assets/*path"
 {:get {:handler #'serve-asset}}]
```

Avoid declaring parameterized routes with identical shapes because their
meaning is ambiguous. For an application that owns all routing itself, pass one
handler Var directly:

```clojure
(def app (h/app #'handler))
```

That creates an any-method, any-path application route.

## Await host work without blocking Nginx

The `delayed` handler is marked `^:async`. It awaits the installed `nginx/sleep`
host operation, yields the worker's event loop, and resumes before returning a
normal response map.

```clojure
(defn ^:async delayed
  [_request]
  (std.foundation.coroutine/await
    (std.native.Host/call "nginx" "sleep" [25]))
  {:status 200 :body "resumed\n"})
```

Only call host services installed by the current runtime. See
[Async handlers](/guides/async-handlers/) and
[Host capabilities](/concepts/host-capabilities/) for the execution model.

## Inspect generated OpenAPI

In development mode, Hoplite exposes the generated document at
`/openapi.json` unless the app selects another path:

```sh
curl -s http://127.0.0.1:8080/openapi.json
```

A build also writes it to:

```text
.hoplite/openapi/hello-service.json
```

The current generator includes paths, methods, operation IDs, summaries, and a
generic success response. It does not yet infer request bodies, parameter
schemas, or response schemas. Production mode has no implicit OpenAPI endpoint;
set `:openapi {:path "/openapi.json"}` only when deliberate exposure is wanted.

## Inspect the build

Build without starting Nginx:

```sh
hoplite serve build --mode dev --profile server .
```

The generated application and platform plan live under `.hoplite/`:

```text
.hoplite/
├── app.hal
├── app.hbx
├── apps.hta
├── platform.edn
├── platform.hta
├── conf/
│   └── nginx.conf
└── openapi/
    └── hello-service.json
```

`platform.edn` is the inspectable module plan. The Hara
bytecode is generated and validated; the current Nginx startup path still uses
the generated HAL bootstrap while bytecode bootstrap integration is completed.

## Operate the production service

Validate and start the production configuration:

```sh
hoplite serve check --mode prod --profile server .
hoplite serve start --mode prod --profile server .
hoplite serve status .
```

After changing the service, rebuild and reload it:

```sh
hoplite serve build --mode prod --profile server .
hoplite serve reload .
```

Stop it cleanly:

```sh
hoplite serve stop .
```

For a container or process supervisor, keep it in the foreground:

```sh
hoplite serve foreground --mode prod --profile server .
```

See [Production operation](/guides/production-operation/) for logs, background
operation, provider ownership, and the macOS LaunchAgent lifecycle.

## Where to go next

- [Applications and resources](/concepts/applications-resources/) specifies the route model.
- [Requests and responses](/concepts/requests-responses/) describes logical HTTP values.
- [Development console](/guides/development-console/) provides a persistent Hara REPL for declared apps.
- [Multiple applications](/guides/multiple-applications/) hosts several apps in one Nginx configuration.
- [Project schema](/reference/project-schema/) documents profiles, modules, body limits, and route adapters.
- [Status and roadmap](/project/status/) tracks the pre-release boundaries described in this guide.
