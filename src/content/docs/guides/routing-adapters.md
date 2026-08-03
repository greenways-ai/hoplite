---
title: Routing adapters
description: Choose raw, lazy request, or portable HTA request boundaries per route.
---

Every operation accepts `:route/adapter`. The application-level value supplies
the default; an operation may override it.

```clojure
(h/app
  {:name "example"
   :route/adapter :request
   :resources
   [["/portable"
     {:post {:handler #'portable-handler
             :route/adapter :request+hta}}]]})
```

## `:request` — default

The handler receives a lazy map-like request backed by the active Nginx
request. Looking up `:headers` does not copy or encode all headers, and a
synchronous response returns through ABI v2 without an HTA event or work
allocation. This is the normal choice for application handlers.

## `:raw`

The handler receives an exchange used with `hoplite.raw`. Accessors read the
borrowed request and response operations construct the native response.

```clojure
(ns example.raw
  (:require [hoplite.raw :as raw]))

(defn health [exchange]
  (raw/respond! exchange 200
                {"content-type" "text/plain"}
                "ok\n"))
```

`raw/start!`, `raw/write!`, and `raw/finish!` provide the response-builder
surface. The current ABI buffers builder chunks until `finish!`; direct Nginx
backpressure-aware streaming is still experimental.

## `:request+hta`

The handler receives a materialized HTA round-trip of the request. Choose this
for portability boundaries, isolated adapters, or compatibility with code that
must own the value independently of Nginx request memory. It deliberately pays
allocation and encode/decode costs.

## Direct Nginx configuration

Standalone handlers use the same choices. The default is `request`.

```nginx
hoplite_content example.app/fast request;
hoplite_content example.raw/health raw;
hoplite_content example.portable/handle request+hta;
```

An unknown adapter is rejected while Nginx validates its configuration.
