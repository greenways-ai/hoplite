# Hoplite

Hoplite is the Hara HTTP runtime for Nginx. It turns immutable route data and
Hara handler Vars into a worker-local serving plane with prepared dispatch,
bounded host boundaries, asynchronous suspension, and streaming responses.

```text
request -> Nginx -> prepared Hoplite route -> Hara handler -> native response
```

Hoplite is developed as a **library-first project**. Its product is the public
application API, runtime behaviour, embedding ABI, diagnostics, compatibility,
and performance. Storage products, application-specific authorization policy,
and downstream product release acceptance are deliberately outside the core
release gate.

## Why Hoplite

Hoplite is designed to make a small Hara service easy to understand without
placing a ceiling on production use:

- routes are immutable data and handlers are ordinary Hara Vars;
- route handlers are resolved and prepared once per Nginx worker;
- synchronous handlers keep a direct fast path;
- suspended handlers can await bounded host services without blocking a worker;
- request bodies remain request-scoped capabilities rather than ambient bytes;
- large and ranged responses stream with backpressure and bounded ownership;
- production workers boot from a deterministic, checksummed bytecode bundle;
- the development CLI and the production server remain separate surfaces.

## Install

### Homebrew

```shell
brew install greenways-ai/tap/hoplite
hoplite version
hoplite-server version
```

### Release installer

```shell
curl -fsSL https://raw.githubusercontent.com/greenways-ai/hoplite/main/packaging/scripts/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"
hoplite version
```

### Container

```shell
docker run --rm -p 8080:8080 ghcr.io/greenways-ai/hoplite:latest
curl -i http://127.0.0.1:8080/hello
```

## Create an application

Generate the starter project:

```shell
curl -fsSL https://raw.githubusercontent.com/greenways-ai/hoplite/main/packaging/scripts/new-app.sh | sh -s -- hello
cd hello
```

A Hoplite application selects one app Var. Routes are declared as data, and
referring to a handler with `#'` preserves the Var for worker preparation.

`app.hal`:

```clojure
(ns example.app
  (:require [hoplite.core :as h]))

(defn hello
  [_request]
  {:status 200
   :headers {"content-type" "text/plain; charset=utf-8"}
   :body "Hello from Hoplite\n"})

(def app
  (h/app
    {:name "example"
     :resources
     [["/hello"
       {:get {:name :hello
              :summary "Return a greeting"
              :route/adapter :request
              :handler #'hello}}]]}))
```

`project.edn`:

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

## Build and run

```shell
hoplite serve check .
hoplite serve build --mode dev .
hoplite serve foreground --mode dev .
```

For the slim production serving plane:

```shell
hoplite serve build --mode prod --profile server .
hoplite-server .
```

Production build output is isolated under `.hoplite/`:

```text
.hoplite/
  app.hal
  app.hbx
  apps.hta
  platform.edn
  platform.hta
  conf/nginx.conf
  openapi/<app-name>.json
```

`app.hbx` is the deterministic application bundle loaded transactionally by a
worker. `app.hal` remains inspectable build output and a development input; it
is not the production worker bootstrap artifact.

## Runtime model

There is one Hoplite runtime per Nginx worker. Bootstrap loads the application
bundle, constructs the worker-local router, and prepares every selected handler.
A request then performs method/path matching and invokes the prepared program.
Runtime values, fibers, promises, and handles never cross between workers.

Routes choose a boundary adapter:

- `:raw` exposes the borrowed Nginx exchange and the lowest-level response API;
- `:request` exposes a lazy request view and accepts ordinary response maps;
- `:request+hta` materializes a portable HTA request when portability is worth
  the extra allocation.

Asynchronous handlers use the same application model:

```clojure
(defn delayed
  [_request]
  (std.foundation.coroutine/await
    (std.native.Host/call "nginx" "sleep" [25]))
  {:status 200 :body "resumed\n"})
```

Hara infers suspension from `await`; a synchronous request does not allocate an
asynchronous work record merely because another route can suspend.

## Public surfaces

Before 1.0, Hoplite treats the following as intentional public surfaces:

- the `hoplite.core` application and routing API;
- the documented route adapters and request/response contracts;
- the production bundle and worker-startup contract for a release line;
- the native data-plane and host-provider ABIs;
- the `hoplite` control CLI and `hoplite-server` serving CLI.

Breaking changes to these surfaces require a migration note, focused fixtures,
and a deliberate version change. Internal provider implementations, downstream
application policy, and historical migration code are not promoted to public
API merely because they remain in the repository during extraction.

## Project boundaries

Hoplite owns HTTP application composition, Nginx integration, worker lifecycle,
request/response transport, asynchronous execution, streaming, bytecode startup,
and the interfaces required to embed or extend those capabilities.

Hoplite does **not** own application account models, authorization semantics,
replay policy, database products, object-storage products, or another project's
release proof. Extensions may implement Hoplite interfaces, but their publishing
and product acceptance do not determine whether the core library is healthy.

See [Project direction](docs/project-direction.md) and
[CI gates](docs/ci-gates.md) for the complete decision rules.

## Contributor workflow

Source and native build tooling live under `core/`:

```shell
cd core
make setup
make check
make runtime
make nginx
make server-cli
make benchmark-bytecode
```

The required pull-request gates cover the locked Rust/Hara workspace, public C
headers and registries, a real generic production image, request-body smoke
coverage, and the documentation build. Compatibility matrices and benchmarks
run separately so they provide evidence without turning extension publication
into a core merge gate.

## License

Apache License 2.0.
