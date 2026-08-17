#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
MANIFEST="$ROOT/core/runtime/Cargo.toml"
HEADER="$ROOT/core/nginx/hoplite_runtime.h"
INVENTORY="$ROOT/docs/native-symbols.txt"
TARGET="$ROOT/core/target/debug"
STATIC_LIBRARY="$TARGET/libhoplite_runtime.a"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# `rustc --print native-static-libs` writes linker arguments to the compiler
# diagnostic stream. Disable terminal colouring before capturing that line so
# an ANSI reset sequence can never become part of the final library name.
export CARGO_TERM_COLOR=never

# Remove only this package's outputs. The workspace test has already warmed the
# debug dependency graph, while rebuilding the static library guarantees rustc
# emits the native link set instead of Cargo reusing an earlier artifact.
cargo clean \
  --manifest-path "$MANIFEST" \
  --package hoplite-runtime

cargo rustc \
  --manifest-path "$MANIFEST" \
  --locked \
  --lib \
  -- --print native-static-libs 2>&1 | tee "$WORK/rustc.log"

native_static_libs="$({
  sed -n 's/^.*native-static-libs: //p' "$WORK/rustc.log"
} | tail -n 1)"
if [[ -z "$native_static_libs" ]]; then
  echo "Rust did not report the native static-library link set" >&2
  exit 1
fi
if [[ "$native_static_libs" == *$'\033'* ]]; then
  echo "Rust native static-library output contains a terminal escape sequence" >&2
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
  --example embed

cc -std=c11 -Wall -Wextra -Werror \
  -I "$ROOT/core/nginx" \
  "$ROOT/core/runtime/examples/embed.c" \
  "$STATIC_LIBRARY" \
  "${native_link_args[@]}" \
  -o "$WORK/hoplite-c-embed"
"$WORK/hoplite-c-embed"

cc -std=c11 -Wall -Wextra -Werror \
  -I "$ROOT/core/nginx" \
  "$ROOT/core/nginx/hoplite_console_transport.c" \
  "$ROOT/core/nginx/tests/console_transport.c" \
  -o "$WORK/hoplite-console-transport"
"$WORK/hoplite-console-transport"

cc -std=c11 -Wall -Wextra -Werror \
  -I "$ROOT/core/nginx" \
  "$ROOT/core/nginx/hoplite_console_transport.c" \
  "$ROOT/core/nginx/hoplite_console_worker.c" \
  "$ROOT/core/nginx/tests/console_worker.c" \
  -o "$WORK/hoplite-console-worker"
"$WORK/hoplite-console-worker"

grep -Eo 'hoplite_[a-z0-9_]+\(' "$HEADER" \
  | tr -d '(' \
  | LC_ALL=C sort -u > "$WORK/header-symbols.txt"

if command -v llvm-nm >/dev/null 2>&1; then
  NM_TOOL="$(command -v llvm-nm)"
elif [[ -x /opt/homebrew/opt/llvm/bin/llvm-nm ]]; then
  NM_TOOL=/opt/homebrew/opt/llvm/bin/llvm-nm
else
  NM_TOOL=nm
fi

"$NM_TOOL" -g --defined-only --format=posix "$STATIC_LIBRARY" \
  | awk '{ symbol=$1; sub(/^_/, "", symbol); if (symbol ~ /^hoplite_/ && symbol !~ /:$/) print symbol }' \
  | LC_ALL=C sort -u > "$WORK/binary-symbols.txt"

diff -u "$INVENTORY" "$WORK/header-symbols.txt"
diff -u "$INVENTORY" "$WORK/binary-symbols.txt"

printf '%s\n' \
  "Rust embedding fixture: passed" \
  "C embedding fixture: passed" \
  "console worker transport: passed" \
  "console worker lifecycle: passed" \
  "public native header and binary symbols: exact"
