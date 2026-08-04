# Hoplite

Hoplite is a Hara application server built directly into Nginx. Applications
are immutable resource trees, handlers are Hara Vars, and each Nginx worker
resolves and compiles its route handlers once during startup.

```text
request -> nginx worker -> cached Hara handler -> native response
```

HTA remains available as an explicit portability adapter; it is no longer the
mandatory boundary for every request.

Hoplite supports macOS and Linux. Tagged releases publish standalone binaries
for Apple Silicon, Intel macOS, ARM64 Linux and x86-64 Linux, alongside a
Homebrew formula and the `ghcr.io/greenways-ai/hoplite` OCI image.

## Install

### Homebrew

Use the fully qualified formula so Homebrew trusts only the selected Greenways
tap entry:

```shell
brew install greenways-ai/tap/hoplite
hoplite version
```

### Release installer

Install the published macOS or Linux binary without a package manager:

```shell
curl -fsSL https://raw.githubusercontent.com/greenways-ai/hoplite/main/scripts/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"
hoplite version
```

### Container

Run the published starter image immediately:

```shell
docker run --rm -p 8080:8080 ghcr.io/greenways-ai/hoplite:latest
curl -i http://127.0.0.1:8080/hello
```

## Create an application

Generate the two-file starter without cloning or building Hoplite:

```shell
curl -fsSL https://raw.githubusercontent.com/greenways-ai/hoplite/main/scripts/new-app.sh | sh -s -- hello
cd hello
```

A project selects one app Var. Routes are data; calling a handler is a runtime
operation, while referring to it with `#'` is declarative configuration.

`app.hal`:

```clojure
(ns example.app
  (:require [hoplite.core :as h]))

(defn hello
  [request]
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
           :profile/options {:port 8080}
           :profile/extensions
           {:extension/hoplite
            {:hoplite/authentication
             {:auth/realms
              {:management {:auth/providers [:auth/key]
                            :auth/required true}
               :application {:auth/providers [:auth/key]
                             :auth/required false}}}}}}}}
```

Hoplite owns authentication for both its management surface and application
requests. The built-in `:auth/key` provider is the default user-owned-key
mechanism. The application realm is permissive until a route policy requires a
principal; the management realm cannot be made public. Additional identity
mechanisms are installed as versioned Hoplite modules.

See [`examples/app.hal`](examples/app.hal) and
[`examples/project.edn`](examples/project.edn) for the complete starter.

## Run it

Check the project, build it, and start the service:

```shell
hoplite serve check .
hoplite serve build --mode dev .
hoplite serve foreground --mode dev .
```

In another terminal:

```shell
curl -i http://127.0.0.1:8080/hello
```

Production mode uses production worker defaults and disables development-only
behavior:

```shell
hoplite serve foreground --mode prod --profile server .
```

Foreground mode runs the application and the Hoplite management gateway in one
process. Management listens on `127.0.0.1:9090` by default; use
`HOPLITE_MANAGEMENT_LISTEN` to select another loopback socket or `off` to run
without the embedded management surface.

Build output is placed under the application's `.hoplite/` directory:

```text
.hoplite/
  app.hal
  app.hbc
  apps.hta
  platform.edn
  platform.hta
  conf/nginx.conf
  openapi/<app-name>.json
```

`platform.edn` is the inspectable compiled module and authentication plan;
`platform.hta` is the equivalent runtime transport. Both are produced from the
selected profile in `project.edn`.

The application bytecode is generated and validated today; Nginx still uses
the HAL bootstrap while the bytecode bootstrap ABI is being integrated.

## Development console

Running `hoplite` without arguments opens a Hara REPL with the project
namespaces and `hoplite.dev` service installed:

```clojure
(ns user
  (:require [example.app :as app]
            [hoplite.dev :as dev]))

(dev/start #'app/app {:project "." :profile :server})
(dev/list-all)
(dev/status #'app/app)
(dev/logs #'app/app {:bytes 4096})
(dev/restart #'app/app)
(dev/stop #'app/app)
```

The console accepts only an app Var selected by a project profile. It does not
execute arbitrary server definitions or shell commands. Multiple projects can
be tracked by the same console.

## Multiple applications

The normal `hoplite.core/app` interface describes one router. Advanced hosting
uses `hoplite.internal/config` to place several declared apps into one Nginx
configuration:

```clojure
(ns example.host
  (:require [example.api :as api]
            [example.admin :as admin]
            [hoplite.internal :as internal]))

(def config
  (internal/config
    {:worker-processes 4
     :apps [{:id :api
             :app #'api/app
             :port 8080
             :hostnames ["api.example.test"]}
            {:id :admin
             :app #'admin/app
             :port 8081
             :hostnames ["admin.example.test"]}]}))
```

Point the selected profile's `:profile/main` at `example.host/config`.

## Commands

```shell
hoplite                         # development console
hoplite eval '(+ 19 23)'
hoplite run file.hal
hoplite serve check PROJECT
hoplite serve build --mode dev PROJECT
hoplite serve foreground --mode prod PROJECT
hoplite serve start PROJECT
hoplite serve status PROJECT
hoplite serve reload PROJECT
hoplite serve stop PROJECT
hoplite serve install PROJECT   # macOS LaunchAgent
hoplite serve uninstall PROJECT
```

Set `HOPLITE_NGINX` only when deliberately selecting an external development
Nginx executable.

## Contributor build and test

The following targets are for contributors working on Hoplite itself, not the
product installation path:

```shell
make setup
make check
make runtime
make nginx
make macos
make benchmark-bytecode
```

`make nginx` downloads the pinned Nginx source, verifies its checksum, and
statically links the Hoplite module and Rust runtime. The final `hoplite`
executable embeds that Nginx binary.

The bytecode loading benchmark compares HAL compilation, HBC decoding, and
already-decoded execution for `hoplite.core`, `hoplite.internal`, and
`hoplite.dev`.

## Homebrew releases

The tagged release workflow:

1. verifies that the tag matches `Cargo.toml` and resolves immutable Hoplite and Hara commits;
2. builds the deterministic HARP package and container image;
3. builds and smoke-tests standalone binaries for both macOS architectures and both Linux architectures;
4. creates or updates the GitHub release without replacing the tag;
5. renders a source formula pinned to the exact release inputs;
6. updates `greenways-ai/homebrew-tap` when `HOMEBREW_TAP_TOKEN` is set.

The workflow can be dispatched manually for an existing tag when release
infrastructure changes. Tap setup and local formula instructions live in
[`packaging/homebrew/README.md`](packaging/homebrew/README.md).

## Runtime model

There is one Hoplite runtime per Nginx worker. Bootstrap loads application
definitions, then `apps.hta` prepares a worker-local router and compiles every
handler call once. A request performs method/path matching and executes the
cached program. Runtime Values, fibers, promises, and handler handles remain
inside their worker.

Asynchronous handlers can await Nginx host services without blocking:

```clojure
(defn delayed
  [_request]
  (std.foundation.coroutine/await
    (std.native.Host/call "nginx" "sleep" [25]))
  {:status 200 :body "resumed\n"})
```

Hara infers suspension from `await`. A normal handler stays on the direct path
until it actually suspends, so `^:async` is optional and no promise/work record
is allocated for a synchronous response.

Routes select one of three boundary adapters: `:raw` exposes the borrowed Nginx
exchange and `hoplite.raw` response operations, `:request` exposes a lazy
map-like request and returns response maps, and `:request+hta` materializes an
HTA request for portability and compatibility. See the routing-adapters guide
for the detailed contract and trade-offs.

## License

Apache License 2.0.
