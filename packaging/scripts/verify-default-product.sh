#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
MANIFEST="$ROOT/core/Cargo.toml"
TARGET_DIR="$(mktemp -d)"
TREE_FILE="$(mktemp)"
trap 'rm -rf "$TARGET_DIR" "$TREE_FILE"' EXIT

cargo build \
  --manifest-path "$MANIFEST" \
  --locked \
  --bins \
  --target-dir "$TARGET_DIR"

for program in hoplite hoplite-server; do
  test -x "$TARGET_DIR/debug/$program" || {
    echo "default product did not build $program" >&2
    exit 1
  }
done

actual_programs="$({
  find "$TARGET_DIR/debug" \
    -maxdepth 1 \
    -type f \
    -perm -111 \
    -printf '%f\n'
} | LC_ALL=C sort)"
expected_programs="$(printf '%s\n' hoplite hoplite-server | LC_ALL=C sort)"
if [[ "$actual_programs" != "$expected_programs" ]]; then
  echo "default Cargo binary set is not the two-program Hoplite product" >&2
  printf 'expected:\n%s\nactual:\n%s\n' "$expected_programs" "$actual_programs" >&2
  exit 1
fi

cargo build \
  --manifest-path "$MANIFEST" \
  --locked \
  --features application-console \
  --bin hoplite-console-bundle \
  --bin hoplite-console-evaluator \
  --bin hoplite-console-supervisor \
  --target-dir "$TARGET_DIR"

for program in \
  hoplite-console-bundle \
  hoplite-console-evaluator \
  hoplite-console-supervisor; do
  test -x "$TARGET_DIR/debug/$program" || {
    echo "explicit application-console feature did not build $program" >&2
    exit 1
  }
done

cargo tree \
  --manifest-path "$MANIFEST" \
  --locked \
  --package hoplite \
  --depth 1 \
  --edges normal \
  --prefix none >"$TREE_FILE"

direct_packages="$(awk '{print $1}' "$TREE_FILE" | LC_ALL=C sort -u)"
for required in \
  base64 \
  ed25519-dalek \
  getrandom \
  hara-native \
  hoplite-application-bundle \
  p256; do
  grep -Fxq "$required" <<<"$direct_packages" || {
    echo "default Hoplite dependency tree is missing generic runtime dependency $required" >&2
    exit 1
  }
done

for forbidden in \
  hoplite-auth-store-abi \
  hoplite-store-sqlite \
  rusqlite; do
  if grep -Fxq "$forbidden" <<<"$direct_packages"; then
    echo "default Hoplite dependency tree contains retired product dependency $forbidden" >&2
    exit 1
  fi
done

printf '%s\n' \
  "default product binaries: hoplite, hoplite-server" \
  "application console binaries: explicit application-console feature" \
  "generic host cryptography: present" \
  "application-authentication and database products: absent"
