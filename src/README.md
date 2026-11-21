# Frontend source layout

Organize code by user-facing feature. A feature keeps its existing components, helpers,
types, and styles together. The root contains only the application entry, Vite type
declarations, and this guide.

```text
src/
  main.tsx                    # Mount the app and register global diagnostics
  vite-env.d.ts
  app/                        # Shell, navigation, error boundary, desktop context menu
    styles/                   # Global theme, shell, and navigation styles
  features/
    auth/                     # Login, authentication events, access capabilities
    chat/                     # Conversation workspace and platform navigation
      local-reader/           # Reader, native types, lazy media thumbnails, formatting, styles
      summary/                # Conversation summaries, history, input/record helpers
      collections/            # Existing collection/message views
    connectors/               # Source detection, account picker, disk access
      wizard/                 # WeChat/WhatsApp setup and import scope controls
      wechat/                 # WeChat connection state/actions, native types, connection views
    backup/                   # Shared backup schedule, switch, submission feedback
    tasks/                    # Task page, runtime, cloud jobs, snapshots, status
    analysis/                 # Events, topics, reports, Q&A, analysis settings
    agents/                   # Local CLI configuration and tool access
    settings/                 # Settings model, account, language, About, existing editors
      settingsModel.ts        # Types, defaults, revision checks and merge/error helpers
    feedback/                 # Feedback form, screenshots, diagnostic presentation
    updates/                  # Update checks, download, installation state
  shared/
    api/                      # Native IPC, diagnostics, service URL, device registration
    i18n/                     # Locale state and English catalog
    ui/                       # Common controls, evidence drawer, icons, loading states
    content/                  # Existing markdown/summary rendering and record types
    scheduling/               # Shared daily/report plan helpers
    state/                    # Page snapshots and serialized settings saves
  assets/                     # Brand artwork, platform icons, fonts, licenses
```

## Ownership and dependencies

- `main.tsx` starts the application. `app/` composes feature pages and owns global styles.
- Features import their dependencies directly. Avoid broad barrel exports and forwarding
  modules at former paths; both hide ownership and can introduce cycles.
- `shared/` contains existing code used across features. It does not import `app/` or
  `features/`. Assets remain in one shared resource directory.
- Keep feature-specific styles beside their feature. The global CSS cascade still relies
  on import order, so relocation must preserve when each stylesheet loads.
- Existing cross-feature dependencies are retained. For example, the settings editor uses
  connector scope controls, and the chat reader uses connection checks and task state.
  Directory grouping does not introduce new services, state stores, or request ownership.
- Settings consumers import types, defaults, revision checks, and merge/error helpers from
  `features/settings/settingsModel.ts`. Import `Settings.tsx` only to render its editors.
  The model does not import page components, native IPC, or lifecycle hooks; saves remain
  owned by the existing callers and serialized save queue.
- Reader response types live in `features/chat/local-reader/nativeTypes.ts`; WeChat
  account, key, capture and decryption responses belong to
  `features/connectors/wechat/nativeTypes.ts`. Both are type-only, without forwarding exports
  or duplicate declarations. Existing IPC response fields remain unchanged.
- `connectors/wechat/useWeChatConnection.ts` composes key import, native capture/recovery,
  and decryption hooks. Each owns its state, actions and effects; connection views consume
  those owners rather than a large set of reader setters. The reader retains account
  selection/revision saves and chat data; it supplies session refresh, snapshot invalidation
  and its existing live-sync guard. Connection code does not import the reader component.
  The current V7 checklist and standalone connection panel share these actions.
  V7 alone owns wizard navigation; the standalone panel has no parallel wizard steps.
  Native platform detection and Windows memory scanning remain in the current checklist.
- Shared byte/age/key-date formatting lives in `shared/content/localDataFormat.ts`;
  connection UI does not import chat presentation code.
- `local-reader/MediaThumb.tsx` owns the lazy image thumbnail and its existing shared
  preview queue. The queue remains module-scoped, capped at three active previews;
  observers and cancellation remain owned by each thumbnail instance.

## Boundaries reserved for later extraction

`features/settings/Settings.tsx` still combines import, account, and legacy automation
editors; its shared model has been extracted without changing the declarations.
`features/chat/local-reader/WeChatLocal.tsx` retains account selection, chat browsing and
legacy work organization. Connection state/actions and views, native response types and
lazy media thumbnails are separate. Review account lifecycle, message browsing and work
organization as separate future batches. `shared/content/summary.tsx` still combines
legacy record types and rendering.
Review each remaining internal boundary independently before further extraction. The native
Rust modules under `src-tauri/src/` are a separate refactoring scope.

## File size

Keep new or extracted handwritten files around or below 1000 lines, with a clear
responsibility. Treat larger files as a signal to review their boundaries. Split
state, effects, operations and views by ownership; do not compress code or create
forwarding layers solely to reduce a line count. Existing large files are migrated
in independently verified batches. Generated catalogs and static assets are reviewed
according to their own format rather than split arbitrarily.

Entry-layer Rust unit tests live in `src-tauri/src/tests/`; their README documents
ownership, filters and the required serial regression command. Production Rust
extraction is a separate batch from test relocation. Native command adapters are
grouped under `src-tauri/src/commands/`; that directory's README describes ownership
and IPC compatibility. When a private production owner moves, register its tests
under that owner rather than widening helper visibility.

Local Agent adapters and summary history are separate command owners. The native
MCP desktop lifecycle lives in `src-tauri/src/mcp/runtime.rs`, alongside the existing
listener module. Startup and login/settings callbacks share its single managed
state; command extraction must not create additional service tasks or state locks.

Native chat reads are grouped by capability under `src-tauri/src/commands/chat/`.
Source-specific access checks and the shared local-read budget belong to
`src-tauri/src/chat/`; summary callers share those checks. Existing WeChat/WhatsApp
branches, cache scoping, pagination and IPC names remain unchanged by relocation.

## Safe migration and extraction

1. Change one bounded set of file locations and update production, preview, and test paths.
2. Run `npm run build` and `npm run test:frontend` before the next batch. For a pure move,
   compare production artifacts with the preceding stable baseline when possible.
3. Check affected interactions in the isolated browser fixture. This validates the React
   UI with simulated native IPC; it does not replace packaged native/server E2E tests.
4. Commit each verified batch separately. Keep directory moves separate from logic changes.
5. After the directory layout is stable, extract one responsibility at a time. Preserve
   cancellation, identity boundaries, pagination, media queues, and task scheduling until
   their own implementation and tests have been reviewed.
6. Remove obsolete logic separately from extraction. Check production and preview callers
   first; a hidden control or an old name alone is not evidence that a flow is unused.
