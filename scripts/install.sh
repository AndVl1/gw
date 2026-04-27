#!/usr/bin/env sh
# Install gw — Gradle output filter for AI coding agents.
#
# Usage:
#   curl -sSL https://raw.githubusercontent.com/AndVl1/gw/main/scripts/install.sh | sh
#   curl -sSL https://raw.githubusercontent.com/AndVl1/gw/main/scripts/install.sh | sh -s -- --version v0.2.4
#   curl -sSL https://raw.githubusercontent.com/AndVl1/gw/main/scripts/install.sh | sh -s -- --dir /usr/local/bin
#
# Env:
#   GW_VERSION   Pin version (e.g. v0.2.4). Default: latest GitHub release.
#   GW_INSTALL_DIR  Install dir. Default: $HOME/.local/bin.
#   GW_NO_VERIFY=1  Skip sha256 check (not recommended).

set -eu

REPO="AndVl1/gw"
VERSION="${GW_VERSION:-}"
INSTALL_DIR="${GW_INSTALL_DIR:-$HOME/.local/bin}"
NO_VERIFY="${GW_NO_VERIFY:-0}"

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION="$2"; shift 2 ;;
        --dir) INSTALL_DIR="$2"; shift 2 ;;
        --no-verify) NO_VERIFY=1; shift ;;
        -h|--help)
            grep '^#' "$0" | sed 's/^#//'
            exit 0
            ;;
        *) echo "unknown arg: $1" >&2; exit 1 ;;
    esac
done

err() { echo "gw-install: $*" >&2; exit 1; }
info() { echo "gw-install: $*"; }

need() { command -v "$1" >/dev/null 2>&1 || err "missing required tool: $1"; }
need curl
need tar
need uname

OS=$(uname -s)
ARCH=$(uname -m)

case "$OS" in
    Darwin) os_target="apple-darwin" ;;
    Linux)  os_target="unknown-linux-gnu" ;;
    *) err "unsupported OS: $OS (try install.ps1 on Windows)" ;;
esac

case "$ARCH" in
    x86_64|amd64) arch_target="x86_64" ;;
    arm64|aarch64) arch_target="aarch64" ;;
    *) err "unsupported arch: $ARCH" ;;
esac

TARGET="${arch_target}-${os_target}"

if [ -z "$VERSION" ]; then
    info "resolving latest release..."
    VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep -o '"tag_name": *"[^"]*"' \
        | head -1 \
        | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
    [ -n "$VERSION" ] || err "could not resolve latest version"
fi

VER_NUM="${VERSION#v}"
ARCHIVE="gw-${VER_NUM}-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE}"
SHA_URL="${URL}.sha256"

info "downloading ${ARCHIVE} (${VERSION})..."
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

curl -fsSL "$URL" -o "$TMP/$ARCHIVE" || err "download failed: $URL"

if [ "$NO_VERIFY" != "1" ]; then
    info "verifying sha256..."
    curl -fsSL "$SHA_URL" -o "$TMP/$ARCHIVE.sha256" || err "sha256 download failed"
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$TMP" && sha256sum -c "$ARCHIVE.sha256" >/dev/null) || err "sha256 mismatch"
    elif command -v shasum >/dev/null 2>&1; then
        (cd "$TMP" && shasum -a 256 -c "$ARCHIVE.sha256" >/dev/null) || err "sha256 mismatch"
    else
        info "warn: no sha256 tool found, skipping verification"
    fi
fi

info "extracting..."
tar -C "$TMP" -xzf "$TMP/$ARCHIVE"

mkdir -p "$INSTALL_DIR"
mv "$TMP/gw-${VER_NUM}-${TARGET}/gw" "$INSTALL_DIR/gw"
chmod +x "$INSTALL_DIR/gw"

info "installed: $INSTALL_DIR/gw"

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        echo
        echo "  WARNING: $INSTALL_DIR is not in PATH."
        echo "  Add to your shell config:"
        echo "    export PATH=\"$INSTALL_DIR:\$PATH\""
        ;;
esac

"$INSTALL_DIR/gw" --version
