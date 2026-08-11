#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
usage: build-blob-provider-artifact.sh VERSION REVISION OUTPUT

Build a deterministic target-independent Hoplite blob-provider source artifact
from one exact repository commit. OUTPUT receives a gzip-compressed ustar
archive and OUTPUT.sha256 receives its lowercase SHA-256 checksum.
EOF
}

if [[ $# -ne 3 ]]; then
  usage >&2
  exit 2
fi

version=$1
revision=$2
output=$3

if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][A-Za-z0-9._-]+)?$ ]]; then
  echo "invalid provider package version: $version" >&2
  exit 2
fi

for command in git tar gzip sha256sum find sort awk; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "required command is unavailable: $command" >&2
    exit 2
  fi
done

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repository=$(cd "$script_dir/../.." && pwd)
commit=$(git -C "$repository" rev-parse --verify "$revision^{commit}")
if [[ ! $commit =~ ^[0-9a-f]{40}$ ]]; then
  echo "revision did not resolve to a full Git commit: $revision" >&2
  exit 2
fi

package_name="hoplite-blob-provider-$version"
artifact_name="$package_name.tar.gz"
temporary=$(mktemp -d "${TMPDIR:-/tmp}/hoplite-blob-provider.XXXXXX")
trap 'rm -rf "$temporary"' EXIT
source_tree="$temporary/source"
package_root="$temporary/$package_name"
mkdir -p "$source_tree" "$package_root/abi"

payload_paths=(
  LICENSE
  core/abi/blob-store
  core/abi/blob-filesystem-reader
  core/abi/blob-store-filesystem
  core/abi/blob-store-provider
  core/abi/blob-store-provider-ffi
  core/abi/provider-hta
  packaging/providers/blob/provider-manifest.json
)

git -C "$repository" archive --format=tar "$commit" "${payload_paths[@]}" \
  | tar -xf - -C "$source_tree"

cp "$source_tree/LICENSE" "$package_root/LICENSE"
cp "$source_tree/packaging/providers/blob/provider-manifest.json" \
  "$package_root/provider-manifest.json"
for crate in \
  blob-store \
  blob-filesystem-reader \
  blob-store-filesystem \
  blob-store-provider \
  blob-store-provider-ffi \
  provider-hta; do
  cp -R "$source_tree/core/abi/$crate" "$package_root/abi/$crate"
done

printf '%s\n' 'hoplite.provider-source/0-alpha' > "$package_root/PACKAGE_FORMAT"
printf '%s\n' "$version" > "$package_root/PACKAGE_VERSION"
printf '%s\n' "$commit" > "$package_root/SOURCE_REVISION"
cat > "$package_root/README.md" <<EOF
# Hoplite blob provider $version

This is the deterministic source artifact for the separately composed
\`hoplite.blob\` filesystem provider.

Source revision: \`$commit\`

Build and test the canonical C provider boundary with:

\`\`\`sh
cargo test --manifest-path abi/blob-store-provider-ffi/Cargo.toml --locked
cargo build --manifest-path abi/blob-store-provider-ffi/Cargo.toml --release --locked
\`\`\`

The provider contract, driver, paths and credentials remain trusted
distribution configuration and never enter request or HAL values.
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
