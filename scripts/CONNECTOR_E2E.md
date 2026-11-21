# Connector browser E2E

Start Vite (`npm run dev -- --host 127.0.0.1 --port 1420`), then use an owned Ego Browser task space:

```js
const task = await taskSpace('Connector E2E');
const { runConnectorE2E } = await import('file:///absolute/repository/path/scripts/test-connectors-e2e.browser.mjs');
console.log(await runConnectorE2E(task));
```

After observing successful results, finish the task space in a separate browser invocation.

The fixture renders the production `PrototypeShell`, connector catalogue, WeChat and WhatsApp wizards, import selection, backup schedule and completion page. IPC is a strict in-memory test double: unknown commands fail the test, saves enforce revisions, and save conflicts/delays can be injected. It never reads credentials, touches real chat databases, changes system permissions/signatures, or uploads messages. It is a development-only entry point, absent from the production Vite entry graph.

Covered paths:

- WeChat: system checks, explicit permission retry without focus polling, preparation/login/authorization/logout/capture sequence, selection across pages, individual and bulk initial ranges, seven-day defaults, schedule save acknowledgement and completion, and the official-WeChat open request.
- WhatsApp: system checks, activation, simultaneous private/group selection, range persistence and completion; no WeChat key/capture operations.
- Schedule: the wizard initially confirms the shared schedule, explicit edits wait for save acknowledgement, keeping the current schedule does not rewrite it for another platform, runtime pause/resume uses one shared control, save conflicts block editing, explicit reload restores the confirmed revision, and unsupported email is hidden.

These are browser UI end-to-end tests with an IPC boundary, not live macOS authorization, debugger attachment, real account login or server-upload tests. Run `cargo test --manifest-path src-tauri/Cargo.toml --lib` and `npm run test:frontend` (Node 22.15+; includes the existing regression scripts) alongside them. Real permission restart, key capture and account-specific database compatibility still need a signed-client hardware acceptance run; do not report the isolated fixture as that acceptance.

Post-merge checks also cover the upstream untrusted-signature import path and English navigation, backup schedule and history-range controls.

`runLoginRecoveryE2E(task)` from the same module exercises a delayed auth-status response: the retry view remains mounted, duplicate online events share one in-flight check, returning to login invalidates the old response, and recovering a saved session does not cancel its credentials.

`runLoginUiE2E(task)` checks the V7 login surface: password validation, pending and failed requests, browser wait/cancel/open errors, registration and password-recovery URLs, keyboard submission and focus, first-use server setup, English copy, and the 940×640 minimum window. Browser/password IPC is simulated; this does not validate the live authorization callback lifecycle. Preview with `/scripts/connectors-e2e.html?anonymous` without signing out the installed client; append `&no-server` or `&english` for the other states.
