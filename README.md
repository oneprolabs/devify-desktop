# AIMyChats (Tauri 2)

A local-first Windows/macOS desktop app for reading selected WeChat conversations locally, organizing work content, and uploading the records to a self-hosted Devify server for scheduled reports.

Main areas:

1. **聊天记录** — on explicit user action, locates the local WeChat 4.x data directory, decrypts a copy of the SQLCipher databases with keys you supply, and opens a WeChat-style three-pane viewer: conversation list (single chats and group chats, with member counts and unread badges), message history with day/time separators, and keyword search across conversations. Messages can be clicked into a cross-conversation selection that survives switching chats. The work-content picker can select a time range, one or more conversations, or all conversations located by a keyword, then expands the selected conversations together for review.
2. **聊天白名单** — independently selects which group chats and one-to-one chats are visible for each WeChat account.
3. **自动化策略** — configures local sync interval, incremental upload of allowlisted conversations to Devify, and the daily report time that is synced to a Devify topic subscription.
4. **Devify 配置** — selects the server, space, and device. The daily report schedule is stored on the server.
5. **Agent 集成** — one settings tab with two sections. **本地总结** picks a local CLI agent, model, timeout, and extra instructions for manual chat summaries. **工具接入** generates one prompt that installs the read-only `wxq` CLI and the three `SKILL.md` documents from the app's in-process download service into directories you choose, then verifies them with `wxq health` / `wxq policy`. No MCP configuration is involved.
6. **关于** — shows the current build (`0.1.0`), the runtime platform, and the app identifier. The version is read from the Rust build (`Cargo.toml`), which is kept in sync with `package.json` and `tauri.conf.json`.

聊天记录是默认入口；原始消息可以在本地导出，自动化策略负责按白名单上传记录，日报由 Devify 服务端按计划生成。

The app never modifies, deletes, or writes to WeChat's own database files. It does not hook WeChat, inject code, bypass login, or talk to WeChat servers. Decrypted copies live only in the app's data directory on this machine.

## What is implemented

### 本地聊天记录 (local database reading)

