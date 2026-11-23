#!/usr/bin/env bash
# Build AIMyChats Dev.app by default and sign it with the local self-signed identity.
#
# Why: macOS binds Full Disk Access to the app's designated requirement. With an ad-hoc
# signature that requirement is the per-build CDHash, so every rebuild would need a new
# grant. A self-signed identity makes it `identifier "local.wechat.devifybridge" and
# certificate leaf = H"…"`, which survives rebuilds.
#
# We sign here instead of relying on `bundle.macOS.signingIdentity` because codesign only
# finds the private key in its own keychain when it is passed `--keychain`; the bundler
# does not pass it, which fails with errSecInternalComponent.
#
# Installing into /Applications replaces only the selected variant.
# Use --production explicitly for a production build; --no-install only creates artifacts.
set -euo pipefail
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"

# Development is the safe default. Replacing the production installation is explicit.
CHANNEL="dev"
INSTALL=true
for arg in "$@"; do
  case "$arg" in
    --dev) CHANNEL="dev" ;;
    --production) CHANNEL="production" ;;
    --no-install) INSTALL=false ;;
    *) echo "Usage: $0 [--dev|--production] [--no-install]" >&2; exit 2 ;;
  esac
done
PRODUCT="AIMyChats"
BUNDLE_ID="local.wechat.devifybridge"
CONFIG_ARGS=(--config src-tauri/tauri.conf.json)
if [ "$CHANNEL" = "dev" ]; then
  PRODUCT="AIMyChats Dev"
  BUNDLE_ID="local.wechat.devifybridge.dev"
  CONFIG_ARGS=(--config src-tauri/tauri.dev-app.conf.json)
fi
IDENTITY="${AIMYCHATS_SIGNING_IDENTITY:-AIMyChats Local Signing}"
KEYCHAIN="${AIMYCHATS_SIGNING_KEYCHAIN:-$HOME/Library/Keychains/signing.keychain-db}"
PASS_FILE="$HOME/.config/aimychats-signing/keychain.pass"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/src-tauri/target/release/bundle/macos/$PRODUCT.app"
DEST="/Applications/$PRODUCT.app"
VERSION="$(node -p "JSON.parse(require('fs').readFileSync(process.argv[1], 'utf8')).version" "$ROOT/src-tauri/tauri.conf.json")"
ARCH="$(uname -m)"
if [ "$ARCH" = "arm64" ]; then ARCH="aarch64"; fi
DMG="$ROOT/src-tauri/target/release/bundle/dmg/${PRODUCT}_${VERSION}_${ARCH}.dmg"
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

cd "$ROOT"

SIGN_ARGS=()
IDENTITY_STORE_ARGS=()
if [ -f "$KEYCHAIN" ]; then
  if [ -f "$PASS_FILE" ]; then security unlock-keychain -p "$(cat "$PASS_FILE")" "$KEYCHAIN"; fi
  SIGN_ARGS=(--keychain "$KEYCHAIN")
  IDENTITY_STORE_ARGS=("$KEYCHAIN")
fi
if [ ! -f "$KEYCHAIN" ] && [ -z "${AIMYCHATS_SIGNING_IDENTITY:-}" ]; then
  # A valid installed Developer ID is stable across builds too. Never choose
  # arbitrarily when more than one team/certificate is available.
  AVAILABLE=$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application:.*\)".*/\1/p')
  if [ "$(printf '%s\n' "$AVAILABLE" | sed '/^$/d' | wc -l | tr -d ' ')" = 1 ]; then IDENTITY="$AVAILABLE"; fi
fi
# The configured identity may be the repository's self-signed CI certificate.
# `-v` filters that identity out; keep it only for automatic Developer ID selection.
if ! security find-identity -p codesigning ${IDENTITY_STORE_ARGS[@]+"${IDENTITY_STORE_ARGS[@]}"} | grep -Fq "$IDENTITY"; then
  echo "找不到签名身份：${IDENTITY}。请设置 AIMYCHATS_SIGNING_IDENTITY（以及可选的 AIMYCHATS_SIGNING_KEYCHAIN）。" >&2
  exit 1
fi
SIGN_ARGS=(--sign "$IDENTITY" ${SIGN_ARGS[@]+"${SIGN_ARGS[@]}"})

# caffeinate: a sleeping Mac locks the signing keychain mid-build.
# `--bundles app` skips the bundler's DMG: it is built from the ad-hoc app and is
# replaced by the signed one below anyway, so building it twice is pure waste.
CI=true caffeinate -i -m npm run tauri build -- --bundles app "${CONFIG_ARGS[@]}"
ACTUAL_ID=$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$APP/Contents/Info.plist")
if [ "$ACTUAL_ID" != "$BUNDLE_ID" ]; then
  echo "Bundle identity mismatch; refusing installation: $ACTUAL_ID" >&2
  exit 1
fi

# A copied bundle can have correct plist metadata but a stale executable from another variant.
"$APP/Contents/MacOS/wechat-devify-bridge" --build-info | node -e '
let input="";process.stdin.on("data",chunk=>input+=chunk);process.stdin.on("end",()=>{
 const info=JSON.parse(input);if(info.identifier!==process.argv[1])throw Error("Executable build identity mismatch");
});' "$BUNDLE_ID"

for binary in "$APP"/Contents/MacOS/*; do
  codesign --force "${SIGN_ARGS[@]}" --timestamp=none "$binary"
done
codesign --force "${SIGN_ARGS[@]}" --timestamp=none "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

# The bundler wrote the DMG from the ad-hoc app; rebuild it from the signed one.
# Stage the app next to an /Applications symlink so the DMG supports drag-to-install.
if [ -d "$APP" ]; then
  STAGING="$(mktemp -d "${TMPDIR:-/tmp}/aimychats-dmg.XXXXXX")"
  trap 'rm -rf "$STAGING"' EXIT
  ditto "$APP" "$STAGING/$PRODUCT.app"
  ln -s /Applications "$STAGING/Applications"
  # `tauri build --bundles app` does not create the dmg directory, and hdiutil does
  # not create it either: a clean checkout fails here without this.
  mkdir -p "$(dirname "$DMG")"
  rm -f "$DMG"
  hdiutil create -quiet -volname "$PRODUCT" -srcfolder "$STAGING" -ov -format UDZO "$DMG"
  rm -rf "$STAGING"
  trap - EXIT
fi

echo
codesign -dvvv "$APP" 2>&1 | grep -E "Authority|Identifier=|CDHash=" || true
codesign -d -r- "$APP" 2>&1 | tail -1

if [ "$INSTALL" = true ]; then
  # Only quit this variant; never address an app by the shared executable name.
  osascript -e "if application id \"$BUNDLE_ID\" is running then tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
  sleep 2
  if [ "$(osascript -e "application id \"$BUNDLE_ID\" is running")" != "false" ]; then
    echo "$PRODUCT 尚未退出，已停止替换。请退出该版本后重新运行安装命令。" >&2
    exit 1
  fi
  # The product is selected above from two fixed values, never user-supplied paths.
  STAGED_APP="/Applications/.$PRODUCT-install-$$.app"
  ditto "$APP" "$STAGED_APP"
  codesign --verify --deep --strict "$STAGED_APP"
  rm -rf "$DEST"
  mv "$STAGED_APP" "$DEST"
  "$LSREGISTER" -f "$DEST"
  echo "已安装到 $DEST"
fi
