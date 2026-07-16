#!/bin/sh
set -eu

REPO="Skiley/runfile"
INSTALL_DIR="${RUNFILE_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${1:-latest}"

case "$(uname -s)" in
  Linux*)  os="unknown-linux-musl" ;;
  Darwin*) os="apple-darwin" ;;
  *) echo "runfile: unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
  x86_64|amd64)  arch="x86_64" ;;
  aarch64|arm64) arch="aarch64" ;;
  *) echo "runfile: unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

target="${arch}-${os}"
archive="runfile-cli-${target}.tar.xz"

if [ "$VERSION" = "latest" ]; then
  url="https://github.com/${REPO}/releases/latest/download/${archive}"
else
  url="https://github.com/${REPO}/releases/download/${VERSION}/${archive}"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $archive..."
curl -fsSL "$url" -o "$tmp/$archive"
tar -xJf "$tmp/$archive" -C "$tmp"

mkdir -p "$INSTALL_DIR"
mv "$tmp/runfile-cli-${target}/run" "$INSTALL_DIR/run"
chmod +x "$INSTALL_DIR/run"

echo "Installed run to $INSTALL_DIR/run"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo
    echo "Add $INSTALL_DIR to your PATH:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac
