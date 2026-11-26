# Release guide

[Back to README](../README.md)

Each version gets its own Release in
[`oneprolabs/devify-desktop-releases`](https://github.com/oneprolabs/devify-desktop-releases),
tagged `v<version>` (for example `v0.1.3`). The release repository is public so anonymous update
checks work; the source repository stays private and only build artifacts are published. The first
updater-enabled version still requires one manual installation.

The Tauri updater needs one stable URL, so `latest.json` lives in a separate **manifest-only**
Release tagged `alpha`. That Release never holds versioned installers: it only carries the current
manifest, whose download URL points at the matching `v<version>` Release.

GitHub Actions workflow `.github/workflows/release-macos.yml` builds and publishes every release:

1. Align the version in npm, Cargo and Tauri files, including lockfiles. Review and merge,
   then create a new `v<version>` source tag. Never move an existing release tag.
   Run **Actions → Release macOS → Run workflow** from that tag, using the same release tag input.
2. Write the `notes` input in **English**. The workflow rejects a note containing CJK
   characters before it starts building, because the same text is published on the public
   releases page and rendered as the update line in **Settings → About**. With no note the job falls
   back to `AIMyChats <version> for macOS Apple Silicon (build <sha>)`.
3. The job runs on `macos-15` (Apple Silicon), imports the self-signed macOS identity, runs
   `scripts/prepare-release.sh` (version check → build → `macos-build.sh --production --no-install` →
   updater archive → Tauri signature → `latest.json`), then:
   - creates or updates the versioned Release with the DMG, `.app.tar.gz` and `.sig`;
   - creates or updates the `alpha` manifest Release and uploads `latest.json` last.
4. The run page links to both Releases; the same files are attached to the workflow run as
   artifacts.

Required repository secrets (Settings → Secrets and variables → Actions):

| Secret | Value |
|---|---|
| `RELEASES_TOKEN` | PAT with **Contents: read and write** on `oneprolabs/devify-desktop-releases`. The default `GITHUB_TOKEN` cannot write to another repository. |
| `MACOS_SIGNING_P12` | base64 of the `AIMyChats Local Signing` identity exported from `~/Library/Keychains/signing.keychain-db` (`security export -t identities -f pkcs12 -o identity.p12`, then `base64 -i identity.p12`). |
| `MACOS_SIGNING_P12_PASSWORD` | Password protecting that `.p12`. |
| `MACOS_KEYCHAIN_PASSWORD` | Any throwaway password; the workflow creates an ephemeral keychain on the runner. |
| `TAURI_SIGNING_PRIVATE_KEY` | base64 of the Tauri updater private key file. Keep the original outside the repository and back it up; losing it makes every installed app unable to verify updates. |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password of the updater key, or empty when it has none. |

The Tauri updater signing key and the macOS app signing identity are separate. The macOS identity
is self-signed, so a downloaded DMG is still blocked by Gatekeeper on other Macs until the app is
notarised; alpha users install manually the first time and afterwards use the in-app updater.

Publishing by hand (same result, useful when Actions is unavailable):

1. Set the same new SemVer version in `package.json`, `src-tauri/tauri.conf.json`, and
   `src-tauri/Cargo.toml`. Alpha prereleases must compare newer than the installed version:
   `0.0.1-alpha.1` is older than `0.0.1`.
2. Set `TAURI_SIGNING_PRIVATE_KEY_PATH`, optionally `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` and
   `AIMYCHATS_RELEASE_NOTES` (English only, same text as the workflow's `notes` input).
   `RELEASES_REPO` defaults to `oneprolabs/devify-desktop-releases` and
   `RELEASES_TAG` to `v<version>`.
3. Run `scripts/prepare-release.sh`. It builds and signs without installing or publishing.
4. Upload the DMG, the archive and signature to the versioned Release:
   `gh release upload "v<version>" -R oneprolabs/devify-desktop-releases --clobber <files>`.
5. Upload `latest.json` to the manifest Release last:
   `gh release upload alpha -R oneprolabs/devify-desktop-releases --clobber latest.json`.
6. Install an updater-enabled baseline manually, then test check → download → install → restart,
   preserved data and login, and macOS Full Disk Access before relying on updates.

The generated manifest currently includes only the architecture built on this Mac; merge valid
platform entries from separately built artifacts before releasing multiple architectures.
