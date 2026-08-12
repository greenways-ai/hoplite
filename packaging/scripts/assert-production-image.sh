#!/usr/bin/env bash
set -euo pipefail

image="${1:-hoplite-ci}"

docker run --rm --entrypoint sh "$image" -eu -c '
test "$(id -u)" -ne 0
test -d /app/.hoplite
test "$(find /app -mindepth 1 -maxdepth 1 | wc -l)" -eq 1
test -f /app/.hoplite/app.hbx
test -f /app/.hoplite/apps.hta
test -f /app/.hoplite/conf/nginx.conf
test ! -e /app/.hoplite/app.hal
test ! -e /app/app.hal
test ! -e /app/project.edn
test ! -e /app/Makefile
if find /app -type f \( -name "*.hal" -o -name "project.edn" -o -name "hara.extension.edn" \) -print -quit | grep -q .; then
  echo "production image contains Hara application source or project input" >&2
  find /app -type f \( -name "*.hal" -o -name "project.edn" -o -name "hara.extension.edn" \) -print >&2
  exit 1
fi
for tool in hoplite cargo rustc cc make; do
  if command -v "$tool" >/dev/null 2>&1; then
    echo "production image unexpectedly contains build tool: $tool" >&2
    exit 1
  fi
done
find /app -type f -print | sort
'

printf 'Validated source-free production composition for %s.\n' "$image"
