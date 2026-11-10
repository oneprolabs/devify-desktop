#!/usr/bin/env bash
# Build a locally signed app, then create and sign the exact updater archive.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
: "${TAURI_SIGNING_PRIVATE_KEY_PATH:?Set TAURI_SIGNING_PRIVATE_KEY_PATH to the updater private key file}"
test -r "$TAURI_SIGNING_PRIVATE_KEY_PATH"
VERSION="$(node -p "require('./src-tauri/tauri.conf.json').version")"
node - "$VERSION" <<'NODE'
const fs = require('fs');
const version = process.argv[2];
const pkg = JSON.parse(fs.readFileSync('package.json', 'utf8')).version;
const cargo = fs.readFileSync('src-tauri/Cargo.toml', 'utf8').match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (version !== pkg || version !== cargo) throw new Error(`Version mismatch: Tauri=${version}, package=${pkg}, Cargo=${cargo}`);
NODE
ARCH="$(uname -m)"
case "$ARCH" in arm64) TARGET_ARCH=aarch64 ;; x86_64) TARGET_ARCH=x86_64 ;; *) echo "Unsupported architecture: $ARCH" >&2; exit 1 ;; esac
scripts/macos-build.sh --no-install
BUNDLE="$ROOT/src-tauri/target/release/bundle/macos"
ARCHIVE="$BUNDLE/AIMyChats_${VERSION}_${TARGET_ARCH}.app.tar.gz"
# Archive after codesign; Tauri's signature is over these final bytes.
tar -czf "$ARCHIVE" -C "$BUNDLE" AIMyChats.app
npm run tauri -- signer sign --private-key-path "$TAURI_SIGNING_PRIVATE_KEY_PATH" --password "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" --app-version "$VERSION" "$ARCHIVE"
node - "$VERSION" "$TARGET_ARCH" "$ARCHIVE" <<'NODE'
const fs = require('fs');
const [version, arch, archive] = process.argv.slice(2);
const signature = fs.readFileSync(`${archive}.sig`, 'utf8').trim();
if (!signature) throw new Error('Missing updater signature');
const filename = archive.slice(archive.lastIndexOf('/') + 1);
const manifest = {version, notes: process.env.AIMYCHATS_RELEASE_NOTES || '', pub_date: new Date().toISOString(), platforms: { [`darwin-${arch}`]: {signature, url: `https://github.com/oneprolabs/devify-desktop/releases/download/alpha/${encodeURIComponent(filename)}`} }};
fs.writeFileSync(`${archive.slice(0, archive.lastIndexOf('/'))}/latest.json`, JSON.stringify(manifest, null, 2) + '\n');
NODE
printf 'Update artifacts: %s, %s.sig, %s/latest.json\n' "$ARCHIVE" "$ARCHIVE" "$BUNDLE"
