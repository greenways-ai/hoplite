#!/usr/bin/env bash
set -Eeuo pipefail

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
[[ -n "$repo_root" ]] || { echo "error: run from a Hoplite checkout" >&2; exit 1; }

fail() {
  echo "error: $*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

persist_local_bin() {
  local line='export PATH="$HOME/.local/bin:$PATH"'
  mkdir -p "$HOME/.local/bin"
  touch "$HOME/.bashrc"
  grep -Fqx "$line" "$HOME/.bashrc" || printf '\n%s\n' "$line" >> "$HOME/.bashrc"
  export PATH="$HOME/.local/bin:$PATH"
}

select_node() {
  local major="$1"
  export NVM_DIR="${NVM_DIR:-$HOME/.nvm}"
  if [[ -s "$NVM_DIR/nvm.sh" ]]; then
    # shellcheck disable=SC1090
    source "$NVM_DIR/nvm.sh"
    nvm install "$major"
    nvm use "$major"
  fi
  need node
  need npm
  [[ "$(node -p 'process.versions.node.split(".")[0]')" == "$major" ]] \
    || fail "Node $major is required; found $(node --version)"
}

ensure_rust() {
  need rustup
  rustup toolchain install 1.88.0 --profile minimal
  need cargo
}

ensure_checkout() {
  local repository="$1"
  local revision="$2"
  local checkout="$3"

  if [[ -e "$checkout" ]]; then
    [[ -d "$checkout/.git" ]] || fail "$checkout exists but is not a Git checkout"
    [[ -z "$(git -C "$checkout" status --porcelain --untracked-files=all)" ]] \
      || fail "dependency checkout is dirty: $checkout"
    local actual
    actual="$(git -C "$checkout" rev-parse HEAD)"
    [[ "$actual" == "$revision" ]] \
      || fail "dependency revision mismatch at $checkout (expected $revision, found $actual); refusing to reset it"
    return
  fi

  mkdir -p "$(dirname "$checkout")"
  local temporary="${checkout}.tmp.$$"
  rm -rf "$temporary"
  git clone --filter=blob:none --no-checkout "$repository" "$temporary"
  git -C "$temporary" fetch --depth 1 origin "$revision"
  git -C "$temporary" checkout --detach "$revision"
  mv "$temporary" "$checkout"
}

print_version() {
  local label="$1"
  shift
  printf '%-14s ' "$label:"
  "$@" --version 2>&1 | head -n 1 || true
}

persist_local_bin
select_node 24
ensure_rust
need git

hara_revision="$(tr -d '[:space:]' < "$repo_root/packaging/hara-revision")"
[[ "$hara_revision" =~ ^[0-9a-f]{40}$ ]] || fail "packaging/hara-revision must contain one full commit SHA"
hara_checkout="$(dirname "$repo_root")/hara"
ensure_checkout "https://github.com/hara-lang/hara.git" "$hara_revision" "$hara_checkout"

hara_manifest="$hara_checkout/core/rust/Cargo.toml"
[[ -f "$hara_manifest" ]] || fail "pinned Hara checkout has no core/rust/Cargo.toml"
cargo +1.88.0 fetch --locked --manifest-path "$hara_manifest"
cargo +1.88.0 build --locked --release --manifest-path "$hara_manifest" --bin hara --bin hara-test
install -m 0755 "$hara_checkout/core/rust/target/release/hara" "$HOME/.local/bin/hara"
install -m 0755 "$hara_checkout/core/rust/target/release/hara-test" "$HOME/.local/bin/hara-test"

cargo +1.88.0 fetch --locked --manifest-path "$repo_root/core/Cargo.toml"
cargo +1.88.0 build --locked --release --manifest-path "$repo_root/core/Cargo.toml" --bin hoplite
install -m 0755 "$repo_root/core/target/release/hoplite" "$HOME/.local/bin/hoplite"

npm ci --prefix "$repo_root/website"

[[ -z "$(git -C "$repo_root" status --porcelain --untracked-files=all)" ]] \
  || fail "setup changed the Hoplite working tree"

printf '\nHoplite development environment ready.\n'
print_version "Node" node
print_version "npm" npm
print_version "Rust" rustc +1.88.0
print_version "Cargo" cargo +1.88.0
print_version "Hara" hara
print_version "hara-test" hara-test
print_version "Hoplite" hoplite
printf 'Hara revision: %s\n' "$(git -C "$hara_checkout" rev-parse HEAD)"
cat <<'CHECKS'

Available checks (dependencies are prepared for offline execution):
  cargo +1.88.0 test --locked --manifest-path core/Cargo.toml --workspace
  cargo +1.88.0 check --locked --manifest-path core/Cargo.toml --examples
  npm run build --prefix website

Optional Docker integration (only when a daemon is available):
  docker info
  bash packaging/scripts/smoke-cosocket-tcp.sh
CHECKS
