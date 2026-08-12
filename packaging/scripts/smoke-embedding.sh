#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
MANIFEST="$ROOT/core/runtime/Cargo.toml"
HEADER="$ROOT/core/nginx/hoplite_runtime.h"
INVENTORY="$ROOT/docs/native-symbols.txt"
WORK="$(mktemp -d)"
TARGET="$WORK/target"
STATIC_LIBRARY="$TARGET/release/libhoplite_runtime.a"
trap 'rm -rf "$WORK"' EXIT

# Use an isolated target directory and ask rustc for the native link set before
# running the Rust fixture. This guarantees rustc executes with
# --print=native-static-libs instead of Cargo reusing a release artifact built
# by an earlier CI step and emitting no link metadata.
CARGO_TARGET_DIR="$TARGET" cargo rustc \
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

CARGO_TARGET_DIR="$TARGET" cargo run \
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