- **Discovery without hardcoded paths**: scans macOS sandbox containers / Windows tenant directories for `db_storage` folders, so version and container-name changes do not break it. Lists accounts, every `.db` file, size, encryption state, page-1 salt, and WAL presence.
- **Key import with proof, not trust**: accepts pasted `x'<64 hex key><32 hex salt>'`, bare hex, `all_keys.json`-style JSON, or an account **passphrase**. A bare 64-hex value is tried as a raw key first and then as a passphrase, in which case the per-database key is derived with `PBKDF2-HMAC-SHA512(passphrase, salt, 256000, 32)` (SQLCipher 4 defaults). Every candidate is verified against the database's page-1 HMAC-SHA512 before being stored. Keys that do not verify are rejected and everything else is kept unchanged (no privilege escalation, no guessing). Re-importing re-verifies stored keys and replaces rotated ones, and reports “无需重复导入” when nothing changed (also stating that the submitted material added no new key, so a mistyped value cannot pass as a success); **清除本账号密钥** resets one account without touching WeChat's files.
- **Key store**: `<app_data_dir>/wechat-keys.json`, mode `0600`, containing only verified `(account, db path) → key + salt` entries.
- **Per-page decryption**: SQLCipher 4 layout — page size 4096, reserve 80 (16-byte IV + 64-byte HMAC), `mac_key = PBKDF2-HMAC-SHA512(enc_key, salt ^ 0x3a, 2, 32)`. Every page is authenticated before being written; a single mismatch aborts the whole file, so no partially decrypted database is ever published. Output is written to a temporary file and renamed atomically.
- **Scoped by default**: only `session/session.db`, `contact/contact.db`, `message/message_*.db`, `message/media_*.db` (voice payloads) and `head_image/*` are decrypted, so chat browsing does not touch FTS indexes or unrelated databases. Clear the checkbox to process every database.
- **Attachment manifest**: every image, video, document and voice message gets a manifest entry (kind, name, size, path, existence). Documents (`msg/file`), videos (`msg/video`) and their covers are plaintext and open in Finder. Image `.dat` payloads (V2: AES-128-ECB prefix + single-byte-XOR trailer) are **decrypted offline** with keys derived from the account's own `kvcomm` cache. Previews are built lazily as images scroll into view (full-size originals are capped at 900 px), **查看原图** opens the native-resolution image, and export writes `media/images/**` as real JPEG/PNG. `WxAM` (HEVC) originals are decoded through `ffmpeg` when it is installed; messages whose original was never downloaded keep the 微信 thumbnail and say so. Export writes `transcript.md`, `manifest.json` and a `media/` folder to `~/Downloads/AIMyChats/<chat>-<timestamp>/`.
- **Voice transcription**: WeChat voice is SILK v3 stored in `message/media_0.db` (`VoiceInfo.voice_data`). The app decodes it with a vendored Skype SILK reference decoder to 24 kHz mono WAV (cached under `<app_data>/voice-cache/`), plays it inline, and transcribes on demand with a local CLI (`whisper-cli`, `mlx_whisper` or `whisper`) configured under **Agent 集成 → 语音转写**. Transcripts are cached per `account|server_id`; nothing is uploaded and no model is bundled.
- **Limited WAL replay**: committed prefix only, with per-frame salt and page-HMAC checks. SQLite's rolling checksum chain is **not** validated, so the result can miss the newest committed transaction. The UI says so for every database.
- **Schema discovery, not assumption**: `PRAGMA table_info` is used everywhere; missing columns produce an explicit error instead of fabricated messages. Sessions come from `session/session.db`, display names from `contact/contact.db`, messages from `Msg_<md5(username)>` tables across all `message/message_*.db` shards, with `Name2Id` used for sender ids. The viewer only exposes one-to-one and group chats; official accounts, service accounts, built-in WeChat service sessions, and synthetic placeholder sessions are filtered out consistently from browsing, search, exploration, and digest input.
- **Group chats**: sessions ending in `@chatroom` are marked as groups, with member counts from `chat_room` ⋈ `chatroom_member`. The inline `sender_username:` prefix that WeChat stores inside group message content is stripped, used as the sender fallback, and never shown as part of the text.
- **Readable rich messages**: WeChat stores links, files, quotes, mini programs and transfers as XML (often zstd-compressed). The viewer never prints raw markup — it decompresses, strips the group prefix, then labels the message by `local_type` plus the app message `<type>` and `<title>`/`<des>` (`[图片]`, `[文件] 预算.xlsx`, `[链接] 季度复盘（…）`, …).
- **Self detection**: own messages are recognised by voting across direct conversations and are right-aligned in green.
- **Bounded keyword search**: `Enter` in the search box scans up to 200 conversations (500 hard cap), max 50 hits per conversation, and reports how many were scanned vs skipped. Snippets are rendered with the keyword highlighted; opening a hit jumps to the matching message.
- **Work-content selection**: the “整理工作内容” flow supports today / 7 days / 30 days / all time / custom dates, explicit multi-conversation selection, and keyword-based cross-conversation locating. After loading, every selected conversation is expanded in one review panel with keyword highlighting. “AI 总结” then hands exactly the reviewed messages (capped at 400 per conversation and ~45,000 characters) to the local agent you configured; it asks for confirmation before anything leaves the app, and shows how many conversations, messages and characters will be sent.
- **Local agent summarisation**: the `Agent 总结` page scans this machine for supported agent CLIs (`claude`, `codex`, `opencode`, `gemini`, `qwen`, `cursor-agent`), falling back to the login shell when a Finder-launched app has a minimal `PATH`. Pick one, optionally pin an executable path, model and timeout, add extra instructions, and run a real connectivity test. The app never stores model credentials: it runs the chosen CLI non-interactively (`claude -p`, `codex exec`, `opencode run`, …) in the app data directory, with read-only sandboxing where the CLI supports it. Output is plain text; failures, timeouts, and Claude's `is_error` JSON are surfaced as errors instead of being presented as a summary.
- **Message-level selection and exploration**: click individual messages to build a persistent selection across conversations. “总结 / 探索” produces a local, rule-based first pass with project focus items (to-dos, decisions, risks, questions and data), a date-grouped timeline, and related conversations discovered by extracted terms. Related sessions can be added to the selection for another review pass. This is navigation and evidence organization, not an LLM conclusion.
- **Centralized strategy**: the `聊天白名单` page persists the allowed group and one-to-one conversations per account. The Rust layer always enforces this allowlist for the session list, message reads, search, exploration, digest, and scheduled delivery; an empty allowlist exposes no chat history.
- **Server-side daily report**: enabling the daily report creates or updates a `TopicSubscription` on Devify with the selected local time and IANA timezone. Devify runs the schedule even when the desktop app is closed, then generates the report from the uploaded notes and events. The app only keeps the topic id and the last known run status.
- **No SMTP delivery in this path**: the daily report no longer reads the local database or sends email from the desktop. Chat records reach Devify through the existing incremental upload, and delivery/notification is a server concern.
- **Read-only service and agent kit**: the desktop app starts an in-process listener at `127.0.0.1:18765` and stops it with the app. Chat data is exposed as whitelist-checked REST (`/api/policy`, `/api/sessions`, `/api/messages`, `/api/search`, `/api/digest`, plus `/health`). The same listener distributes the agent kit: `/agent/manifest`, `/agent/wxq` (the read-only CLI binary), `/agent/skills`, `/agent/skills/<name>` and `/agent/install.sh`. The **Agent 集成 → 工具接入** page turns this into one prompt: the user picks a host and two directories, copies the prompt, and the receiving agent downloads both artifacts and verifies the connection. The CLI speaks the REST side, so any agent with shell access can read chats. `WECHAT_AGENT_BIND` moves the listener and `WECHAT_AGENT_TOKEN` is mandatory for any non-loopback bind (the older `WECHAT_MCP_*` names still work as aliases). Every request re-reads `app-settings.json` and re-checks the whitelist, so policy changes take effect without a restart. Nothing can decrypt, write, or send mail. MCP over SSE remains available for existing GUI clients but is not part of the page or the prompt; the standalone `wechat-mcp-sse` binary is only a debugging entry point.
- **Per-conversation browsing**: newest-first messages, page size 100 with "load older", day dividers, and time stamps only when messages are more than five minutes apart.
- **Incremental upload to Devify**: while automatic sync is on, every interval decrypts the local cache and uploads only the allowlisted conversations that have messages newer than their saved cursor. The first run of a conversation sends its full local history. Batches are idempotent by content, and the client never invents messages the local database does not contain. See [Sync chat records to a Devify space](#sync-chat-records-to-a-devify-space).
- **Cloud records view**: the 聊天 page opens a **云端记录** tab that reads the selected Devify space's conversations and messages (search, paging, jump-to-message). Event evidence links open the matching server-side message instead of the local viewer.
- **Clear cache** action deletes only the decrypted copies, never WeChat's originals.

## Prerequisites

- Node.js LTS + npm
- Rust stable toolchain
- Tauri 2 platform prerequisites: <https://v2.tauri.app/start/prerequisites/>
- For local chat records: WeChat 4.x desktop on the same machine, already logged in with local history.
- A running Devify server with the chat-analysis periodic task enabled. The desktop app needs no SMTP configuration.

## Daily report schedule

The daily report is a server-side `TopicSubscription`. When you save the daily time in **自动化策略**, the app creates or updates a topic titled `微信工作日报` with `period=day`, the selected `time_local`, and the machine's IANA timezone. Devify's scheduler runs it even when the app is closed, using the chat records that have already been uploaded.

The desktop no longer generates the daily digest locally and no longer sends it by SMTP. If you need email or push delivery, configure it on the Devify side.

## Read local chat records (optional)

WeChat 4.x stores its databases encrypted with SQLCipher 4 and keeps the per-database keys **only in the running process's memory**. Reading them requires root access to the WeChat process. This project decrypts databases you already own on your own machine; it does not bypass login, server-side protection, or cloud backup.

### Import keys from a tool you trust (recommended)

The supported input path is a JSON file produced by whatever tool you trust (for example an extractor you already use on this machine). Supported shapes: `x'<64 hex key><32 hex salt>'`, a plain 64-hex key, and the `all_keys.json` map. In the app: **本地聊天记录 → 1. 密钥 → 选择密钥文件**.

Every key is re-verified against the current databases' page-1 HMAC before it is stored, so a stale or rotated key is reported rather than used.

### In-app key capture (macOS, Apple Silicon)

WeChat 4.1.10+ no longer keeps the raw key per database in memory: it keeps a 32-byte account
**passphrase** and derives each database key with `PBKDF2-HMAC-SHA512(passphrase, salt, 256000, 32)`.
The passphrase is only observable at login, so the app implements the same capture flow as the
reference project (`~/Workspace/AI/WeChatDataAnalysis`), in process, without a second tool and
without shell commands for the user to type:

1. **环境检查** — the guide shows every guard before it lets you start: Apple Silicon, LLDB present,
   `/Applications/WeChat.app` is Tencent-signed (Team `5A4RE8SF68`, hardened runtime, not ad-hoc),
   and the current user can write both the bundle and `/Applications` (otherwise a crashed run
   could not be restored without another authorization).
2. **备份并临时重签** — a verified `ditto` ZIP of the original bundle
   (`…/key-capture/wechat-app-backups/WeChat-<version>-<build>-original.zip`) plus a same-volume
   APFS clone into `/Applications/.WeChat.wedata-official-restore.app`. Only the **clone** is
   re-signed ad-hoc. Depending on the probe result, this may include a deep re-sign of nested
   components without sandbox entitlements. The clone is then
   exchanged with the installed bundle using `renameatx_np(RENAME_SWAP)`, so no half-copied
   application is ever left behind. You must tick the risk checkbox first: WeChat quits, the
   temporary build needs a fresh QR login and may use a different data directory.

   **macOS 15+ entitlement gate.** macOS 15 refuses to create a process whose ad-hoc signature still
   claims the team-bound entitlements (`com.apple.application-identifier`,
   `com.apple.developer.team-identifier`, `com.apple.security.application-groups`,
   `com.apple.security.temporary-exception.*`): launchd fails with errno 153 (`Launchd job spawn
   failed`) and WeChat never reaches `main`. Before touching the installed bundle, the app probes
   the original and empty entitlement sets using a purpose-built probe. If the original set fails
   and the empty set works, it deep-signs the clone without entitlements, including nested components.
   The temporary app runs without the original sandbox and may create files in a different location.
   The probe cannot guarantee that the full application will launch; if it fails, cancel to restore
   the official app. If neither probe works, the installed WeChat is left untouched.
3. **打开微信、登录并完成捕获** — you open WeChat and log in yourself (the app never bypasses a
   macOS security prompt). Then:
   - **检查启动** verifies the running process really is this transaction's ad-hoc build
     (Team/version/CDHash + device/inode identity);
   - **检查捕获断点** attaches LLDB briefly behind one `do shell script … with administrator
     privileges` prompt (the password goes to macOS, never to this app), proves
     `CCKeyDerivationPBKDF` resolves, then detaches **before** you log out;
   - **启动监测** asks for authorization again, arms the breakpoint monitor and only then shows
     `监测已就绪` — you log out and log back into the same account at that point;
   - the LLDB callback only accepts a candidate whose PBKDF2 salt matches this account's databases
     **and** whose HMAC unlocks page 1 of **both** the message and the session database. A single
     database hit is never treated as success. The app then re-validates the same candidate in Rust
     (`crypto::verify_key` / `verify_passphrase`) before storing anything.
4. **导入并校验** — the verified passphrase is written to
   `~/.wcdb-key-tool/wechat-passphrase.json` with `0600` and imported through the normal per-database
   verification, then imported keys are shown per account. A rotated passphrase replaces the stale
   key; a paste or file that unlocks nothing reports what happened instead of a silent success.
   The guide also shows the file's age and permission and offers **修复口令文件权限** when it is
   readable by other local accounts.

**Restore is not optional and not best-effort.** Every failure path (timeout, wrong account, failed
validation, cancel button, app crash on the next launch) swaps the untouched official clone back and
verifies Team identifier, version and CDHash before reporting success. If the installed bundle has
changed underneath the transaction, or the recovery slot holds something this transaction did not
create, the app **refuses to overwrite** and keeps the app, the ZIP backup and the recovery state
(`external_install_conflict`, `in_place_debug_identity_unknown`,
`official_restore_staging_conflict`), telling you what to do instead of guessing.

The monitor runs as root through LLDB, so it needs `xcode-select --install` and two administrator
authorizations per capture. If you would rather not re-sign WeChat at all, import a key file from a
tool you trust instead — the guide stays available from the **本地数据** tab in the sidebar (密钥、解密与缓存).

**If WeChat will not open after the temporary re-sign (macOS 15+).** This is Gatekeeper, not a broken
signature. A DMG-installed `WeChat.app` keeps the `com.apple.quarantine` attribute; replacing Tencent's
Developer ID signature with an ad-hoc one invalidates the notarisation ticket, so Gatekeeper refuses
the launch. macOS 15 removed the Control-click → **打开** override, and 15.1 can reject the launch
without showing any dialog at all (the Finder icon just does nothing). Approve it once:

1. Double-click `/Applications/WeChat.app` in Finder and dismiss the warning with **完成 / Done**
   (never **移到废纸篓**).
2. Within one hour open **系统设置 → 隐私与安全性 → 安全性**, find `“WeChat” 已被阻止…` and click
   **仍要打开**, enter the admin password, then confirm **打开**.
3. Back in the app, click **我已登录，检查启动**.

The step-3 panel now reports the isolation attribute, `spctl --assess` verdict, the number of running
WeChat processes, and any `open` error, so "nothing happened" is explainable instead of silent. If no
**仍要打开** entry ever appears, the launch was not blocked by Gatekeeper — check
`~/Library/Logs/DiagnosticReports` for a WeChat crash report and use **取消并恢复微信** first.

**If the launch error mentions `launchd 153` / `Launchd job spawn failed`.** That is not Gatekeeper
and no override helps: the system killed the process while creating it, because the ad-hoc signature
could not satisfy the entitlements it carries (see the entitlement gate above; the same error occurs
when the temporary copy still holds Tencent's team-bound claims). The panel switches to the
`AMFI` branch — one **取消并恢复微信** button, no System Settings steps — and the app re-signs only
after a probe proved the chosen dictionary can start. Use **导入密钥**, or capture on macOS 14 or
earlier, when the probe reports that neither variant starts (`in_place_amfi_rejected`).

Key freshness is visible: the guide shows when the current account's keys were last verified.
WeChat updates and re-logins rotate the passphrase, so capture and import again when decryption
starts failing with HMAC errors.

**Clearing keys really locks the data again.** **清除本账号密钥** deletes the stored keys *and* the
account's decrypted snapshot (`…/wechat-cache/<account>`), because the snapshot is plaintext SQLite
and would otherwise stay readable. Cache entries remember the salt of the key that produced them, so
after clearing keys (or after a re-key) the cached copy is never served as a fresh success — the next
view reports `no-key` until a key is imported again. Re-importing does not need a new capture while
`~/.wcdb-key-tool/wechat-passphrase.json` is still valid: **导入已提取的口令** re-verifies every
database.

### macOS memory scanner (only where the process is already debuggable)

> Verified on 2026-10-26: stock WeChat 4.1.15 (hardened runtime, sandboxed) refuses the attach **even as root** (`attach failed (Not allowed to attach to process)`), and the bundled scanner is denied `task_for_pid` the same way. So on an official install this path does not work; use a key file from a tool you trust. The scanner stays useful for builds that permit debugging.

The bundled scanner walks the WeChat process's readable memory, extracts `x'<key><salt>'` candidates, and keeps only those whose HMAC matches a database on disk. It needs root **and** a process whose task port may be inspected.

```bash
# 1. Probe read-only and change nothing. Success prints "Process ... stopped".
sudo lldb -p $(pgrep -x WeChat) -o "detach" -o "quit"

# 2. Only if the probe attaches:
cd src-tauri
cargo build --bin wechat-key-scan --features aux-binaries
sudo ./target/debug/wechat-key-scan --out ~/wechat-keys.json
```

If the probe fails with `Not allowed to attach to process`, the scanner will fail the same way (`task_for_pid` is denied); stop there instead of looking for a workaround that modifies WeChat.

### Do not re-sign WeChat by hand

An earlier revision of this README suggested:

```bash
sudo codesign --force --deep --sign - /Applications/WeChat.app
```

Do not run that. On WeChat 4.1.15 (Tencent, TeamIdentifier `5A4RE8SF68`) an ad-hoc `--deep` signature strips the sandbox and App Group entitlements. The app then starts **unsandboxed**, writes its state to `~/Documents/app_data` instead of its container, and exits within seconds. Re-signing cannot restore the Apple-issued identity, the App Group container access, or the keychain groups.

The in-app capture described above is different on purpose and is the only sanctioned path: it keeps
the entitlement-bearing signature on a **clone**, verifies the result against the reference's
entitlement/helper checklist, exchanges bundles atomically, and records recovery facts before the
first byte of the installed app changes. Never reproduce a single step of it manually.

Recovery without data loss:

1. Quit WeChat. Never delete `~/Library/Containers/com.tencent.xinWeChat`; the chat history lives there.
2. Download the official build: <https://dldir1v6.qq.com/weixin/Universal/Mac/WeChatMac_4.1.15.dmg>.
3. Verify before installing: `codesign --verify --deep --strict` passes, `spctl -a -vv` reports `Notarized Developer ID`, and the TeamIdentifier is `5A4RE8SF68`.
4. Replace `/Applications/WeChat.app` with the copy from the DMG (for example with `ditto`), then launch. The existing container is reused as-is.

Other notes:

- The scanner prints per-database results only. It never prints key material.
- WeChat updates rotate keys: re-import after an update.
- WeChat 4.1.10+ caches only a passphrase in memory instead of the raw key. Pasting that passphrase (64 hex) into the app works: each database's key is derived per salt and verified before use. The bundled scanner still emits raw keys only.

### Windows

The discovery, key import, decryption, and query layers are platform-neutral, but the memory scanner is macOS-only for now. On Windows, supply keys produced by a tool you trust through the same JSON import; they will still be verified before use.

### What the decryption path guarantees, and what it does not

Guaranteed:

- Correct SQLCipher 4 page format (verified against databases produced by the independent `sqlcipher` CLI, with `PRAGMA integrity_check` on the output).
- Every page's HMAC is checked before use.
- No silent fallback: a wrong key or a mismatched salt produces an explicit failure, not empty data.
- A WAL salt mismatch, HMAC failure, page-1 frame, or truncation stops the replay prefix instead of guessing.

Not guaranteed:

- **Completeness while WeChat is running.** WAL replay is a restricted path. For an authoritative snapshot, quit WeChat (or at least accept that the most recent transaction may be missing).
- **Schema stability across versions.** The query layer adapts to columns via introspection, but if a future version renames the semantic fields, the app will report an unsupported schema rather than invent data.
- **Rich content.** Images, voice, video, files, and stickers are represented by placeholders; no media is extracted or forwarded. Compressed text (`WCDB_CT_message_content = 4`) is decompressed with zstd, and failures are labelled.

## Configure the Devify server

1. Deploy and start your self-hosted Devify instance using its official instructions: <https://github.com/oneprolabs/devify>.
2. Sign in from the desktop app and select the space that should receive the chat records.
3. Keep **自动同步** enabled so the records needed by the daily report are uploaded and analysed.
4. Save the daily report time in **自动化策略**. The app creates or updates the server-side topic; Devify's scheduler runs it even when the app is closed.
5. Verify the workflow with non-sensitive test text before relying on the report.

The desktop uploads chat archives through Devify's HTTP API; it does not send the daily report by SMTP. Report generation and any notification delivery happen on the server.

## 问题反馈

从「设置 → 问题反馈」打开，也可从登录、上传失败提示进入。
填写问题描述，粘贴、拖入或选择最多 3 张截图；提交前可裁剪和实心遮盖敏感信息。
诊断日志默认不附带，勾选后可预览最近 30 分钟或 24 小时的运行记录。
诊断采集不读取聊天正文、数据库、密钥或登录凭据。

反馈不再要求填写联系方式：提交人就是当前登录账号。客户端会自动附带客户端版本、
操作系统版本（如 `macOS 14.5 (23F79)`）和已安装的微信版本，便于定位环境差异。

客户端访问 Devify 时，每次 HTTP 请求会携带独立的 `X-Request-ID`。
诊断记录包含同一 ID 的请求开始、HTTP 状态或网络错误、耗时；Devify 的响应头
和请求期间的服务端日志携带该 ID，可据此关联排查。重试会生成新请求 ID，
反馈编号等业务幂等键保持不变。

草稿按服务端和登录用户保存在本机，提交失败可重试原反馈。在线反馈发送至界面显示的
当前 Devify 实例，需登录且服务端已部署反馈模块；处理状态与回复在「我的反馈」查看。

Devify 管理端入口为 `/management/feedback`；部署需运行数据库迁移、挂载私有
附件目录并登记附件清理任务，详见 Devify 仓库 `docs/feedback.md`。

## Sync chat records to a Devify space

Settings → 备份周期 owns the upload plan: one card per connected platform with its own switch, schedule (every N minutes, at fixed times, or manual only), optional upload window (allow or deny, with the weekday list) and a **立即备份一次** action. Settings → 本地自动化 syncs the daily report time to a Devify topic. Both require a configured Devify account and a selected space; the whitelist still decides what is sent.

- **Scope**: one package per conversation, containing only that conversation's allowlisted messages. The first sync of a conversation uploads its full local history, later runs start at the saved cursor. The cursor is inclusive on purpose (several messages can share a second); the server deduplicates by `message_key`.
- **Package**: `wechat.v1` archive with `manifest.json` (`schema_version`, `platform`, `source_namespace`, `counts`, `scope.conversation_ids`, `whitelist_snapshot`, `client`), `conversations.jsonl` (`conversation_id`, `title`, `kind`, `self_sender_key`), `messages.jsonl` (`message_key`, `source_message_id`, `sender_key`, `sender_name`, `direction`, `occurred_at`, `source_order`, `record_kind`, `source_fields`) and `message_parts.jsonl` (`part_key`, `ordinal`, `kind`, `text`).
- **Idempotency**: the create key is derived from the packaged content, so a retry reuses its batch. A batch the server already accepted (`processing` or `completed`) is reported as success without uploading again, and `archive_size` is not declared at create time so retries cannot conflict with it.
- **Status**: every upload becomes a job in 任务 with its batch id, and finished batches are reconciled back to `completed` / `failed`. 备份周期 shows the next planned run, the last run time, status, message count, and the last error.
- **Schedule**: the scheduler wakes every 30 s and asks `src-tauri/src/backup.rs` whether a run is due, so `interval` means "at least this long since the last run", `times` catches up to the newest scheduled time of the day (a sleeping Mac uploads once it wakes), and `window` only limits when that is allowed.
- **Viewing**: the 聊天 page's 云端记录 tab lists the space's conversations and reads messages through `GET /api/v1/spaces/<id>/conversations` and `/conversations/<id>/messages`, including `focus_message_id` for jumping to a specific message.

## Connect an external agent (one prompt)

The app publishes an **agent kit** from its loopback listener: the read-only `wxq` CLI and the three `SKILL.md` documents. The **Agent 集成 → 工具接入** page turns it into one prompt:

1. Pick the host (Claude Code, Codex, OpenCode, Gemini CLI, Qwen Code, Cursor Agent, WorkBuddy, or a custom directory). Picking one fills in its user-level skills directory.
2. Set the CLI directory (default `~/.local/bin`) and the skills directory. Both accept `~` or absolute paths.
3. Copy the prompt and paste it into that host, allowing shell execution. The agent downloads both artifacts, writes the three `SKILL.md` files, then runs `wxq health` and `wxq policy`.

Nothing in the prompt depends on the repository checkout, and no MCP configuration is involved. The service and every download live on the loopback listener and stop with the app.

### Agent kit endpoints

| Method | Path | Query / body |
|---|---|---|
| `GET` | `/agent/manifest` | — |
| `GET` | `/agent/wxq` | — |
| `GET` | `/agent/skills` | — |
| `GET` | `/agent/skills/<name>` | — |
| `GET` | `/agent/install.sh` | `bin`, `skills` (absolute or `~/…`) |

The `/agent/*` routes expose only the app's own CLI binary and static skill documents — never chat data. They follow the same token rule as `/api/*`.

Manual install without an agent:

```bash
curl -fsSL 'http://127.0.0.1:18765/agent/install.sh?bin=~/.local/bin&skills=~/.claude/skills' | sh
```

### Read-only chat endpoints

| Method | Path | Query / body |
|---|---|---|
| `GET` | `/health` | — |
| `GET` | `/api/policy` | `account_id?` |
| `GET` | `/api/sessions` | `account_id?` |
| `GET` | `/api/messages` | `username`, `limit` (1–500), `before?`, `before_id?`, `account_id?` |
| `GET` | `/api/search` | `keyword`, `groups_only?`, `max_sessions?` (1–200), `account_id?` |
| `POST` | `/api/digest` | JSON `{"usernames":[…],"since?":…,"until?":…,"account_id"?}` |

`GET` returns JSON; failures return `{"error": "…"}` with `400` (rule violation) or `401` (missing/wrong token). Every request re-reads `app-settings.json` and re-checks the whitelist, so policy changes take effect without a restart.

### `wxq` command line

`wxq` is a small HTTP client for those endpoints — it never opens a database, decrypts, or sends mail. It arrives through the prompt (or the install script above); no manual build is required.

```bash
wxq health
wxq policy
wxq sessions
wxq messages --chat 56209716836@chatroom --limit 50
wxq messages --chat 56209716836@chatroom --before 1793025903 --before-id 4211
wxq search --keyword 报价 --groups-only
wxq digest --chat 56209716836@chatroom --since 2026-10-20 --until 2026-10-27
```

- Default output is a readable transcript/table; add `--json` for the raw payload.
- `--endpoint` / `WECHAT_AGENT_ENDPOINT` and `--token` / `WECHAT_AGENT_TOKEN` move the target and supply the Bearer token (the `WECHAT_MCP_*` names remain as aliases).
- `--before`, `--since`, `--until` accept Unix seconds, `2026-10-20`, or `"2026-10-20 18:00"`; a date-only `--until` means end of that day.
- Non-zero exit code plus a message on stderr for whitelist, token, or connectivity failures.

Agent-facing skills live in `skills/` and call `wxq`.

### MCP (legacy, not shown in the app)

The listener still speaks MCP over SSE at `http://127.0.0.1:18765/sse` for existing GUI clients. It is intentionally absent from the **Agent 集成** page and the prompt. `GET /sse` returns the `POST /messages?sessionId=…` endpoint; tools are read-only and whitelist-enforcing (`wechat_get_policy`, `wechat_list_sessions`, `wechat_get_messages`, `wechat_search_messages`, `wechat_digest`). The standalone `wechat-mcp-sse` binary stays available as a debugging entry point; it is not shipped in the app and needs `WECHAT_DEVIFY_APP_DATA_DIR` set to the app data directory.

## Configure the summary agent

The app does not ship model credentials. It reuses an agent CLI that is already installed and logged in on this machine:

1. Open **Agent 总结**. The page scans `PATH`, common install directories, and finally the user's login shell, so a Finder-launched app still finds `~/.local/bin`, Homebrew, nvm/mise/asdf-style installs.
2. Select a detected agent. `重新扫描` refreshes the list; unavailable entries stay visible with the reason.
3. Optional: pin an executable path, choose a model, raise the timeout (30–900 seconds), and add extra instructions for the summary. When the CLI can list its models (OpenCode does), the page reads them through `刷新模型` and the model field suggests them; other agents accept a hand-typed model id.
4. Click **测试连接** — it sends a real one-line prompt through the CLI, so expired logins, disabled accounts, and quota problems appear here instead of surfacing as a fake summary.

> Leaving **模型** empty uses the CLI's own default. An agent can be configured with several providers and still fail here: OpenCode, for example, defaults to its built-in provider, so a signed-out default reports `This API key has expired` even though your other providers work. Pick one of the listed models (for example `agione-complate/...`) and test again. Authentication failures are called out with this hint in the error text.
5. Save. In the chat drawer, review the original messages, click **整理这段聊天**, choose **本地快速整理** (offline keyword grouping) or **AI 总结** (confirm the scope first), and wait for the result.

| Agent | Detection | Non-interactive invocation |
|---|---|---|
| Claude Code | `claude` | `claude -p --output-format json` (prompt on stdin; `is_error` is treated as failure) |
| Codex CLI | `codex` | `codex exec --sandbox read-only --skip-git-repo-check --ephemeral -o <file> -` (prompt on stdin) |
| OpenCode | `opencode` | `opencode run [--model <provider/model>] <prompt>` (`opencode models` fills the picker) |
| Gemini CLI | `gemini` | `gemini -p <prompt>` |
| Qwen Code | `qwen` | `qwen -p <prompt>` |
| Cursor Agent | `cursor-agent` | `cursor-agent -p <prompt>` |

Input bounds are enforced in Rust, not only in the UI: at most 400 messages per conversation, 1,500 characters per message, and ~45,000 characters per prompt. Truncation is reported in the prompt and in the returned stats.

> **Privacy**: the selected agent processes the chat text with whatever provider and model it is configured to use, which may be a cloud service. Nothing is sent until you confirm the scope in the drawer. The server-side daily report only uses records that were already uploaded with your confirmation.

### Saved summaries

Every generated result is also written to `summary-history.json` in the app data directory (`~/Library/Application Support/local.wechat.devifybridge/` on macOS):

- **本地快速整理** keeps the extracted highlights, timeline, related conversations, and search terms as structured data.
- **AI 总结** keeps the Markdown output plus the agent, model, duration, and truncation flag.
- **每日自动日报** is generated on Devify and does not create a local history record.

The file is written atomically with owner-only (`0600`) permissions, capped at the **newest 50 records** (older ones are dropped automatically), and never uploaded. In the drawer, **最近整理** lists the records saved for the current account; open one to re-read it and close it to return to the live preview, or delete it individually. Deleting only affects this local file — it cannot touch the original WeChat database or a report that was already generated on Devify.

## Image decryption (offline)

WeChat 4.x stores chat images as `msg/attach/<md5(username)>/<YYYY-MM>/Img/*.dat`:

```text
[15B header: 07 08 "V2" 08 07 | u32le aes_size | u32le xor_size | 1B pad]
[AES-128-ECB ciphertext, PKCS#7 padded] [plaintext middle] [XOR trailer]
```

The keys are **derived, not scraped from the running client**:

| Key | Derivation |
|---|---|
| `code` | digits in `<WeChat config>/…/kvcomm/key_<code>_*.statistic` |
| `xor_key` | `code & 0xFF` |
| `aes_key` | `md5("{code}{wxid}").hexdigest()[:16]`, used as **16 ASCII bytes** |
| `wxid` | account directory name with its `_<hash>` suffix stripped |

Several codes can exist after an update, so every candidate is verified against a real ciphertext block: decrypting the first block must produce `FF D8 FF` / `PNG` / `GIF` / `wxgf`. A wrong key effectively never matches three leading bytes. Nothing is read from WeChat's process memory; only kvcomm statistic files and the `.dat` files themselves.

Legacy single-byte-XOR payloads and the older `V1` magic (fixed AES key `md5("0")`) are handled too.

`WxAM` originals (`wxgf` magic) wrap a HEVC stream. Their Annex-B bitstream is extracted by scanning for the start code and decoded with `ffmpeg` (`brew install ffmpeg`, optional):

- Chat previews use WeChat's own bubble JPEG, the `_t.dat` thumbnail, or a 900 px render of the original.
- **查看原图 / export** decode the full-size original (`2352×684`, `1702×1276`, …) as PNG.
- When only a thumbnail exists locally (WeChat downloads originals on demand), the entry is marked 「本机只有缩略图」 instead of pretending it is the original.

## Configure voice transcription

Voice messages are decoded in-process; only the speech-to-text step uses an external CLI. Nothing is uploaded and no model is bundled.

1. Install one engine (any of them):
   - `brew install whisper-cpp` plus a ggml model (for example `ggml-large-v3-turbo.bin`); point the model field at the file.
   - `pip install mlx-whisper` on Apple Silicon; the default `mlx-community/whisper-large-v3-mlx` downloads on first use.
   - `pip install openai-whisper`; the default `small` downloads on first use.
2. Open **Agent 集成 → 语音转写**, pick the engine, optionally set the model/language, and save. `自动选择` uses the first available engine.
3. In a conversation, a voice message shows its length plus **转写**; the transcript appears inline and is cached in `voice-transcripts.json` (keyed by account and `server_id`). **重新转写** re-runs one clip.

The WAV decoder lives in `src-tauri/vendor/silk` (Skype's SILK reference implementation, BSD-style licence with an explicit **no patent grant**). It is compiled by `build.rs`; removing that directory only disables voice decoding, everything else keeps building.

## Run / build

```bash
npm install
npm run tauri dev
```

Build a desktop bundle:

```bash
# The CLI is data, not a sidecar executable: `beforeBuildCommand` runs
# `npm run stage:wxq`, which builds it with the opt-in feature and copies it to
# src-tauri/binaries/wxq-<host-triple> for `bundle.resources`.
scripts/macos-build.sh              # builds, signs, and replaces /Applications/AIMyChats.app
scripts/macos-build.sh --no-install # builds + signs only, leaves /Applications untouched
```

Artifacts land in `src-tauri/target/release/bundle/` (`macos/AIMyChats.app`, `dmg/AIMyChats_0.1.0_aarch64.dmg`). The DMG mounts with `AIMyChats.app` next to an `Applications` symlink, so the app can be dragged straight into `/Applications`. On macOS the app bundle contains one executable (`Contents/MacOS/wechat-devify-bridge`) plus `Contents/Resources/binaries/wxq-*`; the CLI is never treated as a helper program and does not take part in the app's code signature.

### macOS signing (why not ad-hoc)

macOS binds Full Disk Access to the app's **designated requirement**. With an ad-hoc signature
that requirement is the CDHash, which changes on every build, so the user has to re-grant access
after each rebuild. This repo signs with a local self-signed identity instead, so the requirement
is `identifier "local.wechat.devifybridge" and certificate leaf = H"…"` and survives rebuilds.

One-time setup on a Mac:

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
security list-keychains -d user -s ~/Library/Keychains/login.keychain-db ~/Library/Keychains/signing.keychain-db

# Trust it for code signing (one system prompt), and keep the keychain password for the build script.
security add-trusted-cert -p codeSign -k ~/Library/Keychains/login.keychain-db signing.cert.pem
printf 'aimychats' > ~/.config/aimychats-signing/keychain.pass && chmod 600 ~/.config/aimychats-signing/keychain.pass
```

`scripts/macos-build.sh` signs the app itself (nested binaries first, then the bundle), rebuilds
the DMG from the signed app, and replaces `/Applications/AIMyChats.app` (pass `--no-install` to
stay inside the repo), because `codesign` only finds the private key in that keychain when it
is passed `--keychain`, which the Tauri bundler does not do. `spctl` still rejects the result since
it is not notarized by Apple — expected for a self-signed build, and only matters for downloads that
carry the quarantine attribute.

If signing starts failing with `errSecInternalComponent`, the signing keychain is locked or has lost
its partition list; recreate it with the import + `set-key-partition-list` steps above.

Regenerate the icon set after replacing the mark in `src/assets/app-icon-source.png`:

```bash
# Adds the macOS tile: 1024 canvas, 824 body, 185px rounded corners, transparent outside.
python3 src-tauri/app-icon.py src-tauri/app-icon.png
cp src-tauri/app-icon.png src/assets/app-icon.png   # runtime logo (sidebar, About, login)
npx tauri icon src-tauri/app-icon.png
```

## Tests

```bash
cd src-tauri
cargo test
```

`tests/pipeline.rs` generates encrypted fixtures with the independent `sqlcipher` CLI (page size 4096, SQLCipher 4) and asserts that decryption, session listing, contact-name resolution, message decoding, cache reuse, passphrase derivation, group prefix stripping, self detection, bounded search, and digest extraction all behave. Install it with `brew install sqlcipher` / `apt install sqlcipher`; the test prints a skip notice and passes when the CLI is absent.

`cargo test --bin wxq --features aux-binaries` covers the CLI's flag/date parsing, URL encoding, chunked responses, and output formatting; it does not need a running app.

### Developer helpers

Both examples only read WeChat's own files; the decrypted copy goes to the app data directory (or a temp directory).

```bash
cd src-tauri
# Unlock + decrypt exactly like the UI does, so the viewer opens without pasting the key.
WX_PASS='<64 hex>' cargo run --quiet --example seed
# Print what the query layer sees for a real account.
WX_PASS='<64 hex>' cargo run --quiet --example probe -- 会议 200
```

## Important limitations

- The masking rules are heuristic, not a data-loss-prevention system. They can miss personal names, addresses, IDs, financial details, or secrets. Manually inspect and edit the preview.
- The app sends text only; it does not forward images, voice messages, stickers, or attachments.
- It does not automatically become a native WeChat forwarding target. You choose a conversation and push it to the preview, or copy/paste manually.
- The work digest is deterministic keyword extraction. It can miss context and can classify small talk as a to-do; treat it as a draft, not an answer. The optional agent summary is real generation, but it can still misread ambiguous chat: every output keeps the original transcript in the drawer for comparison.
- Agent summaries leave the machine through the selected CLI. Its provider, model, retention policy and quota apply; the app cannot audit or redact what that agent sends upstream.
- The viewer has no media previews or voice transcription: non-text messages appear as labelled placeholders. Avatars come from WeChat's own `head_image.db` (decrypted alongside the chat databases) and are only requested for the names on screen; a contact without a stored image falls back to a letter avatar.
- The daily report schedule is stored on Devify and runs on the server, so it no longer depends on the Tauri process staying alive. The app still needs to upload new chat records for the report to include them.
- The report uses the server's configured LLM. If the server has no LLM configuration, the report keeps the rule-based snapshot and is marked `partial`.
- **Reading local databases breaks the earlier "no database access at all" claim in this project's history.** The local-reading feature is opt-in, requires your explicit key import and decrypt action, and is limited to data you already have on this machine.
- Decrypting a local database may conflict with WeChat's Terms of Service or your organization's device policy. You are responsible for having the right to access that data and for the consent of the people in the conversations.

## Architecture

- `src/main.tsx`: workspace shell with chat records, chat whitelist, automation strategy, Agent 集成, Devify configuration, and local data areas.
- `src/WeChatLocal.tsx` + `src/wechat.css`: scan → key import → decrypt → three-pane viewer (sessions / messages / digest drawer), including the reviewed-scope AI summary and its confirmation step.
- `src/Settings.tsx`: whitelist, sync, daily summary, and Devify target settings.
- `src/AgentHub.tsx`: the Agent 集成 tab — segmented switch between 本地总结 (`AgentSettings`) and 工具接入 (`AgentAccess`).
- `src/AgentSettings.tsx`: local agent scan, selection, generation parameters, and connectivity test.
- `src/AgentAccess.tsx`: one-prompt agent installation — host and directory selection, generated prompt, download endpoints, and install status.
- `src/About.tsx`: 关于 page — build stage/version from the `app_info` command, runtime platform, and app identifier.
- `src-tauri/src/lib.rs`: Tauri commands, scheduled sync, rule-based digest, agent summarisation, server-side daily schedule sync, and service startup wiring.
- `src-tauri/src/agent.rs`: agent detection (PATH/search dirs/login shell), bounded non-interactive runs, timeout/kill handling, output and error normalisation.
- `src-tauri/src/access.rs`: embedded skill documents, host → skills-directory mapping, `wxq` resolution, prompt and install-script generation.
- `src-tauri/src/wechat/query.rs`: schema-discovered reads, whitelist filtering, and the bounded transcript builder shared by interactive and automated summaries.
- `src-tauri/src/mcp.rs`: in-process listener (axum): whitelist-enforcing REST handlers, `/agent/*` download routes, and the legacy MCP SSE adapter; `src-tauri/src/bin/wechat-mcp-sse.rs` is a thin standalone debug entry point.
- `src-tauri/src/wechat/discover.rs`: container/tenant discovery, account and `.db` inventory, salt extraction.
- `src-tauri/src/wechat/crypto.rs`: SQLCipher 4 page format, HMAC verification, AES-256-CBC page decryption.
- `src-tauri/src/wechat/keys.rs`: key parsing (hex / `x'…'` / JSON), verified key store with `0600` permissions.
- `src-tauri/src/wechat/decrypt.rs`: per-page decrypt into an atomic cache file, cache invalidation by source size+mtime.
- `src-tauri/src/wechat/wal.rs`: restricted WAL replay with explicit incompleteness reporting.
- `src-tauri/src/wechat/query.rs`: schema-introspecting session/message/contact queries, group member counts, bounded search, rule-based digest, text labelling.
- `src-tauri/src/bin/wechat-key-scan.rs`: macOS root memory scanner that emits only verified keys.
- `src-tauri/capabilities/default.json`: minimal Tauri core permissions; no filesystem-wide or shell permissions are granted.

### Alpha updater releases

Each version gets its own Release in
[`oneprolabs/devify-desktop-releases`](https://github.com/oneprolabs/devify-desktop-releases),
tagged `v<version>` (for example `v0.1.3`). The release repository is public so anonymous update
checks work; the source repository stays private and only build artifacts are published. The first
updater-enabled version still requires one manual installation.

The Tauri updater needs one stable URL, so `latest.json` lives in a separate **manifest-only**
Release tagged `alpha`. That Release never holds versioned installers: it only carries the current
manifest, whose download URL points at the matching `v<version>` Release.

GitHub Actions workflow `.github/workflows/release-macos.yml` builds and publishes every release:

1. Run **Actions → Release macOS → Run workflow**. Leave the tag input empty to publish
   `v<version>` for the version in `src-tauri/tauri.conf.json`; override it only for a deliberate
   re-tag.
2. Write the `notes` input in **English**. The workflow rejects a note containing CJK
   characters before it starts building, because the same text is published on the public
   releases page and rendered as the update line in **设置 › 关于**. With no note the job falls
   back to `AIMyChats <version> for macOS Apple Silicon (build <sha>)`.
3. The job runs on `macos-15` (Apple Silicon), imports the self-signed macOS identity, runs
   `scripts/prepare-release.sh` (version check → build → `macos-build.sh --no-install` →
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
