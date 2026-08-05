---
title: Static upstreams
description: Connect a Hoplite application to a fixed HTTPS service without introducing request-selected proxy authority.
---

Hoplite can place an immutable Nginx proxy prefix beside Hara routes. This is useful for a local gateway whose policy and discovery surface are written in Hara while a reviewed remote service owns a separate API.

```clojure
(ns greenways.beacon
  (:require [hoplite.core :as h]))

(defn health
  [_request]
  {:status 200
   :headers {"content-type" "application/json"}
   :body "{\"status\":\"ready\"}"})

(def app
  (h/app
    {:name "greenways-beacon"
     :proxies
     [{:path "/space/"
       :upstream "https://greenways.space/beacon/v1/"}]
     :resources
     [["/beacon/v1/health"
       {:get {:name :beacon/health
              :handler #'health}}]]}))
```

A request to `/space/rooms/42?view=summary` is sent to `https://greenways.space/beacon/v1/rooms/42?view=summary`. The method and body are forwarded by Nginx and never materialized in a Hara worker.

## Security boundary

Static upstreams are deliberately narrower than a general forward proxy:

- the source prefix and destination are selected by the built project;
- request headers cannot choose a hostname, scheme or destination path;
- remote destinations require HTTPS;
- HTTP is accepted only for `localhost`, `127.0.0.1`, or `[::1]` development targets;
- user information, query strings, fragments, variables and dot segments are rejected in configuration;
- local cookies, `Origin`, `Referer`, and `X-Forwarded-*` ambient authority are cleared;
- `Authorization` and application-defined signed headers remain available for an explicit upstream protocol; and
- generated OpenAPI carries an `x-hoplite-static-proxies` inventory so the built route is inspectable.

The upstream still authenticates and authorizes the caller. A proxy declaration proves only that the application author approved a route to that origin.

## Prefix rules

Both paths must be directory-like prefixes:

```clojure
{:path "/space/"
 :upstream "https://greenways.space/beacon/v1/"}
```

These are rejected:

```clojure
;; Remote cleartext
{:path "/space/"
 :upstream "http://greenways.space/beacon/v1/"}

;; Request-selected Nginx variable
{:path "/space/"
 :upstream "https://greenways.space/$request_uri"}

;; Embedded credential
{:path "/space/"
 :upstream "https://token@greenways.space/beacon/v1/"}

;; Ambiguous traversal
{:path "/space/"
 :upstream "https://greenways.space/beacon/../private/"}
```

## Development

A local Space implementation may be selected in a separate app Var or profile:

```clojure
{:path "/space/"
 :upstream "http://127.0.0.1:5173/beacon/v1/"}
```

Do not make the destination configurable through a query parameter, request header, or unvalidated environment interpolation. Generate a reviewed application definition for each deployment profile instead.
