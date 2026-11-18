# V7 connector flow review — 2026-11-18

## Result and integration

The connector catalogue, WeChat and WhatsApp guided setup, import management and backup schedule use the V7 layout. Initial history defaults to the most recent seven local calendar days for newly selected conversations without an explicit saved range. Private and group selections are additive. A shared range can apply across pagination, or a selected conversation can be edited individually. Existing sync cursors retain incremental progress.

Integrated `origin/main` at `baed81c316c08c0f8444729667e6eef68000ecd7` (#47 and earlier changes). Conflict resolution preserves the upstream language system, update panel, untrusted-signature import path, upload cancellation and stable hidden-chat/wizard routing. New UI copy uses the same language catalogue. The per-account 1,000-conversation limit is unchanged and recorded for later work.

## Review findings addressed

- Saving one platform serializes other account drafts too. Validation now checks every selected account draft so an unfinished custom date range cannot accidentally become an unrestricted historical import.
- A pending or failed schedule save cannot advance the wizard. The confirmed revision remains the authority; an explicit reload is required after a conflict.
- Untrusted WeChat can reach the existing read/import route, but is not labelled as an official signature and does not bypass native capture verification.
- Upload-held credentials are a temporary condition, not logout. A known active upload now prevents a remote token refresh that could rotate a token before local persistence is available. Login retry and credential generation protection remain intact.
- The backup page distinguishes WeChat scheduling from WhatsApp's currently manual-only backups and hides unsupported email. A WhatsApp account alone does not make WeChat appear connected.

## Change scope

| Area | Effect |
| --- | --- |
| Connector and wizard React components, shared CSS | Layout, explicit checks/retries, loading locks, navigation and completion |
| Import settings | Existing allowlist and initial-range fields; cross-account validation and seven-day defaults |
| Backup schedule | Existing save queue and backend schedule fields; V7 rows and truthful source status |
| Native app information | Adds the OS version to the existing app-info response |
| Native WeChat recovery | Explicit action deeply verifies an official replacement and archives old transaction records; original backups are retained |
| Native authentication | Temporary-error classification and a pre-refresh upload-lease check; existing CAS and identity checks remain authoritative |
| Tests | Strict isolated browser IPC fixture, regression tests and a Node runner for the upstream TSX/locale imports |

No server API, chat database format, installer signing or release/tag workflow is changed by this branch.

## Validation

- `npm run build`: passed.
- `npm run test:frontend`: all 19 regression scripts passed.
- `cargo test --manifest-path src-tauri/Cargo.toml --lib`: 347 passed, 3 ignored for existing local-data fixtures.
- Native login/identity subset: all 13 passed, including a rotated refresh token saved only after an upload lease ends.
- Browser E2E: six scenarios passed after conflict resolution: WeChat connection through completion, WhatsApp connection through completion, explicit permission retry, schedule conflict/reload, untrusted-signature manual import access, and English navigation/schedule/history controls. Unexpected IPC and browser errors: zero.
- V7 reference and rendered schedule inspected in settings and wizard modes, including daily times and upload windows. Build warnings about bundle size and Node's upstream module-mocking API are non-blocking.

The E2E fixture renders production UI with an isolated in-memory native IPC boundary. It does not grant live macOS permissions, attach a debugger to the user's WeChat, log in to a real account or upload personal records. A signed-client hardware acceptance run remains separate; the automated results must not be described as that run. Current user permissions and data were not reset during this validation.

See [the E2E runner instructions](scripts/CONNECTOR_E2E.md) for reproduction.
