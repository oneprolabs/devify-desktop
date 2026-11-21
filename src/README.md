# Frontend source layout

Group code by user-facing feature. Keep a feature's components, presentation helpers,
types, and styles together. Move existing code in small, independently verified batches;
the root modules are intentionally retained until their own migration is reviewed.

## Current boundaries

```text
src/
  main.tsx                    # Application entry
  PrototypeShell.tsx          # Navigation and page composition
  features/
    chat/
      ChatWorkbench.tsx       # Conversation selection and workspace orchestration
      ChatEventsPane.tsx      # Event filtering, grouping, and presentation only
      ChatPlatformBar.tsx     # Platform navigation
      LocalChatAvatar.tsx     # Lazy local avatars
      RemoteChatFallback.tsx  # Missing-local-record fallback
      chatPresentation.ts     # Chat presentation helpers
      *.css                   # Chat workspace and platform styles
      local-reader/
        presentation.tsx      # Pure date/size/avatar formatting and search highlighting
      summary/
        ConversationSummary.tsx
        summaryInput.ts       # Input admission and bounded message reads
        summaryRecords.ts     # Durable task/server-result reconciliation
        conversation-summary.css
  assets/                     # Application artwork, platform icons, fonts
  ...                         # Existing modules awaiting later batches
```

## Dependency rules

- The application shell composes features. Feature modules must not import the shell.
- Import the owning module directly. Avoid forwarding files at old locations and broad
  barrel exports that hide dependencies or introduce cycles.
- Feature code may use the existing root infrastructure (`desktopInvoke`, `locale`,
  `taskRuntime`, settings, and entitlements). Extract shared infrastructure only when its
  boundary is clear; do not copy it into each feature.
- Keep CSS import order and DOM structure stable during directory moves. Several styles
  currently rely on the global cascade, so moving a stylesheet does not change when it loads.
- The existing `WeChatLocal.tsx`, `ChatSummary.tsx`, and `summary.tsx` still provide legacy
  reading, local-summary, and rendering APIs. Their consumers and state lifetimes need a
  separate review before splitting or moving them.

## Incremental refactoring

1. Move one coherent feature and update all production, fixture, and test imports.
2. Verify `npm run build`, `npm run test:frontend`, and the affected browser interactions.
3. Save the directory move separately from component/logic extraction.
4. Extract presentation first. Keep request ownership, cancellation, pagination, and
   task scheduling in their existing owner until each has its own focused review.

Subsequent batches can group connectors/wizards, tasks/backup, analysis workspaces,
and settings. The large native `src-tauri/src/lib.rs` requires a separate Rust refactor;
frontend organization does not require changes to native commands or server contracts.

Extracted presentation is limited to the chat event pane and the local reader's pure
formatting/highlighting functions. Event loading, request cancellation, retry timing,
conversation ownership, media queues, and task scheduling remain in their existing
modules. Review larger stateful extractions as separate batches.
