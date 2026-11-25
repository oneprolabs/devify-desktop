<p align="center">
  <img src="src/assets/app-mark-transparent.png" width="80" alt="AIMyChats" />
</p>

<h1 align="center">AIMyChats Desktop</h1>

<p align="center">Bring scattered conversations together. Turn them into useful knowledge.</p>

<p align="center">
  <a href="https://github.com/oneprolabs/devify-desktop-releases/releases">Download</a> ·
  <a href="https://github.com/cloud2ai/devify">AIMyChats Server</a> ·
  <a href="#development">Develop</a>
</p>

## Why this project

Decisions, progress and follow-ups get buried across chat tools.
**devify-desktop** is the desktop client for **AIMyChats**: a chat aggregation
platform that reads conversations available on your computer and collects new
messages as they arrive locally, on your configured schedule.

WhatsApp is currently supported. We plan to add more platforms over time.

Use your conversations to find what matters: summarize discussions, track to-dos,
and connect related events without piecing everything together by hand.

## Desktop + server

[Devify](https://github.com/cloud2ai/devify) is the AIMyChats server.
The desktop collects messages; the server provides shared storage and AI analysis.

| Component | Responsibility |
|---|---|
| Desktop | Connect chat sources, browse local history, choose backup scope and schedule, and upload new messages. |
| Server | Store uploaded records, generate summaries, identify events and their relationships, and produce reports. |
| Your Agent | Use Codex, Claude Code, WorkBuddy or another compatible Agent to read permitted records through an AIMyChats Skill. |

You do not need your own AI Agent or model. Use the server's AI features directly,
or connect an existing Agent when you want to work with your own tools.
Server analysis can continue while the desktop is closed, using records already uploaded.

## Works with your AI Agent

Bring your WhatsApp conversations into **OpenAI Codex**, **Claude Code**,
**WorkBuddy** and your other Agent workflows.

| How you work | Available tools |
|---|---|
| Install the AIMyChats Skill from the app | Codex CLI, Claude Code, OpenCode and WorkBuddy. |
| Generate summaries in AIMyChats through a CLI | Codex CLI, Claude Code, Gemini CLI, OpenCode, Cursor CLI and Qwen Code. |
| Connect another Agent | Copy the installation prompt for a host that supports local Skills and local tool execution. Compatibility depends on the host. |

Find these options in **Settings → Agent integration**. For example, ask Codex
to summarize yesterday's group discussion, Claude Code to extract project decisions,
or WorkBuddy to organize follow-ups from your chats.

One Skill provides a common interface across supported chat platforms. Your Agent
can find conversations, read messages, and retrieve existing cloud to-dos and reports.
Chat reads use local records; cloud-result requests use the desktop's current login.
The desktop must stay open and signed in, and the Agent must be able to access its local service.

AIMyChats can also call supported Agent CLIs to generate chat summaries in the app.
**Calling a local CLI does not guarantee an offline model**: its model and provider
settings determine where the selected content is processed.

Agent integration currently requires an eligible account. Some Agent hosts restrict
local connections even after a Skill is installed.

## Your data, your scope

- **Choose what to back up.** Select conversations, the initial history range and
  the backup schedule. New messages upload automatically when backup is enabled.
- **Keep source records intact.** Chat readers do not modify the source app's records.
  Connection setup may require explicit system permissions.
- **Protect local data.** Treat app-owned caches and credentials as sensitive data.
- **Limit Agent access.** The default bridge listens on localhost, requires a token,
  and checks the selected chat scope. Cloud reads also check the current account and space.
- **Know where AI runs.** Cloud backup and analysis send selected data to the configured
  server. Your Agent may send content to its model provider. Use a server and provider you trust.

## Get started

1. [Download AIMyChats](https://github.com/oneprolabs/devify-desktop-releases/releases) and sign in.
2. Connect a supported chat platform and choose the conversations to back up.
3. Set the backup schedule, then browse chats, generate summaries or install the Skill.

Available history and attachments depend on what the source app has downloaded locally.
AI results should be checked against the original messages.

## Development

Built with **Tauri 2, Rust, React and TypeScript**.
Install Node.js LTS, Rust stable and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).

```sh
npm ci
npm run desktop:dev
```

### Development app and production app

Use **AIMyChats Dev** for everyday debugging. It has separate app data, credentials,
Skills and a local API port, and does not use the production updater.
It defaults to the development service at `http://192.168.8.182:7000`.

To change the address, open **Sign in… → Connection settings → Server address →
Save and return to sign-in**. This changes only the selected app's saved settings.
The environment bar shows the active service; pointing Dev at production affects real data.

```sh
npm run build                  # Type-check and build the frontend
npm run test:frontend          # Frontend regression checks
npm run desktop:build:dev      # Signed macOS Dev package; does not install
```

See the [development guide](docs/development.md) for app isolation, native tests and signing,
the [source layout](src/README.md) for code ownership, and the
[release guide](docs/releasing.md) for publishing.
