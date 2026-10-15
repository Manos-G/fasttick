#!/usr/bin/env bash
# Builds an AppImage of FastTick: target/appimage/FastTick-<version>-<arch>.AppImage
#
# Requirements: cargo and a C/C++ tool chain (see README.md), and
# appimagetool. If appimagetool is not on the PATH and APPIMAGETOOL is not
# set, the script downloads it to target/appimage/tools.
#
# The AppImage contains the fasttick binary, the desktop entry, the icon and
# the licence files. It uses the C library and the graphics libraries of the
# host system.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
ARCH=$(uname -m)
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
OUT="$ROOT/target/appimage"
APPDIR="$OUT/FastTick.AppDir"

cargo build --release --locked -p fasttick-app
if [ ! -f THIRD_PARTY_LICENSES.md ]; then
    echo "THIRD_PARTY_LICENSES.md is missing: run 'cargo about generate about.hbs -o THIRD_PARTY_LICENSES.md'" >&2
    exit 1
fi

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" \
    "$APPDIR/usr/share/applications" \
    "$APPDIR/usr/share/icons/hicolor/256x256/apps" \
    "$APPDIR/usr/share/icons/hicolor/scalable/apps" \
    "$APPDIR/usr/share/doc/fasttick"
install -m 755 target/release/fasttick "$APPDIR/usr/bin/fasttick"
install -m 644 packaging/fasttick.desktop "$APPDIR/fasttick.desktop"
install -m 644 packaging/fasttick.desktop "$APPDIR/usr/share/applications/fasttick.desktop"
install -m 644 packaging/fasttick-256.png "$APPDIR/fasttick.png"
install -m 644 packaging/fasttick-256.png "$APPDIR/usr/share/icons/hicolor/256x256/apps/fasttick.png"
install -m 644 packaging/fasttick.svg "$APPDIR/usr/share/icons/hicolor/scalable/apps/fasttick.svg"
install -m 644 LICENSE README.md THIRD_PARTY_LICENSES.md "$APPDIR/usr/share/doc/fasttick/"
cat > "$APPDIR/AppRun" <<'APPRUN'
#!/bin/sh
HERE=$(dirname "$(readlink -f "$0")")
exec "$HERE/usr/bin/fasttick" "$@"
APPRUN
chmod 755 "$APPDIR/AppRun"

TOOL=${APPIMAGETOOL:-$(command -v appimagetool || true)}
if [ -z "$TOOL" ]; then
    TOOL="$OUT/tools/appimagetool-$ARCH.AppImage"
    if [ ! -x "$TOOL" ]; then
        mkdir -p "$OUT/tools"
        curl -fL -o "$TOOL" \
            "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
        chmod 755 "$TOOL"
    fi
fi

# Containers often have no FUSE: the tool can extract itself and run.
APPIMAGE_EXTRACT_AND_RUN=1 ARCH="$ARCH" "$TOOL" --no-appstream "$APPDIR" \
    "$OUT/FastTick-$VERSION-$ARCH.AppImage"
echo "Made $OUT/FastTick-$VERSION-$ARCH.AppImage"
