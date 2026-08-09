#!/bin/sh
set -eu

name="${1:-hoplite-app}"

case "$name" in
  ""|"."|".."|*/*)
    echo "Choose a simple directory name, for example: hello" >&2
    exit 1
    ;;
esac

if [ -e "$name" ]; then
  echo "Path already exists: $name" >&2
  exit 1
fi

mkdir -p "$name"

cat > "${name}/app.hal" <<'HAL'
(ns app
  (:require [hoplite.core :as h]))

(defn hello
  [_request]
  {:status 200
   :headers {"content-type" "text/plain; charset=utf-8"
             "x-hoplite" "true"}
   :body "Hello from Hoplite\n"})

(def app
  (h/app
    {:name "hello"
     :resources
     [["/hello"
       {:get {:name "hello"
              :summary "Return a greeting"
              :handler #'hello}}]]}))
HAL

cat > "${name}/project.edn" <<'EDN'
{:hara/type :project
 :hara/version "1.0.0"
 :project/id hoplite/app
 :project/version "0.1.0"
 :project/source-paths ["."]
 :project/test-paths []
 :project/extension-paths []
 :project/capabilities #{:host/nginx}
 :project/main app
 :project/default-profile :server
 :project/profiles
 {:server {:profile/language :hoplite
           :profile/main app/app
           :profile/options {:port 8080}}}}
EDN

cat <<EOF
Created ${name}/app.hal
Created ${name}/project.edn

Run it:
  cd ${name}
  hoplite serve foreground --mode prod .
EOF
