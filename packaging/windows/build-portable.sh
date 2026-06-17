#!/usr/bin/env bash
set -euo pipefail

VERSION="${VERSION:-0.1.0}"
TARGET="${TARGET:-x86_64-pc-windows-gnu}"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DIST_DIR="${ROOT_DIR}/dist/windows"
PORTABLE_NAME="kmsync-${VERSION}-windows-x64-portable"
PORTABLE_DIR="${DIST_DIR}/${PORTABLE_NAME}"
ZIP_PATH="${DIST_DIR}/${PORTABLE_NAME}.zip"

cd "${ROOT_DIR}"
mkdir -p "${DIST_DIR}"

if [[ "${TARGET}" == "x86_64-pc-windows-gnu" ]]; then
  export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="${CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER:-x86_64-w64-mingw32-gcc}"
fi

cargo build --release -p kmsync --target "${TARGET}"

rm -rf "${PORTABLE_DIR}" "${ZIP_PATH}" "${ZIP_PATH}.tmp"
mkdir -p "${PORTABLE_DIR}/configs" "${PORTABLE_DIR}/docs"

cp "${ROOT_DIR}/target/${TARGET}/release/kmsync.exe" "${PORTABLE_DIR}/kmsync.exe"
cp "${ROOT_DIR}/configs/daemon.example.json" "${PORTABLE_DIR}/configs/daemon.example.json"
cp "${ROOT_DIR}/configs/mac-to-windows.profile.json" "${PORTABLE_DIR}/configs/mac-to-windows.profile.json"
cp "${ROOT_DIR}/configs/windows-to-mac.profile.json" "${PORTABLE_DIR}/configs/windows-to-mac.profile.json"
cp "${ROOT_DIR}/docs/USER_GUIDE.md" "${PORTABLE_DIR}/docs/USER_GUIDE.md"
cp "${ROOT_DIR}/packaging/windows/enable-firewall.cmd" "${PORTABLE_DIR}/enable-firewall.cmd"
cp "${ROOT_DIR}/packaging/windows/start-core-service.cmd" "${PORTABLE_DIR}/start-core-service.cmd"

(
  cd "${DIST_DIR}"
  zip -qr "${ZIP_PATH}.tmp" "${PORTABLE_NAME}"
)
mv "${ZIP_PATH}.tmp" "${ZIP_PATH}"

echo "Created ${ZIP_PATH}"
