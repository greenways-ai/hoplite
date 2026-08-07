#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 SCRIPT [ARG...]" >&2
  exit 64
fi

target="$1"
shift

if [[ ! -f "$target" ]]; then
  echo "Benchmark script not found: $target" >&2
  exit 1
fi

docker_binary="$(type -P docker || true)"
if [[ -z "$docker_binary" ]]; then
  echo "Missing benchmark dependency: docker" >&2
  exit 1
fi

# Docker's container-top API requires a PID column even when callers only need
# command arguments. Newer daemons reject `ps -eo args`, so normalize that one
# query while leaving every other Docker command unchanged.
docker() {
  local args=("$@")
  local index
  if [[ "${args[0]:-}" == "top" ]]; then
    for ((index = 1; index + 1 < ${#args[@]}; index++)); do
      if [[ "${args[index]}" == "-eo" && "${args[index + 1]}" == "args" ]]; then
        args[index + 1]="pid,args"
      fi
    done
  fi
  "$docker_binary" "${args[@]}"
}

source "$target"
