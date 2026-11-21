import { t, tf } from '../../shared/i18n/locale';
import type { UploadOwner } from '../tasks/taskCloud';


export type AppSettings = {
  user_settings_revision: number;
  active_account_id: string;
  chat: {
    allowed_usernames: Record<string, string[]>;
    initial_ranges: Record<string, Record<string, { since: number | null; until: number | null }>>;
    sync_interval_seconds: number;
  };
  connectors: { waitlist: string[]; wechat_enabled: boolean; whatsapp_enabled?: boolean };
  devify: { base_url: string; device_label: string; device_id: string; platform: string; active_space_id: string | null; recipient: string; subject: string };
  upload_queue?: Array<{ id: string; space_id: string; batch_id?: number | null; status: string; message: string; updated_at: string }>;
  voice: {
    auto_transcribe: boolean;
    upload_transcripts: boolean;
  };
  sync: {
    enabled: boolean;
    cursors?: Record<string, Record<string, unknown>>;
    owners?: Record<string, UploadOwner>;
    saved_progress?: Array<{ owner?: UploadOwner | null; cursors: Record<string, unknown> }>;
    last_run_at: string | null;
    last_status: string | null;
    last_error: string | null;
    last_message_count: number;
  };
  backup: {
    mode: 'interval' | 'times' | 'manual';
    times: string[];
    window_enabled: boolean;
    window_mode: 'deny' | 'allow';
    window_start: string;
    window_end: string;
    /** 0 = 周一 … 6 = 周日. */
    days: number[];
    /** Only upload over Wi-Fi or a wired link, never a tethered phone. */
    wifi_only: boolean;
    /** text = no attachments, images = pictures, all = every attachment up to 25 MB. */
    attachments: 'text' | 'images' | 'all';
  };
  language: { ui: string; summary: string; ask: string };
  automation: {
    enabled: boolean;
    daily_summary_enabled: boolean;
    daily_time: string;
    last_daily_run: string | null;
    last_daily_status: string | null;
    last_daily_error: string | null;
    daily_topic_id?: number | null;
    daily_topic_space_id?: string | null;
    last_schedule_sync_at?: string | null;
    last_schedule_signature?: string | null;
    last_schedule_error?: string | null;
  };
  agent: {
    provider_id: string;
    command: string;
    model: string;
    timeout_seconds: number;
    instructions: string;
    use_for_daily: boolean;
  };
};

export function defaultSettings(): AppSettings {
  return {
    user_settings_revision: 0,
    active_account_id: '',
    chat: { allowed_usernames: {}, initial_ranges: {}, sync_interval_seconds: 1800 },
    connectors: { waitlist: [], wechat_enabled: true, whatsapp_enabled: true },
    devify: { base_url: 'https://aimychats.com/', device_label: 'AIMyChats', device_id: '', platform: '', active_space_id: null, recipient: '', subject: t("微信工作日报") },
    voice: { auto_transcribe: true, upload_transcripts: true },
    sync: { enabled: false, last_run_at: null, last_status: null, last_error: null, last_message_count: 0 },
    language: { ui: '中文', summary: '跟随界面', ask: '跟随提问' },
    backup: {
      mode: 'interval',
      times: ['18:00'],
      window_enabled: false,
      window_mode: 'deny',
      window_start: '23:00',
      window_end: '07:00',
      days: [0, 1, 2, 3, 4, 5, 6],
      wifi_only: true,
      attachments: 'images',
    },
    automation: {
      enabled: false,
      daily_summary_enabled: true,
      daily_time: '08:30',
      last_daily_run: null,
      last_daily_status: null,
      last_daily_error: null,
      daily_topic_id: null,
      daily_topic_space_id: null,
      last_schedule_sync_at: null,
      last_schedule_signature: null,
      last_schedule_error: null,
    },
    agent: {
      provider_id: '',
      command: '',
      model: '',
      timeout_seconds: 180,
      instructions: '',
      use_for_daily: true,
    },
  };
}

export function requireSettingsRevision<T extends { user_settings_revision?: unknown }>(stored: T): T {
  if (!Number.isSafeInteger(stored?.user_settings_revision) || (stored.user_settings_revision as number) < 0) {
    throw new Error(t("设置版本缺失或无效，请更新应用并重新打开页面。"));
  }
  return stored;
}

function settingsErrorMessage(reason: unknown): string {
  if (typeof reason === 'string') return reason;
  if (reason && typeof reason === 'object' && 'message' in reason) return String(reason.message);
  try { return JSON.stringify(reason) ?? String(reason); } catch { return String(reason); }
}

export function isSettingsConflict(reason: unknown): boolean {
  return settingsErrorMessage(reason).includes('SETTINGS_CONFLICT:');
}

export function settingsSaveError(reason: unknown): string {
  const message = settingsErrorMessage(reason);
  if (message.includes('SETTINGS_CONFLICT:')) {
    return tf("设置已在其他页面或账号/空间中变化，未保存。请重新打开页面，确认当前账号和空间后重新操作。{0}", message.split('SETTINGS_CONFLICT:').slice(1).join('SETTINGS_CONFLICT:'));
  }
  return tf("保存失败：{0}。请检查当前账号和空间后重试。", message);
}

/** Apply a confirmed response without discarding edits made after the request began. */
export function mergeSavedSettings(saved: AppSettings, submitted: AppSettings, draft: AppSettings): AppSettings {
  const merge = (confirmed: unknown, sent: unknown, current: unknown): unknown => {
    if (Object.is(current, sent)) return confirmed;
    if (current && sent && confirmed && typeof current === 'object' && typeof sent === 'object'
      && typeof confirmed === 'object' && !Array.isArray(current) && !Array.isArray(sent) && !Array.isArray(confirmed)) {
      const result: Record<string, unknown> = { ...confirmed as Record<string, unknown> };
      const before = sent as Record<string, unknown>;
      const now = current as Record<string, unknown>;
      for (const key of new Set([...Object.keys(before), ...Object.keys(now)])) {
        if (!(key in now)) delete result[key];
        else result[key] = merge(result[key], before[key], now[key]);
      }
      return result;
    }
    return current;
  };
  const result = merge(saved, submitted, draft) as AppSettings;
  // A revision is backend-owned, even if a stale render was used by an input handler.
  result.user_settings_revision = saved.user_settings_revision;
  return result;
}

export function mergeSettings(stored: AppSettings): AppSettings {
  const defaults = defaultSettings();
  const chat = { ...defaults.chat, ...stored.chat };
  // The stepper works in whole minutes/hours; a 30-second legacy value would be shown as
  // "1 分钟" while the real schedule stayed at 30 seconds. Snap it back to the default.
  if (!(chat.sync_interval_seconds >= 60)) chat.sync_interval_seconds = defaults.chat.sync_interval_seconds;
  return {
    ...defaults,
    ...stored,
    chat,
    connectors: { ...defaults.connectors, ...(stored.connectors ?? {}) },
    devify: { ...defaults.devify, ...stored.devify },
    voice: { ...defaults.voice, ...(stored.voice ?? {}) },
    sync: { ...defaults.sync, ...stored.sync },
    backup: { ...defaults.backup, ...(stored.backup ?? {}) },
    language: { ...defaults.language, ...(stored.language ?? {}) },
    automation: { ...defaults.automation, ...stored.automation },
    agent: { ...defaults.agent, ...stored.agent },
  };
}
