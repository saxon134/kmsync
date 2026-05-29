#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DIST_DIR="${ROOT_DIR}/dist/macos"
STAGING_DIR="${DIST_DIR}/staging"
PKG_ROOT="${STAGING_DIR}/pkg-root"
SCRIPTS_DIR="${STAGING_DIR}/scripts"
IDENTIFIER="com.kmsync.mvp"
LAUNCH_AGENT_ID="com.kmsync.mvp"
VERSION="$(grep -m1 '^version' "${ROOT_DIR}/crates/kmsync/Cargo.toml" 2>/dev/null | sed -E 's/.*"([^"]+)".*/\1/')"
CODESIGN_IDENTITY="${CODESIGN_IDENTITY:-}"
PKG_SIGN_IDENTITY="${PKG_SIGN_IDENTITY:-}"
APPLE_ID="${APPLE_ID:-}"
APPLE_TEAM_ID="${APPLE_TEAM_ID:-}"
APPLE_APP_SPECIFIC_PASSWORD="${APPLE_APP_SPECIFIC_PASSWORD:-}"

if [[ -z "${VERSION}" || "${VERSION}" == *"workspace"* ]]; then
  VERSION="$(grep -m1 '^version' "${ROOT_DIR}/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/')"
fi

sign_binary_if_configured() {
  local binary="$1"
  if [[ -z "${CODESIGN_IDENTITY}" ]]; then
    echo "CODESIGN_IDENTITY not set; leaving ${binary} unsigned"
    return 0
  fi

  codesign \
    --force \
    --timestamp \
    --options runtime \
    --sign "${CODESIGN_IDENTITY}" \
    "${binary}"
}

sign_pkg_if_configured() {
  local pkg="$1"
  if [[ -z "${PKG_SIGN_IDENTITY}" ]]; then
    echo "PKG_SIGN_IDENTITY not set; leaving ${pkg} unsigned"
    return 0
  fi

  local signed_pkg="${pkg%.pkg}-signed.pkg"
  productsign --sign "${PKG_SIGN_IDENTITY}" "${pkg}" "${signed_pkg}"
  mv "${signed_pkg}" "${pkg}"
}

notarize_pkg_if_configured() {
  local pkg="$1"
  if [[ -z "${APPLE_ID}" || -z "${APPLE_TEAM_ID}" || -z "${APPLE_APP_SPECIFIC_PASSWORD}" ]]; then
    echo "APPLE_ID, APPLE_TEAM_ID, or APPLE_APP_SPECIFIC_PASSWORD missing; skipping notarization"
    return 0
  fi

  xcrun notarytool submit \
    "${pkg}" \
    --apple-id "${APPLE_ID}" \
    --team-id "${APPLE_TEAM_ID}" \
    --password "${APPLE_APP_SPECIFIC_PASSWORD}" \
    --wait
  xcrun stapler staple "${pkg}"
}

rm -rf "${DIST_DIR}"
mkdir -p \
  "${PKG_ROOT}/usr/local/bin" \
  "${PKG_ROOT}/usr/local/share/kmsync/configs" \
  "${PKG_ROOT}/usr/local/share/kmsync/docs" \
  "${PKG_ROOT}/Library/LaunchAgents" \
  "${SCRIPTS_DIR}"

if command -v lipo >/dev/null 2>&1 && rustup target list --installed | grep -q '^x86_64-apple-darwin$'; then
  cargo build --release -p kmsync --target aarch64-apple-darwin
  cargo build --release -p kmsync --target x86_64-apple-darwin
  lipo -create \
    "${ROOT_DIR}/target/aarch64-apple-darwin/release/kmsync" \
    "${ROOT_DIR}/target/x86_64-apple-darwin/release/kmsync" \
    -output "${STAGING_DIR}/kmsync"
else
  cargo build --release -p kmsync
  cp "${ROOT_DIR}/target/release/kmsync" "${STAGING_DIR}/kmsync"
fi

install -m 0755 "${STAGING_DIR}/kmsync" "${PKG_ROOT}/usr/local/bin/kmsync"
sign_binary_if_configured "${PKG_ROOT}/usr/local/bin/kmsync"
install -m 0644 "${ROOT_DIR}/configs/mac-to-windows.profile.json" "${PKG_ROOT}/usr/local/share/kmsync/configs/mac-to-windows.profile.json"
install -m 0644 "${ROOT_DIR}/configs/windows-to-mac.profile.json" "${PKG_ROOT}/usr/local/share/kmsync/configs/windows-to-mac.profile.json"
install -m 0644 "${ROOT_DIR}/configs/daemon.example.json" "${PKG_ROOT}/usr/local/share/kmsync/configs/daemon.example.json"
install -m 0644 "${ROOT_DIR}/docs/USER_GUIDE.md" "${PKG_ROOT}/usr/local/share/kmsync/docs/USER_GUIDE.md"

cat > "${PKG_ROOT}/Library/LaunchAgents/${LAUNCH_AGENT_ID}.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${LAUNCH_AGENT_ID}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/kmsync</string>
    <string>core-service</string>
    <string>/usr/local/share/kmsync/configs/daemon.example.json</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>/tmp/kmsync.out.log</string>
  <key>StandardErrorPath</key>
  <string>/tmp/kmsync.err.log</string>
</dict>
</plist>
PLIST

cat > "${PKG_ROOT}/usr/local/share/kmsync/uninstall-macos.sh" <<SCRIPT
#!/usr/bin/env bash
set -euo pipefail

launchctl bootout "gui/\$(id -u)" "/Library/LaunchAgents/${LAUNCH_AGENT_ID}.plist" 2>/dev/null || true
rm -f "/Library/LaunchAgents/${LAUNCH_AGENT_ID}.plist"
rm -f /usr/local/bin/kmsync
rm -rf /usr/local/share/kmsync
echo "KMSync files removed."
SCRIPT

chmod 0755 "${PKG_ROOT}/usr/local/share/kmsync/uninstall-macos.sh"

cat > "${SCRIPTS_DIR}/postinstall" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail

echo "KMSync installed to /usr/local/bin/kmsync"
echo "LaunchAgent installed to /Library/LaunchAgents/com.kmsync.mvp.plist for login startup"
echo "Permission guide installed to /usr/local/share/kmsync/docs/USER_GUIDE.md"
echo "Uninstall cleanup script installed to /usr/local/share/kmsync/uninstall-macos.sh"
echo "Run: /usr/local/bin/kmsync info"
exit 0
SCRIPT

chmod 0755 "${SCRIPTS_DIR}/postinstall"

PKG_PATH="${DIST_DIR}/kmsync-${VERSION}-macos.pkg"

pkgbuild \
  --root "${PKG_ROOT}" \
  --scripts "${SCRIPTS_DIR}" \
  --identifier "${IDENTIFIER}" \
  --version "${VERSION}" \
  --install-location "/" \
  "${PKG_PATH}"

sign_pkg_if_configured "${PKG_PATH}"
notarize_pkg_if_configured "${PKG_PATH}"

echo "Created ${PKG_PATH}"
