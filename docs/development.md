# Development guide

[Back to README](../README.md)

## App isolation

| | Production | Development |
|---|---|---|
| App | AIMyChats | AIMyChats Dev |
| Default service | `https://aimychats.com` | `http://192.168.8.182:7000` |
| Local Agent API | `127.0.0.1:18765` | `127.0.0.1:18766` |
| Skill | `aimychats` | `aimychats-dev` |
| URL scheme | `devify` | `devify-dev` |
| Frontend output | `dist/` | `dist-dev/` |
| Bundled CLI | `src-tauri/binaries/` | `src-tauri/binaries-dev/` |
| Production updater | Enabled | Disabled |

Each app owns its credentials, settings, keys, SQLite tasks and caches. Do not copy
production data into Dev or overwrite production Skills to test a change.

Change the saved service through **Sign in… → Connection settings → Server address →
Save and return to sign-in**. The top environment bar and login dialog show the actual
address. Connecting Dev to production affects real server data; the warning stays visible.

Browser login uses the shared temporary callback `127.0.0.1:47832/auth/callback`.
Only one app can start browser authorization at a time. A busy port does not require
closing the other app; complete or cancel its login first.

## Build and install

Run these commands from the repository root after `npm ci`:

```sh
npm run desktop:dev                 # Hot reload in AIMyChats Dev
npm run desktop:build:dev           # Signed Dev package, no installation
npm run desktop:install:dev         # Build and replace only AIMyChats Dev.app
scripts/macos-build.sh --production --no-install  # Explicit production build
```

macOS packages are written to `src-tauri/target/release/bundle/`.
Grant required macOS permissions to **AIMyChats Dev**, not the production app.
Keep the signing identity stable across rebuilds.

## Tests

```sh
npm run build
npm run test:frontend
CARGO_BUILD_JOBS=2 cargo test --manifest-path src-tauri/Cargo.toml --lib -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --bin aimychats-cli --features aux-binaries -- --test-threads=1
```

Run native tests serially: credentials and identity leases are process-global.
SQLCipher pipeline integration tests need the `sqlcipher` CLI; environment-dependent
skips are not evidence that real chat reads passed.

For browser and native checks, follow [Connector E2E](../scripts/CONNECTOR_E2E.md).
Report simulated browser tests, packaged smoke checks and real server E2E separately.
Use Dev and the development service for routine testing.

## Code layout

- `src/app/`: application shell and navigation.
- `src/features/`: UI and state grouped by feature.
- `src/shared/`: shared UI, API and localization helpers.
- `src-tauri/src/`: Rust commands, platform readers, Agent access and task runtime.
- `skills/`: the unified AIMyChats Skill.

See [frontend ownership](../src/README.md), [native tests](../src-tauri/src/tests/README.md)
and [AGENTS.md](../AGENTS.md) before changing code boundaries.

## macOS signing

macOS binds Full Disk Access to the app's **designated requirement**. With an ad-hoc signature
that requirement is the CDHash, which changes on every build, so the user has to re-grant access
after each rebuild. This repo signs with a local self-signed identity instead, so the requirement
uses a stable app identifier and signing certificate and survives rebuilds.

For a new development Mac without an existing signing identity, use the one-time setup below.
Reuse an existing identity on subsequent builds.

```bash
mkdir -p ~/.config/aimychats-signing && cd ~/.config/aimychats-signing
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -keyout signing.key.pem -out signing.cert.pem \
  -subj "/CN=AIMyChats Local Signing" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning" \
  -addext "basicConstraints=critical,CA:false"

# Dedicated keychain so `codesign` never prompts (password is local and disposable).
security create-keychain -p aimychats ~/Library/Keychains/signing.keychain-db
security set-keychain-settings -lut 86400 ~/Library/Keychains/signing.keychain-db
security unlock-keychain -p aimychats ~/Library/Keychains/signing.keychain-db
security import signing.cert.pem -k ~/Library/Keychains/signing.keychain-db
security import signing.key.pem  -k ~/Library/Keychains/signing.keychain-db -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k aimychats ~/Library/Keychains/signing.keychain-db

# Trust it for code signing (one system prompt), and keep the keychain password for the build script.
security add-trusted-cert -p codeSign -k ~/Library/Keychains/login.keychain-db signing.cert.pem
printf 'aimychats' > ~/.config/aimychats-signing/keychain.pass && chmod 600 ~/.config/aimychats-signing/keychain.pass
```

`scripts/macos-build.sh` signs the app itself (nested binaries first, then the bundle), rebuilds
the DMG from the signed app, and replaces only `/Applications/AIMyChats Dev.app` by default.
Use `--production` explicitly for the production variant, and `--no-install` to leave installed apps untouched, because `codesign` only finds the private key in that keychain when it
is passed `--keychain`, which the Tauri bundler does not do. `spctl` still rejects the result since
it is not notarized by Apple — expected for a self-signed build, and only matters for downloads that
carry the quarantine attribute.

If signing starts failing with `errSecInternalComponent`, the signing keychain is locked or has lost
its partition list; recreate it with the import + `set-key-partition-list` steps above.

