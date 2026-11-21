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
      local-reader/           # Existing local reader, formatting, viewer styles
      summary/                # Conversation summaries, history, input/record helpers
      collections/            # Existing collection/message views
    connectors/               # Source detection, account picker, disk access
      wizard/                 # WeChat/WhatsApp setup and import scope controls
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

## Boundaries reserved for later extraction

`features/settings/Settings.tsx` still combines import, account, and legacy automation
editors; its shared model has been extracted without changing the declarations.
`features/chat/local-reader/WeChatLocal.tsx` still combines the legacy reader and WeChat
setup UI. `shared/content/summary.tsx` still combines legacy record types and rendering.
Review each remaining internal boundary independently before further extraction. The native
Rust modules under `src-tauri/src/` are a separate refactoring scope.

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
