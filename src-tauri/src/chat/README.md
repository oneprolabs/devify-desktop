# Shared chat read boundaries

`access.rs` owns the existing account-ID validation, source-specific selected
conversation checks, visible-session guard and local account-root lookup. Local
reads, upload preparation and summary callers import this owner directly. Keep the
WeChat and WhatsApp connect switches, account-change check and existing error text.

`mod.rs` owns the single `LOCAL_READ_BUDGET` semaphore. Conversation reads, messages,
avatars, local image decoding and upload-state scans share its existing capacity 2;
the original commands keep their acquire/try-acquire behavior and permit lifetimes.
Avoid per-command or per-platform copies, which would multiply concurrent scans.

Native adapters live under `../commands/chat/`. Database/media implementations
remain in `../wechat/` and `../whatsapp/`. This directory adds no database, cache,
scheduler or replacement source abstraction. Shared app-data and upload/sync owners
remain explicit dependencies until their own bounded refactor.
