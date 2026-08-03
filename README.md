# Hoplite

Hoplite is a Hara application server built into nginx. Applications are
immutable resource trees, handlers are Hara Vars, and each nginx worker
resolves and compiles its route handlers once during startup.

```text
request -> nginx -> borrowed Hara request -> cached handler -> native response -> nginx
```

HTA remains available as an explicit portability adapter; it is no longer the
mandatory boundary for every request.

Hoplite is currently macOS-first. Linux is exercised by CI and Docker, but the
standalone packaged executable and Homebrew distribution target macOS.

## Install

After the first tagged release is published:

```shell
brew tap greenways-ai/hoplite
brew install hoplite
hoplite version
```

Until then, build from sibling checkouts of Hoplite and Hara:

```shell
git clone https://github.com/hara-lang/hara.git hara.lang
git clone https://github.com/greenways-ai/hoplite.git hoplite
cd hoplite
make setup
make check
make macos
```

`make help` lists the common development, packaging, example, and benchmark
targets. `make install PREFIX=/usr/local` installs the standalone executable.

## Define an application

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
[`examples/project.edn`](examples/project.edn) for a runnable project.

## Run it

From this repository:

```shell
make example-check
make example-build
make example-dev
```

Or from `examples/`, use its project Makefile:

```shell
make check
make foreground
curl -i http://localhost:8080/hello
```

Production mode uses production worker defaults and disables development-only
behavior:

```shell
make example-prod
# or
hoplite serve foreground --mode prod --profile server /path/to/project
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

The application bytecode is generated and validated today; nginx still uses
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
uses `hoplite.internal/config` to place several declared apps into one nginx
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
nginx executable.

## Build and test

```shell
make check
make runtime
make nginx
make macos
make benchmark-bytecode
```

`make nginx` downloads the pinned nginx source, verifies its checksum, and
statically links the Hoplite module and Rust runtime. The final `hoplite`
executable embeds that nginx binary.

The bytecode loading benchmark compares HAL compilation, HBC decoding, and
already-decoded execution for `hoplite.core`, `hoplite.internal`, and
`hoplite.dev`.

## Homebrew releases

The tagged release workflow:

1. verifies that the tag matches `Cargo.toml`;
2. builds arm64 and Intel standalone macOS executables;
3. publishes both files in a GitHub release;
4. calculates their SHA-256 checksums;
5. generates and publishes `Formula/hoplite.rb`;
6. updates `greenways-ai/homebrew-hoplite` when `HOMEBREW_TAP_TOKEN` is set.

Tap setup and local formula instructions live in
[`packaging/homebrew/README.md`](packaging/homebrew/README.md).

## Runtime model

There is one Hoplite runtime per nginx worker. Bootstrap loads application
definitions, then `apps.hta` prepares a worker-local router and compiles every
handler call once. A request performs method/path matching and executes the
cached program. Runtime Values, fibers, promises, and handler handles remain
inside their worker.

Asynchronous handlers can await nginx host services without blocking:

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

Eclipse Public License 2.0.
