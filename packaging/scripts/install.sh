#!/bin/sh
set -eu

repository="greenways-ai/hoplite"
api="https://api.github.com/repos/${repository}/releases/latest"

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux) os="unknown-linux-gnu" ;;
  *)
    echo "Hoplite supports macOS and Linux." >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  arm64|aarch64) arch="aarch64" ;;
  x86_64|amd64) arch="x86_64" ;;
  *)
    echo "Unsupported architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

release_json="$(curl -fsSL "$api")"
tag="$(printf '%s' "$release_json" | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"

if [ -z "$tag" ]; then
  echo "Could not resolve the latest Hoplite release." >&2
  exit 1
fi

if [ -n "${HOPLITE_INSTALL_DIR:-}" ]; then
  install_dir="$HOPLITE_INSTALL_DIR"
elif [ "$(id -u)" -eq 0 ]; then
  install_dir="/usr/local/bin"
else
  install_dir="${HOME}/.local/bin"
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/hoplite.XXXXXX")"
trap 'rm -rf "$work"' EXIT INT TERM
mkdir -p "$install_dir"

cli_asset="hoplite-${tag}-${arch}-${os}"
cli_url="https://github.com/${repository}/releases/download/${tag}/${cli_asset}"

echo "Downloading Hoplite ${tag} for ${arch}-${os}..."
curl -fL --retry 3 "$cli_url" -o "$work/hoplite"
chmod 0755 "$work/hoplite"
install -m 0755 "$work/hoplite" "${install_dir}/hoplite"
echo "Installed ${install_dir}/hoplite"
"${install_dir}/hoplite" version

server_asset="hoplite-server-${tag}-${arch}-${os}"
server_url="https://github.com/${repository}/releases/download/${tag}/${server_asset}"
if curl -fsSL --retry 3 "$server_url" -o "$work/hoplite-server"; then
  chmod 0755 "$work/hoplite-server"
  install -m 0755 "$work/hoplite-server" "${install_dir}/hoplite-server"
  echo "Installed ${install_dir}/hoplite-server"
  "${install_dir}/hoplite-server" version
else
  echo "This release predates the separate hoplite-server artifact; the Hoplite CLI was installed normally."
fi

case ":${PATH}:" in
  *":${install_dir}:"*) ;;
  *)
    echo
    echo "Add Hoplite to your PATH:"
    echo "  export PATH=\"${install_dir}:\$PATH\""
    ;;
esac
