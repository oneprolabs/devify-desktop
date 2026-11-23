<!-- CODEGRAPH_START -->
## CodeGraph

In repositories indexed by CodeGraph (a `.codegraph/` directory exists at the repo root), reach for it BEFORE grep/find or reading files when you need to understand or locate code:

- **MCP tool** (when available): `codegraph_explore` answers most code questions in one call — the relevant symbols' verbatim source plus the call paths between them, including dynamic-dispatch hops grep can't follow. Name a file or symbol in the query to read its current line-numbered source. If it's listed but deferred, load it by name via tool search.
- **Shell** (always works): `codegraph explore "<symbol names or question>"` prints the same output.

If there is no `.codegraph/` directory, skip CodeGraph entirely — indexing is the user's decision.
<!-- CODEGRAPH_END -->

## Code organization memory

Before reorganizing directories or splitting large files in this project, read
`/Users/ray/.codex/memories/devify-desktop-code-organization.md` and the current
`src/README.md`. For native test changes, also read `src-tauri/src/tests/README.md`.
These record the agreed ownership boundaries, the roughly 1000-line file preference,
and the requirement to verify and commit each bounded batch before continuing.

## Development app isolation

- Use **AIMyChats Dev** for local development. Run `npm run desktop:dev` for hot
  reload, `npm run desktop:build:dev` for a signed macOS package, or
  `npm run desktop:install:dev` to install it. Do not use the production bundle ID
  for everyday debugging. See README's **Development app and production app** section.
- Development defaults to the development service. Occasional production testing
  is allowed by explicitly changing the service address; keep the production warning
  visible and remember that server-side operations then affect real production data.
  Keep the development-mode label and current service address visible on the login
  landing page and in the login dialog, including when connecting to production.
  To change the default development service, click **登录… → 连接设置 → 服务端地址
  → 保存并返回登录**. Tell colleagues this path; changing the address affects only
  the selected app variant's saved settings.
- Installing or restarting Dev must not quit, replace, re-sign or clear the installed
  production app. Do not copy production credentials, task databases, keys or caches
  into Dev, or overwrite production Skills/CLI installations.
- Keep the Dev bundle ID, signing identity, app-owned storage, local API port, login
  callback and URL scheme separate and stable. Keep Dev frontend output in `dist-dev/`
  and CLI resources in `src-tauri/binaries-dev/`; do not point either at production output. Dev must not use the production updater.
  Do not bypass the WeChat installation-operation guard to test capture/recovery.
- Production builds are explicit: `scripts/macos-build.sh --production --no-install`.
  Retain the production release configuration and update signatures; publishing,
  replacing the installed production app and changing server configuration require
  authorization for that action. Follow existing session authorization when present.
- Report simulated browser checks, packaged smoke checks and real server E2E
  separately. Until the server supports the Dev browser-login registration, use
  password login; do not fall back to the production callback port.
