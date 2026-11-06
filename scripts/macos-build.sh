#!/usr/bin/env bash
# Build AIMyChats.app and sign it with the local self-signed identity.
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
# Installing into /Applications is the default: every build is meant to replace the
# running copy. Pass --no-install to only produce the bundle.
set -euo pipefail

IDENTITY="AIMyChats Local Signing"
KEYCHAIN="$HOME/Library/Keychains/signing.keychain-db"
PASS_FILE="$HOME/.config/aimychats-signing/keychain.pass"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/src-tauri/target/release/bundle/macos/AIMyChats.app"
DMG="$ROOT/src-tauri/target/release/bundle/dmg/AIMyChats_0.0.1_aarch64.dmg"
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

cd "$ROOT"

if [ -f "$PASS_FILE" ]; then
  security unlock-keychain -p "$(cat "$PASS_FILE")" "$KEYCHAIN"
fi
if ! security find-identity -v -p codesigning "$KEYCHAIN" | grep -q "$IDENTITY"; then
  echo "找不到签名身份 \"$IDENTITY\"。先按 README 的「macOS signing」一节创建自签名证书。" >&2
  exit 1
fi

# caffeinate: a sleeping Mac locks the signing keychain mid-build.
CI=true caffeinate -i -m npm run tauri build

for binary in "$APP"/Contents/MacOS/*; do
  codesign --force --sign "$IDENTITY" --keychain "$KEYCHAIN" --timestamp=none "$binary"
done
codesign --force --sign "$IDENTITY" --keychain "$KEYCHAIN" --timestamp=none "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

# The bundler wrote the DMG from the ad-hoc app; rebuild it from the signed one.
if [ -d "$APP" ]; then
  rm -f "$DMG"
  hdiutil create -quiet -volname AIMyChats -srcfolder "$APP" -ov -format UDZO "$DMG"
fi

echo
codesign -dvvv "$APP" 2>&1 | grep -E "Authority|Identifier=|CDHash=" || true
codesign -d -r- "$APP" 2>&1 | tail -1

if [ "${1:-}" != "--no-install" ]; then
  osascript -e 'quit app "AIMyChats"' >/dev/null 2>&1 || true
  sleep 2
  # Replace instead of merging: ditto over an existing bundle can leave stale files.
  rm -rf /Applications/AIMyChats.app
  ditto "$APP" /Applications/AIMyChats.app
  "$LSREGISTER" -f /Applications/AIMyChats.app
  echo "已安装到 /Applications/AIMyChats.app"
fi
