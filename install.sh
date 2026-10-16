#!/bin/sh
# Installs or updates FastTick on Ubuntu, Debian or Raspberry Pi OS (64-bit)
# from the Debian package of a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/Manos-G/fasttick/main/install.sh | sh
#
# The script:
#   1. finds the latest release (or the release in FASTTICK_VERSION, for
#      example FASTTICK_VERSION=v0.1.0);
#   2. downloads the package for this computer (amd64 or arm64) and the
#      file SHA256SUMS;
#   3. checks the SHA-256 checksum of the package;
#   4. installs the package with apt-get (with sudo if you are not root),
#      so apt-get also installs the libraries that FastTick needs.
#
# To remove FastTick: sudo apt-get remove fasttick
set -eu

REPO="Manos-G/fasttick"

fail() {
    echo "FastTick install: $*" >&2
    exit 1
}

need() {
    command -v "$1" > /dev/null 2>&1 || fail "the program '$1' is necessary. $2"
}

need curl "Install it with: sudo apt-get install curl"
need dpkg "This script is for Ubuntu and Debian. For other systems, use the AppImage of the release."
need apt-get "This script is for Ubuntu and Debian. For other systems, use the AppImage of the release."
need sha256sum "Install it with: sudo apt-get install coreutils"

ARCH=$(dpkg --print-architecture)
case "$ARCH" in
    amd64 | arm64) ;;
    *) fail "there is no package for this computer ($ARCH). Packages are for amd64 and arm64." ;;
esac

if [ -n "${FASTTICK_VERSION:-}" ]; then
    TAG=$FASTTICK_VERSION
else
    TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" |
        sed -n 's/^ *"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1) ||
        fail "cannot get the latest release from GitHub."
    [ -n "$TAG" ] || fail "cannot find the latest release on GitHub."
fi
VERSION=${TAG#v}
DEB="fasttick_${VERSION}_${ARCH}.deb"
BASE="https://github.com/$REPO/releases/download/$TAG"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM
# apt-get reads the package as the user _apt.
chmod 755 "$TMP"

echo "Downloading FastTick $VERSION ($ARCH)..."
curl -fL --progress-bar -o "$TMP/$DEB" "$BASE/$DEB" ||
    fail "cannot download $BASE/$DEB"
curl -fsSL -o "$TMP/SHA256SUMS" "$BASE/SHA256SUMS" ||
    fail "cannot download $BASE/SHA256SUMS"
chmod 644 "$TMP/$DEB"

(
    cd "$TMP"
    grep " $DEB\$" SHA256SUMS > "$DEB.sha256" || fail "SHA256SUMS has no line for $DEB."
    sha256sum -c "$DEB.sha256" > /dev/null || fail "the checksum of $DEB is not correct."
)
echo "The checksum is correct."

SUDO=""
if [ "$(id -u)" -ne 0 ]; then
    need sudo "Run the script as root."
    SUDO=sudo
fi
$SUDO apt-get install -y "$TMP/$DEB"

echo
echo "FastTick $VERSION is installed. Start it from the application menu, or type: fasttick"
