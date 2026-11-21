#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
PKG_DIR="${ROOT_DIR}/packaging/appimage"
DIST_DIR="${ROOT_DIR}/dist"
APPDIR="${DIST_DIR}/AppDir"
TOOLS_DIR="${ROOT_DIR}/target/appimage-tools"

APP_ID="io.github.cyanide.VirtualCam"
ARCH="${ARCH:-$(uname -m)}"
SKIP_BUILD=0
BUNDLE_LIBS=0

for arg in "$@"; do
    case "$arg" in
        --skip-build) SKIP_BUILD=1 ;;
        --bundle-libs) BUNDLE_LIBS=1 ;;
        -h|--help)
            cat <<'EOF'
Usage: scripts/build-appimage.sh [--skip-build] [--bundle-libs]

Builds dist/Cyanide-<arch>.AppImage from the release binary.
  --skip-build   Reuse an existing target/release/cyanide (or $CYANIDE_BIN)
  --bundle-libs  Use linuxdeploy (if installed) to bundle non-system shared libraries
EOF
            exit 0
            ;;
        *)
            echo "error: unknown option '$arg'" >&2
            exit 1
            ;;
    esac
done

BIN_PATH="${CYANIDE_BIN:-${ROOT_DIR}/target/release/cyanide}"
if [ "$SKIP_BUILD" -eq 0 ]; then
    echo "==> Building release binary..."
    cargo build --manifest-path "${ROOT_DIR}/Cargo.toml" --release --locked
fi

if [ ! -x "$BIN_PATH" ]; then
    echo "error: release binary not found at ${BIN_PATH}" >&2
    exit 1
fi

echo "==> Preparing AppDir at ${APPDIR}..."
rm -rf "${APPDIR}"
mkdir -p \
    "${APPDIR}/usr/bin" \
    "${APPDIR}/usr/share/applications" \
    "${APPDIR}/usr/share/icons/hicolor/256x256/apps" \
    "${APPDIR}/usr/share/icons/hicolor/scalable/apps"

install -Dm755 "${BIN_PATH}" "${APPDIR}/usr/bin/cyanide"
if command -v strip >/dev/null 2>&1; then
    strip --strip-unneeded "${APPDIR}/usr/bin/cyanide" || true
fi

install -Dm755 "${PKG_DIR}/AppRun" "${APPDIR}/AppRun"
install -Dm644 "${PKG_DIR}/${APP_ID}.desktop" "${APPDIR}/${APP_ID}.desktop"
install -Dm644 "${PKG_DIR}/${APP_ID}.desktop" "${APPDIR}/usr/share/applications/${APP_ID}.desktop"
install -Dm644 "${PKG_DIR}/${APP_ID}.png" "${APPDIR}/${APP_ID}.png"
install -Dm644 "${PKG_DIR}/${APP_ID}.png" "${APPDIR}/.DirIcon"
install -Dm644 "${PKG_DIR}/${APP_ID}.png" "${APPDIR}/usr/share/icons/hicolor/256x256/apps/${APP_ID}.png"
install -Dm644 "${PKG_DIR}/${APP_ID}.svg" "${APPDIR}/usr/share/icons/hicolor/scalable/apps/${APP_ID}.svg"

if [ "$BUNDLE_LIBS" -eq 1 ]; then
    if ! command -v linuxdeploy >/dev/null 2>&1; then
        echo "error: --bundle-libs requires 'linuxdeploy' in PATH" >&2
        exit 1
    fi
    echo "==> Bundling libraries via linuxdeploy..."
    linuxdeploy --appdir "${APPDIR}" \
        --executable "${APPDIR}/usr/bin/cyanide" \
        --desktop-file "${APPDIR}/${APP_ID}.desktop" \
        --icon-file "${APPDIR}/${APP_ID}.png"
    # Restore our custom AppRun so CLI arguments and GStreamer paths work cleanly.
    install -Dm755 "${PKG_DIR}/AppRun" "${APPDIR}/AppRun"
fi

resolve_appimagetool() {
    if [ -n "${APPIMAGETOOL:-}" ]; then
        echo "${APPIMAGETOOL}"
        return
    fi
    if command -v appimagetool >/dev/null 2>&1; then
        command -v appimagetool
        return
    fi

    local cached="${TOOLS_DIR}/appimagetool-${ARCH}.AppImage"
    if [ ! -x "$cached" ]; then
        mkdir -p "${TOOLS_DIR}"
        local url="https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-${ARCH}.AppImage"
        echo "==> Downloading appimagetool (${ARCH})..." >&2
        if command -v curl >/dev/null 2>&1; then
            curl -fL --retry 3 -o "${cached}.tmp" "${url}"
        elif command -v wget >/dev/null 2>&1; then
            wget -O "${cached}.tmp" "${url}"
        else
            echo "error: appimagetool not found and neither curl nor wget is installed" >&2
            exit 1
        fi
        chmod +x "${cached}.tmp"
        mv "${cached}.tmp" "${cached}"
    fi
    echo "${cached}"
}

TOOL="$(resolve_appimagetool)"
OUT_FILE="${DIST_DIR}/Cyanide-${ARCH}.AppImage"
rm -f "${OUT_FILE}"

echo "==> Packaging ${OUT_FILE}..."
ARCH="${ARCH}" APPIMAGE_EXTRACT_AND_RUN=1 "${TOOL}" --no-appstream "${APPDIR}" "${OUT_FILE}"
chmod +x "${OUT_FILE}"

echo "==> Done: ${OUT_FILE} ($(du -h "${OUT_FILE}" | cut -f1))"
