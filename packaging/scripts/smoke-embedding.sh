#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
MANIFEST="$ROOT/core/runtime/Cargo.toml"
HEADER="$ROOT/core/nginx/hoplite_runtime.h"
INVENTORY="$ROOT/docs/native-symbols.txt"
TARGET="$ROOT/core/target/release"
STATIC_LIBRARY="$TARGET/libhoplite_runtime.a"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Remove only the runtime package's release outputs. Dependencies remain cached,
# but rustc must execute the static-library build below and therefore always
# emits the native link set instead of Cargo silently reusing an earlier output.
cargo clean \
  --manifest-path "$MANIFEST" \
  --release \
  --package hoplite-runtime

cargo rustc \
  --manifest-path "$MANIFEST" \
  --locked \
  --release \
  --lib \
  -- --print native-static-libs 2>&1 | tee "$WORK/rustc.log"

native_static_libs="$({
  sed -n 's/^.*native-static-libs: //p' "$WORK/rustc.log"
} | tail -n 1)"
if [[ -z "$native_static_libs" ]]; then
  echo "Rust did not report the native static-library link set" >&2
  exit 1
fi
read -r -a native_link_args <<<"$native_static_libs"

test -f "$STATIC_LIBRARY" || {
  echo "Rust did not build the Hoplite static library" >&2
  exit 1
}

cargo run \
  --manifest-path "$MANIFEST" \
  --locked \
  --release \
  --example embed

cc -std=c11 -Wall -Wextra -Werror \
  -I "$ROOT/core/nginx" \
  "$ROOT/core/runtime/examples/embed.c" \
  "$STATIC_LIBRARY" \
  "${native_link_args[@]}" \
  -o "$WORK/hoplite-c-embed"
"$WORK/hoplite-c-embed"

grep -Eo 'hoplite_[a-z0-9_]+\(' "$HEADER" \
  | tr -d '(' \
  | LC_ALL=C sort -u > "$WORK/header-symbols.txt"

nm -g --defined-only --format=posix "$STATIC_LIBRARY" \
  | awk '$1 ~ /^hoplite_/ { print $1 }' \
  | LC_ALL=C sort -u > "$WORK/binary-symbols.txt"

diff -u "$INVENTORY" "$WORK/header-symbols.txt"
diff -u "$INVENTORY" "$WORK/binary-symbols.txt"

printf '%s\n' \
  "Rust embedding fixture: passed" \
  "C embedding fixture: passed" \
  "public native header and binary symbols: exact"
