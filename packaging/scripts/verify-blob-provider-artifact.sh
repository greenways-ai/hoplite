#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
usage: verify-blob-provider-artifact.sh ARCHIVE SHA256 [DESTINATION]

Verify an exact deterministic Hoplite blob-provider source artifact. SHA256 may
be the 64 lowercase hexadecimal value or its sha256: form. When DESTINATION is
provided, the verified package directory is extracted beneath it.
EOF
}

if [[ $# -lt 2 || $# -gt 3 ]]; then
  usage >&2
  exit 2
fi

archive=$1
expected=${2#sha256:}
destination=${3:-}

if [[ ! -f $archive ]]; then
  echo "provider artifact does not exist: $archive" >&2
  exit 2
fi
if [[ ! $expected =~ ^[0-9a-f]{64}$ ]]; then
  echo "expected provider artifact digest must be 64 lowercase hexadecimal digits" >&2
  exit 2
fi

for command in tar gzip sha256sum find sort cmp awk grep; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "required command is unavailable: $command" >&2
    exit 2
  fi
done

actual=$(sha256sum "$archive" | awk '{print $1}')
if [[ $actual != "$expected" ]]; then
  echo "provider artifact digest mismatch: expected $expected, got $actual" >&2
  exit 1
fi

temporary=$(mktemp -d "${TMPDIR:-/tmp}/hoplite-blob-provider-verify.XXXXXX")
trap 'rm -rf "$temporary"' EXIT
entries="$temporary/entries"
tar -tzf "$archive" > "$entries"
if [[ ! -s $entries ]]; then
  echo "provider artifact is empty" >&2
  exit 1
fi

if duplicate=$(LC_ALL=C sort "$entries" | uniq -d | head -n 1) && [[ -n $duplicate ]]; then
  echo "provider artifact contains duplicate entry: $duplicate" >&2
  exit 1
fi

root_name=
while IFS= read -r entry; do
  [[ -n $entry ]] || {
    echo "provider artifact contains an empty path" >&2
    exit 1
  }
  if [[ $entry == /* || $entry == *'//'* || $entry == *'/./'* || $entry == '../'* || $entry == *'/../'* || $entry == '..' ]]; then
    echo "provider artifact contains an unsafe path: $entry" >&2
    exit 1
  fi
  if [[ ! $entry =~ ^[A-Za-z0-9._/-]+$ ]]; then
    echo "provider artifact contains an unsupported path: $entry" >&2
    exit 1
  fi
  candidate=${entry%%/*}
  if [[ -z $root_name ]]; then
    root_name=$candidate
  elif [[ $candidate != "$root_name" ]]; then
    echo "provider artifact contains more than one root directory" >&2
    exit 1
  fi
done < "$entries"

if [[ ! $root_name =~ ^hoplite-blob-provider-[0-9]+\.[0-9]+\.[0-9]+([.-][A-Za-z0-9._-]+)?$ ]]; then
  echo "provider artifact root has an invalid package identity: $root_name" >&2
  exit 1
fi

while IFS= read -r line; do
  mode=${line%% *}
  type=${mode:0:1}
  if [[ $type != d && $type != - ]]; then
    echo "provider artifact contains a non-file entry: $line" >&2
    exit 1
  fi
done < <(tar -tvzf "$archive")

tar --no-same-owner --no-same-permissions -xzf "$archive" -C "$temporary"
package_root="$temporary/$root_name"
if [[ ! -d $package_root ]]; then
  echo "provider artifact root was not extracted" >&2
  exit 1
fi
if [[ -n $(find "$package_root" ! -type d ! -type f -print -quit) ]]; then
  echo "provider artifact contains a non-regular filesystem entry" >&2
  exit 1
fi

required_files=(
  FILES.sha256
  LICENSE
  PACKAGE_FORMAT
  PACKAGE_VERSION
  README.md
  SOURCE_REVISION
  provider-manifest.json
  abi/blob-store/Cargo.toml
  abi/blob-store-filesystem/Cargo.toml
  abi/blob-store-provider/Cargo.toml
  abi/blob-store-provider-ffi/Cargo.toml
  abi/provider-hta/Cargo.toml
)
for path in "${required_files[@]}"; do
  if [[ ! -f $package_root/$path ]]; then
    echo "provider artifact is missing required file: $path" >&2
    exit 1
  fi
done

if [[ $(<"$package_root/PACKAGE_FORMAT") != hoplite.provider-source/v1 ]]; then
  echo "provider artifact format is incompatible" >&2
  exit 1
fi
version=$(<"$package_root/PACKAGE_VERSION")
if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][A-Za-z0-9._-]+)?$ || $root_name != "hoplite-blob-provider-$version" ]]; then
  echo "provider artifact version and root identity do not match" >&2
  exit 1
fi
revision=$(<"$package_root/SOURCE_REVISION")
if [[ ! $revision =~ ^[0-9a-f]{40}$ ]]; then
  echo "provider artifact source revision is invalid" >&2
  exit 1
fi

inventory="$temporary/inventory"
actual_files="$temporary/actual-files"
if ! awk '
  NF != 2 { exit 1 }
  $1 !~ /^[0-9a-f]{64}$/ { exit 1 }
  $2 !~ /^\.\/[A-Za-z0-9._\/-]+$/ { exit 1 }
  $2 == "./FILES.sha256" { exit 1 }
  { print $2 }
' "$package_root/FILES.sha256" | LC_ALL=C sort > "$inventory"; then
  echo "provider artifact file inventory is malformed" >&2
  exit 1
fi
if duplicate=$(uniq -d "$inventory" | head -n 1) && [[ -n $duplicate ]]; then
  echo "provider artifact inventory contains duplicate path: $duplicate" >&2
  exit 1
fi
(
  cd "$package_root"
  sha256sum -c FILES.sha256 >/dev/null
  find . -type f ! -name FILES.sha256 -printf '%p\n' | LC_ALL=C sort > "$actual_files"
)
if ! cmp -s "$inventory" "$actual_files"; then
  echo "provider artifact files do not match the closed inventory" >&2
  exit 1
fi

if [[ -n $destination ]]; then
  mkdir -p "$destination"
  if [[ -n $(find "$destination" -mindepth 1 -print -quit) ]]; then
    echo "provider artifact destination is not empty: $destination" >&2
    exit 2
  fi
  mv "$package_root" "$destination/$root_name"
  package_root="$destination/$root_name"
fi

printf 'verified provider artifact sha256:%s %s\n' "$actual" "$package_root"
