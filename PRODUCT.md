# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

The shipped artifact is a Tauri 2 desktop application for macOS (AppKit/WKWebView shell, 1200×780 default window). The design language must read as a native macOS application, not a website in a wrapper. Windows is a supported build target; macOS is the reference.

## Users

A single operator working on their own Mac: a consultant, PM, or founder who keeps a large share of project communication inside WeChat and needs to get it out again. They are already logged into WeChat 4.x on the same machine, they are technical enough to grant Full Disk Access and paste a key, and they are usually alone at a desk doing end-of-day or end-of-week write-up. They are privacy-sensitive: chat content is commercial material and must not leave the machine except by an action they take themselves.

The secondary reader is the same person's local AI agent, which reads chats through the `wxq` CLI under the same whitelist.

## Product Purpose

AIMyChats turns the WeChat history already on this Mac into readable, organizable, deliverable work material. It decrypts a copy of the local SQLCipher databases with a key the user supplies, shows whitelisted conversations in a reader built for review rather than chat, lets the user pick work content across conversations and time ranges, and produces a local rule-based organizing pass or an AI summary through a locally installed agent CLI. Scheduled digests are delivered to the user's own self-hosted Devify inbox.

Success is: the user opens the app, finds the conversation they were thinking of, selects what matters, and ends with a summary they can send — without ever leaving the machine or trusting a cloud service.

## Positioning

Local-first and evidence-first. Keys are proven against the database's page-1 HMAC before they are stored; nothing is written back to WeChat's files; decrypted copies are the app's own, deletable in one action; the whitelist is enforced in the Rust layer for every read path including the agent CLI. A neighbouring "export your WeChat" product cannot truthfully copy "we never parse anything you did not allowlist, and we can prove the key before we use it."

## Operating Context

- macOS, one window, used at a desk for 20–60 minute review sessions, sometimes in the evening.
- Three recurring rituals: **browse** (find and read a conversation), **assemble** (select messages/conversations across the history into a review set), **produce** (organize or summarize that set, then deliver it).
- First run is a setup ritual that can fail in visible, security-shaped ways: no Full Disk Access, no key, key belongs to a different account, WeChat capture needing a temporary re-sign and a macOS 15 Gatekeeper prompt.
- The app is often open in the background while an agent or a scheduled daily digest touches the same data through the local service.
- Message volume is high: conversations with tens of thousands of messages, paged 100 at a time; long AI summaries with Markdown tables and nested lists.
- All UI copy is Simplified Chinese. Dates, counts, and account ids are shown verbatim.

## Capabilities and Constraints

Eight top-level areas, unchanged by any redesign: 聊天记录 (default), 总结记录, 聊天白名单, 自动化策略, Agent 集成, Devify 配置, 本地数据, 关于.

Constraints that bind any visual work:

- The chat surface is a three-region working view: conversation list, message history, and a drawer for selection / summary / history detail. Cross-conversation message selection and Shift range selection must stay usable.
- **Cloud records are a separate reading surface.** 聊天 → 云端记录 reads the user's own Devify space through the server API (conversation list, message pager, jump-to-message from event evidence). The source is always labelled (微信 for the local reader, Devify 空间 for cloud records), the server view is read-only, and an empty or unparsed batch is stated as empty instead of being filled with local content.
- State is real and often long: partial decryption (`n/20` databases), WAL caveats, per-message truncation flags, agent timeouts, network-less SMTP. Errors are stated as facts plus a next action, never as a spinner that never ends.
- Rich messages (links, files, quotes, mini programs, transfers) render as typed labels, never raw XML.
- Content is Simplified Chinese with heavy Latin/digit mixing (account ids, hashes, paths, model names). Any type choice must hold both.
- No dark mode is in scope. No external font or asset hosting: the app ships offline and CSP restricts remote sources; fonts must come from the app or the system.

## Brand Commitments

- Name **AIMyChats**; app icon uses the official AIMyChats gradient A-mark from https://aimychats.com/zh/.
- The interface accent is green, matching the app icon and the product's WeChat context.
- **2026-10-28, user's explicit choice:** after a currant-accent pass, the user asked for the earlier green back — green is the accent again, on the neutral native chrome.
- The interface must read as a native macOS application.
- **Standing direction (2026-10-28, user's explicit choice):** the category standard — an ordinary, impeccably executed native Mac app — not a distinctive visual world. Conventions are embraced without irony or smuggled quirk. The craft bar is **Things 3, Bear, OmniFocus**: full-height source list under a unified title bar, SF Pro / PingFang at 13px, native control proportions, one accent colour that means "chosen", and no decorative chrome.
- Palette commitment: paper-white reading surface (#FFFFFF) against cool-neutral chrome (#F0F0F2 window / #ECECEF sidebar), with a single green accent (#0D7A45, tint #EAF7F0) for chosen state. No cool-blue SaaS accent, no floating white cards.

## Evidence on Hand

- Real product data exists on this machine: `~/Library/Application Support/local.wechat.devifybridge/app-settings.json`, `summary-history.json` (real AI summaries in Markdown), `wechat-keys.json`, and a populated decrypted cache.
- Screenshots of the incumbent UI are not committed. The incumbent look lives in `src/style.css` (224 lines) and `src/wechat.css` (3413 lines) and in the components.
- No customer quotes, benchmarks, pricing, or press. Future work must not fabricate any.

## Product Principles

1. **Evidence over assertion.** A key is proven, a summary is attributed to an agent and a model, a truncation is counted. The interface shows the proof, not the claim.
2. **Local by default, and visibly so.** Every place where data could leave the machine is marked before the action, not after.
3. **Reading tool, not a chat client.** The surface exists for review, selection, and production. Resembling WeChat is not a goal; finding and assembling material is.
4. **Calm under failure.** Setup, capture, and delivery fail for security reasons. The interface stays legible and says what to do next.
5. **The operator is alone and competent.** No onboarding theater, no gamification, no manufactured urgency; dense information is welcome when it is organized.

## Accessibility & Inclusion

Keyboard reachability for every action (the app already relies on it for selection and shift-range work), visible focus, `prefers-reduced-motion` respected, and text contrast that holds at 12–13px on a light ground.
