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

## Existing cloud results (issue #91)

`cloud.rs` owns fixed GET reads for cloud context, to-do/event list/detail,
report list/detail, topic subscription list/detail and existing topic reports.
MCP `tools/call` and `GET /api/cloud/:operation` share these handlers. Local chat
reads remain in `chat.rs` and stay on the blocking pool; cloud network reads are
async. Every route still requires the app-owned local Bearer token.

Cloud reads reuse the existing desktop access-token refresh, identity generation,
current service/space/capability scope, strict HTTP client and bounded read retries.
There is no separate login, credential forwarding to the Agent, arbitrary URL
proxy, report generation, edit operation or result cache. Each body is capped at
2 MiB; list pages default to 25 and are limited to 100. Detail and row `space_id`
must match the requested current space, and late identity-changing responses fail.
Errors preserve 401/403/404 rather than masquerading as empty results. Returned
pagination URLs are never fetched; consumers use `next_page` on the fixed route.

Verified against server source commit
`5e5264e02cd091ad6d56d96748afd961cc1e39fd`:

- `/spaces/{space}/events`: personal category, completion, keyword and page filters.
- `/spaces/{space}/reports`: page and status only. No cadence/date/topic filter.
- `/reports/{id}`: body, period, processing/status and full sections with sources.
- `/spaces/{space}/topics`: unpaginated array, bounded by the response-size limit,
  then locally paged with `pagination_mode: local_over_server_array`.
- `/topics/{id}/reports`: latest 50 reports; `source_info.subscription_id` is the
  association authority. This is GET, never POST to `/run`. At the cap the response
  says `history_complete: false`; older history requires explicitly paging the
  normal reports list and checking associations, not an implicit full scan.
- Old reports have no structured cadence field. Keep title/source/period metadata;
  a title-based type identification must be disclosed as such, not a server fact.

Needed server improvements, if later requested: native topic paging, paginated
per-topic report history and explicit cadence/date/subscription filters. They are
not implemented or assumed by this client change. A deployed older server may
return 404; do not invent a fallback endpoint or generate a report to compensate.
