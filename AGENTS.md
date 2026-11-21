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
