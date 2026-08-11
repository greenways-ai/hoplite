#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
usage: build-value-provider-artifact.sh VERSION HOPLITE_REVISION HARA_REPOSITORY HARA_REVISION OUTPUT

Build a deterministic target-independent Hoplite value-provider source artifact
from one exact Hoplite commit and one exact Hara canonical-decoder commit.
OUTPUT receives a gzip-compressed ustar archive and OUTPUT.sha256 receives its
lowercase SHA-256 checksum.
EOF
}

if [[ $# -ne 5 ]]; then
  usage >&2
  exit 2
fi

version=$1
hoplite_revision=$2
hara_repository=$3
hara_revision=$4
output=$5

if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][A-Za-z0-9._-]+)?$ ]]; then
  echo "invalid provider package version: $version" >&2
  exit 2
fi

for command in git tar gzip sha256sum find sort awk tr; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "required command is unavailable: $command" >&2
    exit 2
  fi
done
if [[ ! -d $hara_repository/.git ]]; then
  echo "Hara repository is unavailable: $hara_repository" >&2
  exit 2
fi

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repository=$(cd "$script_dir/../.." && pwd)
hoplite_commit=$(git -C "$repository" rev-parse --verify "$hoplite_revision^{commit}")
hara_commit=$(git -C "$hara_repository" rev-parse --verify "$hara_revision^{commit}")
if [[ ! $hoplite_commit =~ ^[0-9a-f]{40}$ ]]; then
  echo "Hoplite revision did not resolve to a full Git commit: $hoplite_revision" >&2
  exit 2
fi
if [[ ! $hara_commit =~ ^[0-9a-f]{40}$ ]]; then
  echo "Hara revision did not resolve to a full Git commit: $hara_revision" >&2
  exit 2
fi

expected_hara=$(git -C "$repository" show \
  "$hoplite_commit:packaging/hara-value-revision" | tr -d '\r\n')
if [[ $expected_hara != "$hara_commit" ]]; then
  echo "Hoplite revision expects Hara $expected_hara, not $hara_commit" >&2
  exit 2
fi

package_name="hoplite-value-provider-$version"
artifact_name="$package_name.tar.gz"
temporary=$(mktemp -d "${TMPDIR:-/tmp}/hoplite-value-provider.XXXXXX")
trap 'rm -rf "$temporary"' EXIT
hoplite_source="$temporary/hoplite-source"
hara_source="$temporary/hara-source"
package_root="$temporary/$package_name"
mkdir -p \
  "$hoplite_source" \
  "$hara_source" \
  "$package_root/hoplite/core/abi" \
  "$package_root/hara/core/rust" \
  "$package_root/backend"

hoplite_payload=(
  LICENSE
  core/abi/blob-store
  core/abi/blob-filesystem-reader
  core/abi/provider-hta
  core/abi/value-provider-filesystem
  core/abi/value-provider-filesystem-ffi
  packaging/providers/value/provider-manifest.json
  packaging/providers/value/object-backend-lock.json
  packaging/providers/blob/provider-lock.json
  packaging/providers/blob/provider-manifest.published.json
  packaging/hara-value-revision
)

git -C "$repository" archive --format=tar "$hoplite_commit" "${hoplite_payload[@]}" \
  | tar -xf - -C "$hoplite_source"
git -C "$hara_repository" archive --format=tar "$hara_commit" \
  LICENSE core/rust/abi core/rust/hta-codec \
  | tar -xf - -C "$hara_source"

cp "$hoplite_source/LICENSE" "$package_root/LICENSE"
cp "$hara_source/LICENSE" "$package_root/hara/LICENSE"
cp "$hoplite_source/packaging/providers/value/provider-manifest.json" \
  "$package_root/provider-manifest.json"
cp "$hoplite_source/packaging/providers/value/object-backend-lock.json" \
  "$package_root/object-backend-lock.json"
cp "$hoplite_source/packaging/providers/blob/provider-lock.json" \
  "$package_root/backend/provider-lock.json"
cp "$hoplite_source/packaging/providers/blob/provider-manifest.published.json" \
  "$package_root/backend/provider-manifest.published.json"

for crate in \
  blob-store \
  blob-filesystem-reader \
  provider-hta \
  value-provider-filesystem \
  value-provider-filesystem-ffi; do
  cp -R "$hoplite_source/core/abi/$crate" \
    "$package_root/hoplite/core/abi/$crate"
done
cp -R "$hara_source/core/rust/abi" "$package_root/hara/core/rust/abi"
cp -R "$hara_source/core/rust/hta-codec" \
  "$package_root/hara/core/rust/hta-codec"

printf '%s\n' 'hoplite.provider-source/0-alpha' > "$package_root/PACKAGE_FORMAT"
printf '%s\n' "$version" > "$package_root/PACKAGE_VERSION"
printf '%s\n' "$hoplite_commit" > "$package_root/SOURCE_REVISION"
printf '%s\n' "$hara_commit" > "$package_root/HARA_REVISION"
cat > "$package_root/README.md" <<EOF
# Hoplite value provider $version

This is the deterministic source artifact for the separately installed
\`hoplite.value\` filesystem adapter.

Hoplite source revision: \`$hoplite_commit\`
Hara decoder revision: \`$hara_commit\`

The artifact contains the exact object-backend lock, shared immutable object
reader, canonical Hara HTA decoder and value-provider C boundary. Build and test
it with:

\`\`\`sh
cargo test \\
  --manifest-path hoplite/core/abi/value-provider-filesystem-ffi/Cargo.toml \\
  --locked
cargo build \\
  --manifest-path hoplite/core/abi/value-provider-filesystem-ffi/Cargo.toml \\
  --release --locked
\`\`\`

Provider selection, the object root, backend identity and decoder revision are
trusted distribution inputs and never enter request or HAL values.
EOF

find "$package_root" -type d -exec chmod 0755 {} +
find "$package_root" -type f -exec chmod 0644 {} +

(
  cd "$package_root"
  while IFS= read -r -d '' path; do
    sha256sum "$path"
  done < <(find . -type f ! -name FILES.sha256 -print0 | LC_ALL=C sort -z)
) > "$package_root/FILES.sha256"
chmod 0644 "$package_root/FILES.sha256"

mkdir -p "$(dirname "$output")"
archive="$temporary/archive.tar.gz"
LC_ALL=C tar \
  --sort=name \
  --format=ustar \
  --mtime='@0' \
  --owner=0 \
  --group=0 \
  --numeric-owner \
  --mode='u+rwX,go+rX,go-w' \
  -C "$temporary" \
  -cf - "$package_name" \
  | gzip -n -9 > "$archive"

cp "$archive" "$output"
digest=$(sha256sum "$output" | awk '{print $1}')
# The sidecar describes the canonical package identity, not the caller's
# temporary output path, so independent builds produce identical sidecars.
printf '%s  %s\n' "$digest" "$artifact_name" > "$output.sha256"
printf 'sha256:%s\n' "$digest"
