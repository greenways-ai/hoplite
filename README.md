# Hoplite

Hoplite is the Hara HTTP runtime for Nginx. It turns immutable route data and
Hara handler Vars into a worker-local serving plane with prepared dispatch,
bounded host boundaries, asynchronous suspension, and streaming responses.

```text
request -> Nginx -> prepared Hoplite route -> Hara handler -> native response
```

> **Alpha versioning:** Hoplite follows Hara's pre-release contract epoch.
> Production application envelopes are `hoplite.application-bundle/0-alpha`
> (`HAB0`) and carry Hara `HBX0` bundles. Package versions remain ordinary
> pre-1.0 semantic versions, while runtime ABI numbers and `_v2` symbol suffixes
> describe separate native call shapes. See
> [Versioning during alpha](docs/versioning.md).

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
  app.hbx
  apps.hta
  platform.edn
  platform.hta
  conf/nginx.conf
  openapi/<app-name>.json
```

`app.hbx` is the deterministic `HAB0` alpha application envelope loaded
transactionally by a worker. Development builds may also emit
`.hoplite/app.hal` for inspection; production builds remove that source
projection. Generated `.hoplite` output is never registered as application
input, even when a project uses `:project/source-paths ["."]`. Inspect the
generated bundle, route manifest, configuration digests, and source-free status
with `hoplite inspect .`; use `--json` for the machine-readable
`hoplite.inspect/0-alpha` report. Verify a built application without executing it
with `hoplite verify .`. Diagnose the complete local project, Nginx, trust,
package-lock, and generated-output environment with `hoplite doctor .`; add
`--deep` only when source compilation and application preflight are intended.
Production startup emits ordered `hoplite.startup-diagnostic/0-alpha` stages;
request failures use `hoplite.request-failure/0-alpha`. See [Startup
diagnostics](docs/startup-diagnostics.md), [request failures](docs/request-failures.md),
and [runtime measurements](docs/runtime-measurements.md).

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
- the alpha application-bundle and worker-startup contract;
- the native data-plane and host-provider ABIs;
- the `hoplite` control CLI and `hoplite-server` serving CLI.

Breaking changes to these surfaces require a migration note, focused fixtures,
and a deliberate version decision. Downstream application policy and retired
historical products are not public API.

## Project boundaries

Hoplite owns HTTP application composition, Nginx integration, worker lifecycle,
request/response transport, asynchronous execution, streaming, bytecode startup,
and the interfaces required to embed or extend those capabilities.

Hoplite does **not** own application account models, authorization semantics,
replay policy, database products, object-storage products, or another project's
release proof. Extensions may implement Hoplite interfaces, but their publishing
and product acceptance do not determine whether the core library is healthy.

See [Project direction](docs/project-direction.md),
[Versioning during alpha](docs/versioning.md), and
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
coverage, and the documentation build. Benchmarks run separately so noisy
hardware evidence does not become a core merge gate.

## License

Apache License 2.0.
