// Response shapes used by the local reader. Keep aligned with the existing native IPC.
// Type declarations only; this module has no runtime dependencies.

export type DbFile = {
  rel: string;
  path: string;
  size: number;
  encrypted: boolean;
  salt: string | null;
  has_wal: boolean;
};

export type Account = {
  id: string;
  dir: string;
  db_count: number;
  encrypted_count: number;
  total_bytes: number;
  /** Newest database write time; the account signed in right now is the one still writing. */
  modified_at: number | null;
  dbs: DbFile[];
};

export type ScanResult = { roots: string[]; accounts: Account[]; skipped: string[] };

export type KeyStatus = {
  path: string;
  stored: number;
  keys: {
    account: string;
    rel: string;
    salt: string;
    source: string;
    verified_at: string;
    verified_secs: number | null;
  }[];
};

export type WalOutcome = {
  present: boolean;
  frames_read: number;
  frames_applied: number;
  final_pages: number | null;
  status: string;
  complete: boolean;
};

export type DecryptOutcome = {
  account: string;
  rel: string;
  status: string;
  pages: number;
  failed_page: number | null;
  error: string | null;
  wal: WalOutcome | null;
  output: string | null;
};

export type SessionItem = {
  username: string;
  display: string;
  last_time: number | null;
  is_group: boolean;
  member_count: number | null;
  preview: string | null;
  unread: number | null;
};

export type MessageItem = {
  local_id: number;
  source_shard: string;
  time: number;
  sender: string | null;
  sender_name: string | null;
  kind: number;
  text: string;
  is_self: boolean;
};

export type MediaItem = {
  local_id: number;
  server_id: number;
  server_id_exact: string;
  source_shard: string;
  time: number;
  kind: 'image' | 'video' | 'file' | 'voice' | 'emoji' | string;
  name: string | null;
  rel_path: string | null;
  size: number | null;
  exists: boolean;
  encrypted: boolean;
  decoded: boolean;
  thumb: string | null;
  preview: string | null;
  sources: string[];
  duration: number | null;
  note: string | null;
};

export type MediaReport = {
  items: MediaItem[];
  counts: { images: number; videos: number; files: number; voices: number; encrypted: number; missing: number };
};

export type VoiceResult = {
  server_id: number;
  local_id: number;
  wav: string | null;
  duration_secs: number;
  text: string | null;
  cached: boolean;
  error: string | null;
};

export type SearchSnippet = {
  time: number;
  sender: string | null;
  sender_name: string | null;
  text: string;
};

export type SearchHit = {
  username: string;
  display: string;
  is_group: boolean;
  hits: number;
  truncated: boolean;
  last_time: number | null;
  snippets: SearchSnippet[];
};

export type SearchReport = {
  keyword: string;
  scanned: number;
  skipped: number;
  hits: SearchHit[];
};

export type PlatformCapabilities = { platform: 'macos' | 'windows' | 'linux' | string; key_acquisition: string; supports_memory_scan: boolean; supports_macos_capture: boolean; supports_key_import: boolean };

export type WechatEnv = {
  app_data_dir: string;
  cache_root: string;
  keys_path: string;
  scanner_path: string | null;
};

export type SavedKeySource = {
  path: string;
  exists: boolean;
  modified_at: number | null;
  age_seconds: number | null;
  mode: string | null;
  secure: boolean;
  extractor: string | null;
  extractor_source: string | null;
  extractor_installed: boolean;
  install_target: string;
  install_source: string | null;
};

/** In-app capture status reported by the Rust port of the reference flow. */
export type CaptureStatus = {
  pending: boolean;
  stage: string;
  needs_cleanup: boolean;
  monitor_ready: boolean;
  capture_phase: string | null;
  resume_available: boolean;
  prepared_process_exited: boolean | null;
  transaction_id: string | null;
  recovery_error_code: string | null;
  entitlement_mode: 'preserved' | 'sanitized' | 'deep' | null;
  platform_supported: boolean;
  lldb_available: boolean;
  wechat_present: boolean;
  wechat_version: string | null;
  wechat_build: string | null;
  wechat_official: boolean;
  wechat_ad_hoc: boolean;
  wechat_quarantined: boolean;
  user_can_write: boolean;
  backup_root: string;
  restore_slot_present: boolean;
  message: string;
};

export type PrepareResult = {
  transaction_id: string;
  version: string;
  build: string;
  backup_path: string;
  backup_created: boolean;
  official_cdhash: string;
  entitlement_mode: 'preserved' | 'sanitized' | 'deep';
  dropped_entitlements: string[];
};

/** Why the resigned copy did not start; rendered as an actionable card instead of a raw error. */
export type LaunchDiagnosis = {
  quarantined: boolean;
  resigned: boolean;
  gatekeeper_accepted: boolean | null;
  gatekeeper_reason: string;
  open_error: string;
  /** The system refused to create the process (AMFI / launchd 153); Gatekeeper steps cannot help. */
  amfi_rejected: boolean;
  processes: number;
  hint: string;
};

export type LaunchAttempt = {
  launched: boolean;
  error: string;
  diagnosis: LaunchDiagnosis;
};

export type LaunchCheck = {
  transaction_id: string;
  ready_for_preflight: boolean;
  requires_manual_launch: boolean;
  pid: number | null;
  diagnosis: LaunchDiagnosis | null;
};

export type PreflightResult = {
  transaction_id: string;
  pid: number;
  pbkdf_locations: number;
  message: string;
};

export type CaptureImport = {
  matched: number;
  already: number;
  accounts: AccountKeyMatch[];
  message: string;
  key_mode: string;
  saved_path: string;
  restored: boolean;
  validated_roles: string[];
};

export type RestoreResult = {
  restored: boolean;
  version: string;
  build: string;
  cdhash: string;
  verified: boolean;
};

export type AccountKeyMatch = {
  account: string;
  matched: number;
  already: number;
  total: number;
  selected: boolean;
};

export type KeyImportResult = {
  matched: number;
  already: number;
  unmatched: string[];
  accounts: AccountKeyMatch[];
  message: string;
};

export type AgentCurrent = {
  configured: boolean;
  agent_id: string;
  agent_name: string;
  command: string | null;
  available: boolean;
};

export type AgentSummary = {
  agent_id: string;
  agent_name: string;
  model: string;
  output: string;
  sessions: number;
  messages: number;
  characters: number;
  truncated: boolean;
  elapsed_ms: number;
};
