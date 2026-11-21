# Native command ownership

This directory groups Tauri command adapters by product responsibility:

- `analysis.rs`: event/report browsing, report generation and analysis preferences.
- `events.rs`: progress notes, event edits and event relations; its ID-list parser stays private.
- `topics.rs`: topic subscriptions, note detail and day/week/month report plans.
- `questions.rs`: question lists, details and submission.
- `agents.rs`: local Agent discovery, models, access-kit setup, MCP status and direct
  local summary invocation. Existing CLI execution remains in `agent.rs`.
- `summary_history.rs`: account-scoped summary history. Persistence and record limits
  remain in `history.rs`; scope resolution remains in `local_scope.rs`.
- `chat/`: local conversations/messages and search, collection date ranges, media,
  voice and explicit cloud reads. Its README describes the five capability owners.
  Existing `wechat_` IPC names also serve the original WhatsApp branches.

`mod.rs` only declares owners. `lib.rs` registers commands through their full owner
paths in `tauri::generate_handler!`; it does not keep forwarding functions or broad
re-exports. Externally invoked command names, argument casing and response shapes
remain compatibility contracts when moving a function. `pub(crate)` permits native
registration without exposing commands outside the crate.

Command validation and permission checks stay with each command. Existing shared
request/authentication helpers still live at the crate root for this migration
stage; commands import those dependencies explicitly. Moving those helpers and the
login/upload lifecycle is a separate change, with its own review and verification.
Do not duplicate HTTP, identity, retry or scheduling logic inside these adapters.

The shared local bridge state and its start/stop reconciliation belong to
`mcp/runtime.rs`. App startup registers that same state once, and login/logout,
entitlement refresh, settings saves and the existing background tick keep calling
the same reconciliation owner. Agent commands read that state; they do not create
another listener or their own service lock. State access stays crate-internal;
the listener task and token remain private to the runtime owner.

Chat adapters share the policy helpers in `chat/access.rs` and the single local-read
semaphore in `chat/mod.rs` (paths relative to the native source root). Local reads,
media decoding, avatars and the existing upload-state command retain that same
budget. Do not put a new semaphore in each command module or duplicate source
whitelists in summary callers.

Keep handwritten files around or below 1000 lines, grouped by coherent behavior.
Move tests with their private owner when appropriate. For example, `events.rs`
registers `../tests/id_list.rs`, preserving private access without a public parser.

Verify a bounded extraction with production compilation, serial native regressions,
command-registration/signature/body comparison and affected caller checks. See
`../tests/README.md` for the required serial command. Browser simulated IPC does not
verify native dispatch; record native/server E2E separately when it is performed.
