# Local Agent bridge lifecycle

`../mcp.rs` owns the existing HTTP/SSE listener, authentication, read-only tools and
access-kit endpoints. `runtime.rs` owns the desktop-managed `McpState`, service
status, single task slot, start/stop reconciliation and post-login entitlement
refresh. Command adapters live in `../commands/agents.rs`.

Startup registers one `McpState`. Login/logout, entitlement refresh, connector
settings and the existing background tick all reconcile that same state. Keep the
task lock, token, readiness timeout and shutdown channel with this owner; do not
create another listener, timer or per-command copy of the state.

The runtime currently uses the existing crate-level app-data and token helpers.
Their extraction belongs to a separately reviewed authentication/context change.
Module relocation must preserve the existing permission gates, error text,
serialized status fields and entitlement refresh timing.
