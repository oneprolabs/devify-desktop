# Chat command adapters

Group adapters by read capability, retaining the existing WeChat/WhatsApp branches:

- `local.rs`: paged selected conversations, account profiles, bounded avatars,
  setup catalogs, paged messages, keyword search and the existing local digest.
- `collections.rs`: selected conversation dates and bounded message ranges.
- `media.rs`: attachment manifests, lazy image previews, image viewing, reveal and
  the existing entitlement-checked export endpoint.
- `voice.rs`: voice-engine preferences, cached playback and transcription. Missing
  voice blobs still refresh through the existing `wechat_sync` owner and state.
- `remote.rs`: explicit cloud conversation lookup, paged cloud records, record
  detail and lazy cloud images. No remote requests are added to local list reads.

Native registration uses full module paths. Keep the external command names,
argument casing, response fields, error markers, source branches and permissions
unchanged. Existing `wechat_` names are compatibility contracts even when their
implementation also supports WhatsApp. Moving an endpoint does not expose it in UI.

Access checks belong to `../../chat/access.rs`, shared with upload preparation,
cloud summaries, queued summaries and local Agent summaries. Local read concurrency
is owned by `../../chat/mod.rs`: one process-wide semaphore, retaining capacity 2
and the original permit lifetimes. Cloud image concurrency remains its existing
separate function-local budget. Keep account/service/space cache keys, keyset
cursors, scan caps, request-generation checks and lazy loading with their callers.

This migration retains the existing crate-level app-data/HTTP helpers, the upload
message-range helper used by collections, and the sync owner used by voice refresh.
Those owners require their own subsequent extraction; adapters must not duplicate
them. Upload coverage/status and connector setup/key capture stay in their current
owners for separate reviewed batches.
