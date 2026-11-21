# Native command ownership

This directory groups Tauri command adapters by product responsibility:

- `analysis.rs`: event/report browsing, report generation and analysis preferences.
- `events.rs`: progress notes, event edits and event relations; its ID-list parser stays private.
- `topics.rs`: topic subscriptions, note detail and day/week/month report plans.
- `questions.rs`: question lists, details and submission.

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

Keep handwritten files around or below 1000 lines, grouped by coherent behavior.
Move tests with their private owner when appropriate. For example, `events.rs`
registers `../tests/id_list.rs`, preserving private access without a public parser.

Verify a bounded extraction with production compilation, serial native regressions,
command-registration/signature/body comparison and affected caller checks. See
`../tests/README.md` for the required serial command. Browser simulated IPC does not
verify native dispatch; record native/server E2E separately when it is performed.
