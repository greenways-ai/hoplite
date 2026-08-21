#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

forbidden='HAB1|HBB2|hoplite\.application-bundle/1|hoplite\.public-surfaces/1'
matches="$(
  git grep -n -E "$forbidden" -- \
    . \
    ':!packaging/scripts/verify-alpha-versioning.sh' \
    || true
)"
if [[ -n "$matches" ]]; then
  printf '%s\n' "$matches" >&2
  echo "stale stable-looking version identifiers remain" >&2
  exit 1
fi

grep -F 'pub const FORMAT: &str = "hoplite.application-bundle/0-alpha";' \
  core/application-bundle/src/lib.rs >/dev/null
grep -F 'pub const MAGIC: &[u8; 4] = b"HAB0";' \
  core/application-bundle/src/lib.rs >/dev/null
grep -F 'pub const HARA_BUNDLE_MAGIC: &[u8; 4] = b"HBX0";' \
  core/application-bundle/src/lib.rs >/dev/null
grep -F 'pub const RUNTIME_ABI_VERSION: u32 = 5;' \
  core/application-bundle/src/lib.rs >/dev/null

test "$(tr -d '[:space:]' < packaging/hara-revision)" = \
  '2bb38e5fffa301ff372da866f4079e88f3dde1ea'
grep -F 'HARA_REF: 2bb38e5fffa301ff372da866f4079e88f3dde1ea' \
  .github/workflows/ci.yml >/dev/null
grep -F 'HARA_REF: 2bb38e5fffa301ff372da866f4079e88f3dde1ea' \
  .github/workflows/cosocket.yml >/dev/null

grep -F 'docs/versioning.md' README.md >/dev/null
grep -F 'hoplite.application-bundle/0-alpha' docs/application-bundle.md >/dev/null
grep -F 'Hara bytecode bundle | `HBX0`' docs/versioning.md >/dev/null
grep -F 'hoplite_bootstrap_application_v2' core/nginx/hoplite_runtime.h >/dev/null
grep -F 'hoplite_handler_invoke_v4' core/nginx/hoplite_runtime.h >/dev/null
grep -F 'hoplite_app_invoke_v4' core/nginx/hoplite_runtime.h >/dev/null
grep -F 'hoplite_abi_version() < 5' \
  core/nginx/ngx_http_hoplite_module.c >/dev/null
grep -F 'hoplite.startup-diagnostic/0-alpha' docs/startup-diagnostics.md >/dev/null
grep -F 'hoplite.runtime-measurement/0-alpha' docs/runtime-measurements.md >/dev/null

echo "alpha version policy verified"
