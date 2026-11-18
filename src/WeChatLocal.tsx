import { t, tf, ctf, dateLocale, sortLocale } from './locale';
import { LoadingSkeleton } from './LoadingSkeleton';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { convertFileSrc, isTauri } from '@tauri-apps/api/core';
import { desktopInvoke as invoke } from './desktopInvoke';
import './wechat.css';
import { Icons } from './icons';
import { DiskAccessCard } from './Permission';
import {
  HISTORY_KIND_LABEL,
  HISTORY_LIMIT,
  SummaryDetail,
  renderAgentMarkdown,
  type HistorySession,
  type NewSummaryRecord,
  type SummaryRecord,
} from './summary';
import { classifyWorkText, extractWorkTerms, type InsightCategory } from './workInsights';
import { exportNotSupported } from './toast';
import { useCapability, useInternalAccount } from './entitlements';
import { isSettingsConflict, requireSettingsRevision, settingsSaveError, type AppSettings } from './Settings';

type DbFile = {
  rel: string;
  path: string;
  size: number;
  encrypted: boolean;
  salt: string | null;
  has_wal: boolean;
};

type Account = {
  id: string;
  dir: string;
  db_count: number;
  encrypted_count: number;
  total_bytes: number;
  /** Newest database write time; the account signed in right now is the one still writing. */
  modified_at: number | null;
  dbs: DbFile[];
};

type ScanResult = { roots: string[]; accounts: Account[]; skipped: string[] };

type KeyStatus = {
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

type WalOutcome = {
  present: boolean;
  frames_read: number;
  frames_applied: number;
  final_pages: number | null;
  status: string;
  complete: boolean;
};

type DecryptOutcome = {
  account: string;
  rel: string;
  status: string;
  pages: number;
  failed_page: number | null;
  error: string | null;
  wal: WalOutcome | null;
  output: string | null;
};

type SessionItem = {
  username: string;
  display: string;
  last_time: number | null;
  is_group: boolean;
  member_count: number | null;
  preview: string | null;
  unread: number | null;
};

type MessageItem = {
  local_id: number;
  source_shard: string;
  time: number;
  sender: string | null;
  sender_name: string | null;
  kind: number;
  text: string;
  is_self: boolean;
};

type MediaItem = {
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

type MediaReport = {
  items: MediaItem[];
  counts: { images: number; videos: number; files: number; voices: number; encrypted: number; missing: number };
};

type VoiceResult = {
  server_id: number;
  local_id: number;
  wav: string | null;
  duration_secs: number;
  text: string | null;
  cached: boolean;
  error: string | null;
};

type SearchSnippet = {
  time: number;
  sender: string | null;
  sender_name: string | null;
  text: string;
};

type SearchHit = {
  username: string;
  display: string;
  is_group: boolean;
  hits: number;
  truncated: boolean;
  last_time: number | null;
  snippets: SearchSnippet[];
};

type SearchReport = {
  keyword: string;
  scanned: number;
  skipped: number;
  hits: SearchHit[];
};

type PlatformCapabilities = { platform: 'macos' | 'windows' | 'linux' | string; key_acquisition: string; supports_memory_scan: boolean; supports_macos_capture: boolean; supports_key_import: boolean };

type WechatEnv = {
  app_data_dir: string;
  cache_root: string;
  keys_path: string;
  scanner_path: string | null;
};

type SavedKeySource = {
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
type CaptureStatus = {
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

type PrepareResult = {
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
type LaunchDiagnosis = {
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

type LaunchAttempt = {
  launched: boolean;
  error: string;
  diagnosis: LaunchDiagnosis;
};

type LaunchCheck = {
  transaction_id: string;
  ready_for_preflight: boolean;
  requires_manual_launch: boolean;
  pid: number | null;
  diagnosis: LaunchDiagnosis | null;
};

type PreflightResult = {
  transaction_id: string;
  pid: number;
  pbkdf_locations: number;
  message: string;
};

type CaptureImport = {
  matched: number;
  already: number;
  accounts: AccountKeyMatch[];
  message: string;
  key_mode: string;
  saved_path: string;
  restored: boolean;
  validated_roles: string[];
};

type RestoreResult = {
  restored: boolean;
  version: string;
  build: string;
  cdhash: string;
  verified: boolean;
};

type AccountKeyMatch = {
  account: string;
  matched: number;
  already: number;
  total: number;
  selected: boolean;
};

type KeyImportResult = {
  matched: number;
  already: number;
  unmatched: string[];
  accounts: AccountKeyMatch[];
  message: string;
};

type ListTab = 'all' | 'group';
type RangeKey = 'today' | '7d' | '30d' | 'all' | 'custom';
type WorkPhase = 'select' | 'review';

type WorkGroup = {
  session: SessionItem;
  messages: MessageItem[];
  truncated: boolean;
};

type SelectedMessage = {
  session: SessionItem;
  message: MessageItem;
};

type WorkInsightItem = SelectedMessage & {
  category: InsightCategory;
};

type WorkInsights = {
  timeline: { day: string; messages: SelectedMessage[] }[];
  focus: WorkInsightItem[];
  related: SearchHit[];
  terms: string[];
};

type AgentCurrent = {
  configured: boolean;
  agent_id: string;
  agent_name: string;
  command: string | null;
  available: boolean;
};

type AgentSummary = {
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

const PAGE = 100;
const WORK_PAGE = 500;
const WORK_MAX_MESSAGES_PER_SESSION = 5000;
/** Matches the per-conversation cap in the Rust transcript builder. */
const AI_MESSAGES_PER_SESSION = 400;
/** Matches `SUMMARY_MAX_CHARS` in the Rust transcript builder. */
const AI_MAX_CHARS = 45_000;
/* Decoding a WxAM original runs ffmpeg, so previews are built lazily and at most a few at
   a time. The backend caches the result on disk; this queue only throttles the IPC calls. */
const previewQueue: Array<() => void> = [];
let previewActive = 0;
const PREVIEW_CONCURRENCY = 3;
function enqueuePreview(task: () => Promise<void>) {
  previewQueue.push(() => {
    previewActive += 1;
    void task().finally(() => {
      previewActive -= 1;
      const next = previewQueue.shift();
      if (next) next();
    });
  });
  if (previewActive < PREVIEW_CONCURRENCY) {
    const next = previewQueue.shift();
    if (next) next();
  }
}

function MediaThumb({ accountId, item, onOpen }: { accountId: string; item: MediaItem; onOpen: () => void }) {
  const [src, setSrc] = useState<string | null>(item.preview ? convertFileSrc(item.preview) : null);
  const [failed, setFailed] = useState(false);
  const [loaded, setLoaded] = useState(false);
  useEffect(() => {
    if (src || failed) return;
    let cancelled = false;
    enqueuePreview(async () => {
      try {
        const path = await invoke<string | null>('wechat_media_preview', {
          accountId,
          localId: item.local_id,
          paths: item.sources,
        });
        if (!cancelled) {
          if (path) setSrc(convertFileSrc(path));
          else setFailed(true);
        }
      } catch {
        if (!cancelled) setFailed(true);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [accountId, item.local_id, src, failed]);

  if (!src) {
    return (
      <span className="wxv-image-placeholder" role="status">
        {!failed && <i className="wxv-image-spinner" aria-hidden="true"/>}{failed ? t("图片加载失败") : t("正在加载图片…")}
      </span>
    );
  }
  const onlyThumb = item.sources.length > 0 && item.sources.every(path => path.endsWith('_t.dat'));
  return (
    <button
      type="button"
      className="wxv-attach-thumb"
      onClick={onOpen}
      title={onlyThumb ? t("本机只有缩略图（微信未下载原图）") : t("查看原图")}
      data-open-image={item.local_id}
    >
      {!loaded && <span className="wxv-image-placeholder" role="status"><i className="wxv-image-spinner" aria-hidden="true"/>{t("正在加载图片…")}</span>}
      <img src={src} alt={t("图片")} data-open-image={item.local_id} onLoad={() => setLoaded(true)} onError={() => { setSrc(null); setFailed(true); }} style={loaded ? undefined : { position: 'absolute', visibility: 'hidden' }} />
      {onlyThumb && <span className="wxv-attach-badge">{t("仅缩略图")}</span>}
    </button>
  );
}

function formatSize(bytes: number): string {
  if (bytes > 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
  if (bytes > 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

function toDate(seconds: number | null): Date | null {
  if (!seconds) return null;
  const date = new Date(seconds > 1e12 ? seconds : seconds * 1000);
  return Number.isNaN(date.getTime()) ? null : date;
}

function pad(value: number): string {
  return value < 10 ? `0${value}` : String(value);
}

function dayKey(date: Date): string {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

function dayLabel(date: Date): string {
  const today = new Date();
  const yesterday = new Date(today.getTime() - 86400000);
  const time = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  if (dayKey(date) === dayKey(today)) return tf("今天 {0}", time);
  if (dayKey(date) === dayKey(yesterday)) return tf("昨天 {0}", time);
  if (date.getFullYear() === today.getFullYear()) return tf("{0}月{1}日 {2}", date.getMonth() + 1, date.getDate(), time);
  return tf("{0}年{1}月{2}日 {3}", date.getFullYear(), date.getMonth() + 1, date.getDate(), time);
}

function timeOnly(seconds: number | null): string {
  const date = toDate(seconds);
  return date ? `${pad(date.getHours())}:${pad(date.getMinutes())}` : '';
}

/** WeChat-style session list timestamp: time for today, date otherwise. */
function listTime(seconds: number | null): string {
  const date = toDate(seconds);
  if (!date) return '';
  const today = new Date();
  if (dayKey(date) === dayKey(today)) return timeOnly(seconds);
  const yesterday = new Date(today.getTime() - 86400000);
  if (dayKey(date) === dayKey(yesterday)) return t("昨天");
  if (date.getFullYear() === today.getFullYear()) return `${date.getMonth() + 1}/${date.getDate()}`;
  return `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())}`;
}

function avatarColor(seed: string): string {
  let hash = 0;
  for (let index = 0; index < seed.length; index += 1) {
    hash = (hash * 31 + seed.charCodeAt(index)) % 360;
  }
  return `hsl(${hash} 38% 52%)`;
}

function initial(name: string): string {
  const trimmed = name.trim();
  return trimmed ? Array.from(trimmed)[0] : '?';
}

function kindBase(kind: number): number {
  return kind & 0xffff;
}

function contains(text: string, keyword: string): boolean {
  return text.toLowerCase().includes(keyword.toLowerCase());
}

/** WeChat keeps writing to the databases of the account that is signed in, so the newest
 *  write is the current account. A remembered choice still wins: someone who deliberately
 *  switched to another account should stay there. */
function currentAccount(accounts: Account[], remembered: string): Account | null {
  if (!accounts.length) return null;
  const known = remembered ? accounts.find(item => item.id === remembered) : undefined;
  if (known) return known;
  return [...accounts].sort((a, b) => (b.modified_at ?? 0) - (a.modified_at ?? 0))[0];
}

function messageKey(username: string, message: MessageItem): string {
  return `${username}:${message.source_shard}:${message.local_id}:${message.time}`;
}

function isPlaceholder(text: string): boolean {
  return text.startsWith('[') && text.endsWith(']');
}

function Highlight({ text, keyword }: { text: string; keyword: string }) {
  if (!keyword.trim()) return <>{text}</>;
  const lower = text.toLowerCase();
  const needle = keyword.toLowerCase();
  const parts: { text: string; hit: boolean }[] = [];
  let cursor = 0;
  let index = lower.indexOf(needle);
  while (index !== -1 && needle) {
    if (index > cursor) parts.push({ text: text.slice(cursor, index), hit: false });
    parts.push({ text: text.slice(index, index + needle.length), hit: true });
    cursor = index + needle.length;
    index = lower.indexOf(needle, cursor);
  }
  if (cursor < text.length) parts.push({ text: text.slice(cursor), hit: false });
  return (
    <>
      {parts.map((part, idx) =>
        part.hit ? <mark key={idx}>{part.text}</mark> : <span key={idx}>{part.text}</span>,
      )}
    </>
  );
}

function rangeStart(range: RangeKey): number | undefined {
  const now = new Date();
  if (range === 'all' || range === 'custom') return undefined;
  if (range === 'today') return Math.floor(new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime() / 1000);
  const days = range === '7d' ? 7 : 30;
  return Math.floor((now.getTime() - days * 86400000) / 1000);
}

function dateInputValue(date: Date): string {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

function dateInputEpoch(value: string, endOfDay = false): number | undefined {
  if (!value) return undefined;
  const [year, month, day] = value.split('-').map(Number);
  if (!year || !month || !day) return undefined;
  const date = endOfDay
    ? new Date(year, month - 1, day, 23, 59, 59, 999)
    : new Date(year, month - 1, day);
  return Math.floor(date.getTime() / 1000);
}

function selectionBounds(range: RangeKey, from: string, to: string): { since?: number; until?: number } {
  if (range === 'custom') {
    return { since: dateInputEpoch(from), until: dateInputEpoch(to, true) };
  }
  const since = rangeStart(range);
  return { since, until: range === 'all' ? undefined : Math.floor(Date.now() / 1000) };
}

function formatAge(seconds: number | null | undefined): string {
  if (seconds == null) return t("时间未知");
  if (seconds < 60) return t("刚刚");
  if (seconds < 3600) return tf("{0} 分钟前", Math.floor(seconds / 60));
  if (seconds < 86400) return tf("{0} 小时前", Math.floor(seconds / 3600));
  return tf("{0} 天前", Math.floor(seconds / 86400));
}

function formatKeyDate(seconds: number | null | undefined): string {
  if (!seconds) return t("时间未知");
  return new Date(seconds * 1000).toLocaleDateString(dateLocale(), {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  });
}

export default function WeChatLocal({
  mode = 'chat',
  onOpenChat,
  onOpenData,
  onOpenWhitelist,
  wizard = false,
  wizardStep = 'wizard-method',
  onWizardBack,
  onWizardNext,
  openSummary = false,
  onSummaryRequestHandled,
  focusConversation,
  focusMessageId,
  searchKeyword = '',
  onClearSearch,
  workRequest = 0,
  sessionRevision = 0,
  accountId: pinnedAccountId,
  onChatState,
}: {
  mode?: 'chat' | 'settings';
  onOpenChat?: () => void;
  onOpenData?: () => void;
  onOpenWhitelist?: () => void;
  wizard?: boolean;
  wizardStep?: string;
  onWizardBack?: () => void;
  onWizardNext?: () => void;
  openSummary?: boolean;
  onSummaryRequestHandled?: () => void;
  focusConversation?: string | null;
  focusMessageId?: string | number | null;
  /** Keyword typed in the embedding page's header. Highlights matches in the open chat. */
  searchKeyword?: string;
  onClearSearch?: () => void;
  /** Counters from the embedding page header's 整理工作内容 button. */
  workRequest?: number;
  sessionRevision?: number;
  /** The account the embedding page already resolved (WeChat or WhatsApp). */
  accountId?: string | null;
  /** Reports whether the page-level 整理/导出 actions may be shown at all. */
  onChatState?: (state: { active: boolean; messages: number; selected: number; agentReady: boolean }) => void;
} = {}) {
  // 整理工作内容 runs the local summary agent, which is an internal-account feature.
  const { internal: internalAccount } = useInternalAccount();
  const { allowed: workDigestGranted } = useCapability('ai.work_digest');
  const { allowed: aiSummaryGranted } = useCapability('ai.agent_summary');
  const { allowed: canConfigureAgent } = useCapability('ai.local_agent');
  const canOrganize = internalAccount && workDigestGranted;
  const canUseAi = internalAccount && aiSummaryGranted;
  const [env, setEnv] = useState<WechatEnv | null>(null);
  const [platform, setPlatform] = useState<PlatformCapabilities | null>(null);
  const [scan, setScan] = useState<ScanResult | null>(null);
  const [accountId, setAccountId] = useState('');
  const settingsRef = useRef<AppSettings | null>(null);
  const accountSwitchLock = useRef(false);
  const [accountSwitching, setAccountSwitching] = useState(false);
  const [keyStatus, setKeyStatus] = useState<KeyStatus | null>(null);
  const [savedKeySource, setSavedKeySource] = useState<SavedKeySource | null>(null);
  const [keyText, setKeyText] = useState('');
  const [applyWal, setApplyWal] = useState(true);
  const [onlyChat, setOnlyChat] = useState(true);

  const [busy, setBusy] = useState<'scan' | 'import' | 'decrypt' | 'messages' | 'search' | 'work' | 'explore' | 'ai' | null>(null);
  const [outcomes, setOutcomes] = useState<DecryptOutcome[]>([]);
  const [showLog, setShowLog] = useState(false);

  const [sessions, setSessions] = useState<SessionItem[]>([]);
  const [sessionsLoaded, setSessionsLoaded] = useState(false);
  const [active, setActive] = useState<SessionItem | null>(null);
  const [messages, setMessages] = useState<MessageItem[]>([]);
  /** Keyword from the embedding page's header, used for in-conversation search. */
  const [inlineKeyword, setInlineKeyword] = useState('');
  const messagesRef = useRef<MessageItem[]>([]);
  const recentMessages = useRef(new Map<string, { messages: MessageItem[]; hasMore: boolean }>());
  const sessionRequest = useRef(0);
  const deepSearchKey = useRef('');
  /** Real avatars by WeChat username; a missing entry keeps the letter avatar. */
  const [avatars, setAvatars] = useState<Record<string, string>>({});
  const avatarAsked = useRef<Set<string>>(new Set());
  const [hasMore, setHasMore] = useState(false);

  const [query, setQuery] = useState('');
  const [tab, setTab] = useState<ListTab>('all');
  const [search, setSearch] = useState<SearchReport | null>(null);

  const [workOpen, setWorkOpen] = useState(false);
  const [workPhase, setWorkPhase] = useState<WorkPhase>('select');
  const [workRange, setWorkRange] = useState<RangeKey>('7d');
  const [workFrom, setWorkFrom] = useState(() => dateInputValue(new Date(Date.now() - 7 * 86400000)));
  const [workTo, setWorkTo] = useState(() => dateInputValue(new Date()));
  const [workKeyword, setWorkKeyword] = useState('');
  const [workSearch, setWorkSearch] = useState<SearchReport | null>(null);
  const [workSelected, setWorkSelected] = useState<string[]>([]);
  const [workGroups, setWorkGroups] = useState<WorkGroup[]>([]);
  const [selectedMessages, setSelectedMessages] = useState<SelectedMessage[]>([]);
  /** Last plain click; Shift + click selects the contiguous range up to the new row. */
  const [selectionAnchor, setSelectionAnchor] = useState<{ username: string; key: string } | null>(null);
  const [workInsights, setWorkInsights] = useState<WorkInsights | null>(null);
  const [agentCurrent, setAgentCurrent] = useState<AgentCurrent | null>(null);
  const [aiSummary, setAiSummary] = useState<AgentSummary | null>(null);
  const [aiConfirm, setAiConfirm] = useState(false);
  /** The single footer entry splits into two paths; this reveals the two choices. */
  const [actionPicker, setActionPicker] = useState(false);
  const [liveSync, setLiveSync] = useState(true);
  const [wechatEnabled, setWechatEnabled] = useState(true);
  const [syncIntervalSeconds, setSyncIntervalSeconds] = useState(30);
  const [syncState, setSyncState] = useState<'idle' | 'syncing' | 'ready' | 'error'>('idle');
  const [lastSyncAt, setLastSyncAt] = useState<Date | null>(null);
  const [whitelistConfigured, setWhitelistConfigured] = useState<boolean | null>(null);
  const [showKeyGuide, setShowKeyGuide] = useState(false);
  const [windowsScanBusy, setWindowsScanBusy] = useState(false);
  useEffect(() => { if (wizard) setShowKeyGuide(true); }, [wizard]);
  const [keyImport, setKeyImport] = useState<KeyImportResult | null>(null);
  const [freshSource, setFreshSource] = useState(false);
  const [fixingPermissions, setFixingPermissions] = useState(false);
  /** In-app capture: environment/state from the backend plus the step currently running. */
  const [captureStatus, setCaptureStatus] = useState<CaptureStatus | null>(null);
  const [captureBusy, setCaptureBusy] = useState<
    'prepare' | 'open' | 'confirm' | 'preflight' | 'start' | 'restore' | null
  >(null);
  const [captureOutcome, setCaptureOutcome] = useState<CaptureImport | null>(null);
  /** The user has to acknowledge that WeChat will be re-signed and re-logged-in. */
  const [captureRiskAccepted, setCaptureRiskAccepted] = useState(false);
  /** Set when the temporary WeChat did not start, so the guide can explain Gatekeeper rules. */
  const [launchDiagnosis, setLaunchDiagnosis] = useState<LaunchDiagnosis | null>(null);
  /** Force the unlock panel so the capture guide stays reachable once keys exist. */
  const [forceUnlock, setForceUnlock] = useState(mode === 'settings');

  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [mobilePane, setMobilePane] = useState<'list' | 'chat'>('list');

  const messageRef = useRef<HTMLDivElement | null>(null);
  const messageContentRef = useRef<HTMLDivElement | null>(null);
  const followNewest = useRef(true);
  const activeRef = useRef<string | null>(null);
  const scrollIntent = useRef<'bottom' | 'preserve'>('bottom');
  const scrollSnapshot = useRef<{ top: number; height: number } | null>(null);
  const pendingHighlight = useRef<string>('');
  const pendingFocusMessage = useRef<string>('');
  const syncLock = useRef(false);
  const clearingRef = useRef(false);
  const handledWorkRequest = useRef(0);
  const sourceSeen = useRef<{ exists: boolean; modifiedAt: number | null } | null>(null);
  const [loadSeq, setLoadSeq] = useState(0);

  /** Attachment manifest for the loaded message window, keyed by `local_id`. */
  const [mediaByMessage, setMediaByMessage] = useState<Record<string, MediaItem[]>>({});
  const [mediaScanComplete, setMediaScanComplete] = useState(false);
  const [imageViewer, setImageViewer] = useState<{ src: string | null; error: string } | null>(null);
  const imageViewerRequest = useRef(0);
  const imageViewerOpener = useRef<HTMLElement | null>(null);
  const [voiceText, setVoiceText] = useState<Record<string, string>>({});
  const [voiceError, setVoiceError] = useState<Record<string, string>>({});
  const [voiceBusy, setVoiceBusy] = useState<Record<string, boolean>>({});
  const [playingVoice, setPlayingVoice] = useState<string | null>(null);
  const [loadingVoice, setLoadingVoice] = useState<string | null>(null);
  const voicePlayer = useRef<HTMLAudioElement | null>(null);
  useEffect(() => () => { voicePlayer.current?.pause(); }, [accountId, active?.username]);

  useEffect(() => {
    void invoke<{ chat?: { sync_interval_seconds?: number }; connectors?: { wechat_enabled?: boolean; whatsapp_enabled?: boolean } }>('app_settings_get')
      // The chat view refreshes far more often than the backup job: the backup interval is
      // 30 minutes by default, which would make a live conversation look frozen. Keep the
      // on-screen cadence between 10 and 30 seconds.
      .then(value => {
        setSyncIntervalSeconds(Math.min(30, Math.max(10, value.chat?.sync_interval_seconds ?? 30)));
        setWechatEnabled(pinnedAccountId?.endsWith('@s.whatsapp.net') ? (value.connectors?.whatsapp_enabled ?? true) : (value.connectors?.wechat_enabled ?? true));
      })
      .catch(() => undefined);
    // Disconnecting happens on the settings page while this page can stay mounted, so a
    // return to the window re-reads the switch and stops the live poll.
    const loadConnection = () => {
      void invoke<{ connectors?: { wechat_enabled?: boolean; whatsapp_enabled?: boolean } }>('app_settings_get')
        .then(value => setWechatEnabled(pinnedAccountId?.endsWith('@s.whatsapp.net') ? (value.connectors?.whatsapp_enabled ?? true) : (value.connectors?.wechat_enabled ?? true)))
        .catch(() => undefined);
    };
    // The agent can be configured from Settings while this page stays mounted, so a
    // return to the window is also a refresh point for the hidden actions' permission.
    const loadAgent = () => {
      void invoke<AgentCurrent>('agent_current')
        .then(setAgentCurrent)
        .catch(() => setAgentCurrent(null));
    };
    loadAgent();
    window.addEventListener('focus', loadConnection);
    window.addEventListener('focus', loadAgent);
    return () => {
      window.removeEventListener('focus', loadConnection);
      window.removeEventListener('focus', loadAgent);
    };
  }, [pinnedAccountId]);

  const account = useMemo(() => scan?.accounts.find(a => a.id === accountId) ?? null, [scan, accountId]);
  const encryptedDbs = useMemo(() => account?.dbs.filter(db => db.encrypted) ?? [], [account]);
  const keyedCount = useMemo(
    () => encryptedDbs.filter(db => keyStatus?.keys.some(k => k.account === account?.id && k.rel === db.rel)).length,
    [encryptedDbs, keyStatus, account],
  );
  const accountMissingKeys = Boolean(account && encryptedDbs.length > 0 && keyedCount < encryptedDbs.length);
  const noKeysYet = Boolean(account && encryptedDbs.length > 0 && keyedCount === 0);
  const keyVerifiedSecs = useMemo(() => {
    if (!account) return null;
    const stamps = (keyStatus?.keys ?? [])
      .filter(key => key.account === account.id && key.verified_secs)
      .map(key => key.verified_secs as number);
    return stamps.length ? Math.max(...stamps) : null;
  }, [keyStatus, account]);
  const keyAgeDays = keyVerifiedSecs
    ? Math.max(0, Math.floor((Date.now() / 1000 - keyVerifiedSecs) / 86400))
    : null;
  const ready = sessionsLoaded && sessions.length > 0;
  const locked = sessionsLoaded && sessions.length === 0;
  const needsWhitelist = sessionsLoaded && whitelistConfigured === false;

  const refreshKeys = useCallback(async () => {
    setKeyStatus(await invoke<KeyStatus>('wechat_key_status'));
  }, []);

  const refreshSavedKeySource = useCallback(async () => {
    try {
      const next = await invoke<SavedKeySource>('wechat_key_source_status');
      const seen = sourceSeen.current;
      if (seen && next.exists && (!seen.exists || (next.modified_at ?? 0) > (seen.modifiedAt ?? 0))) {
        setFreshSource(true);
        setNotice(t("检测到新的口令文件，可以导入并校验。"));
      }
      sourceSeen.current = { exists: next.exists, modifiedAt: next.modified_at ?? null };
      setSavedKeySource(next);
    } catch {
      /* The probe is advisory: a missing home directory must not block the page. */
    }
  }, []);

  const loadSessions = useCallback(
    async (id: string) => {
      if (!id) {
        setWhitelistConfigured(null);
        return;
      }
      try {
        const stored = await invoke<{ chat?: { allowed_usernames?: Record<string, string[]> } }>('app_settings_get');
        const allowed = stored.chat?.allowed_usernames?.[id] ?? [];
        setWhitelistConfigured(allowed.length > 0);
        const list = await invoke<SessionItem[]>('wechat_sessions', { accountId: id });
        setSessions(list);
        setSessionsLoaded(true);
        return list;
      } catch (e) {
        const message = String(e);
        setSessions([]);
        setSessionsLoaded(true);
        setWhitelistConfigured(null);
        // Before the first decrypt there is simply no session.db; that is the
        // onboarding state, not an error worth covering the key guide with.
        if (!message.includes(t("未找到解密后的"))) setError(message);
        return [];
      }
    },
    [],
  );

  useEffect(() => {
    if (sessionRevision > 0 && accountId) void loadSessions(accountId);
  }, [sessionRevision, accountId, loadSessions]);

  const syncLatest = useCallback(async () => {
    if (!account || syncLock.current || clearingRef.current) return;
    syncLock.current = true;
    setSyncState('syncing');
    try {
      // WhatsApp's store is already plaintext: there is nothing to decrypt, so the tick
      // only re-reads the snapshot-backed session and message lists.
      if (!account.id.endsWith('@s.whatsapp.net')) {
        const outcomes = await invoke<DecryptOutcome[]>('wechat_sync', { accountId: account.id });
        const usable = outcomes.some(outcome => outcome.status === 'ok' || outcome.status === 'cached');
        if (!usable) {
          setSyncState('error');
          return;
        }
      }
      const list = await invoke<SessionItem[]>('wechat_sessions', { accountId: account.id });
      setSessions(list);
      setSessionsLoaded(true);
      if (active) {
        const latest = await invoke<MessageItem[]>('wechat_messages', {
          accountId: account.id,
          username: active.username,
          limit: PAGE,
        });
        if (activeRef.current !== active.username) return;
        const changed = latest.length !== Math.min(messages.length, PAGE)
          || latest[0]?.local_id !== messages[0]?.local_id
          || latest[0]?.time !== messages[0]?.time;
        if (changed) {
          const root = messageRef.current;
          scrollIntent.current = root && root.scrollHeight - root.scrollTop - root.clientHeight > 80 ? 'preserve' : 'bottom';
          if (root) scrollSnapshot.current = { top: root.scrollTop, height: root.scrollHeight };
          setMessages(current => {
            const keys = new Set(latest.map(message => `${message.source_shard}:${message.local_id}:${message.time}`));
            return [...latest, ...current.filter(message => !keys.has(`${message.source_shard}:${message.local_id}:${message.time}`))];
          });
          setHasMore(current => current || latest.length >= PAGE);
        }
      }
      setLastSyncAt(new Date());
      setSyncState('ready');
    } catch {
      setSyncState('error');
    } finally {
      syncLock.current = false;
    }
  }, [account, active, messages]);

  useEffect(() => {
    if (!liveSync || !account || !ready || !wechatEnabled) return;
    void syncLatest();
    const timer = window.setInterval(() => {
      if (clearingRef.current) return;
      void syncLatest();
    }, syncIntervalSeconds * 1000);
    return () => window.clearInterval(timer);
  }, [account, liveSync, ready, wechatEnabled, syncLatest, syncIntervalSeconds]);

  /** Do not present a different account until the backend has accepted the selection. */
  const rememberAccount = useCallback(async (id: string) => {
    const stored = settingsRef.current;
    if (!stored) throw new Error(t("设置尚未加载，无法切换账号；请重新打开页面。"));
    if (stored.active_account_id === id) return;
    try {
      const saved = requireSettingsRevision(await invoke<AppSettings>('app_settings_save', { value: { ...stored, active_account_id: id } }));
      if (saved.active_account_id !== id) {
        settingsRef.current = null;
        throw new Error(t("后端未确认所选微信账号，请重新打开页面。"));
      }
      settingsRef.current = saved;
    } catch (reason) {
      // A stale snapshot cannot establish which account the backend now uses.
      if (isSettingsConflict(reason)) settingsRef.current = null;
      throw reason;
    }
  }, []);

  /** Fetch the avatars we do not have yet, one batch per call, and never ask twice. */
  const ensureAvatars = useCallback((id: string, names: Array<string | null | undefined>) => {
    const missing = [...new Set(names.filter((name): name is string => Boolean(name)))].filter(
      name => !avatarAsked.current.has(name),
    );
    if (!missing.length) return;
    missing.forEach(name => avatarAsked.current.add(name));
    for (let index = 0; index < missing.length; index += 120) {
      const batch = missing.slice(index, index + 120);
      void invoke<Array<{ username: string; data_url: string }>>('wechat_avatars', {
        accountId: id,
        usernames: batch,
      })
        .then(rows => {
          if (!rows.length) return;
          setAvatars(previous => {
            const next = { ...previous };
            rows.forEach(row => {
              next[row.username] = row.data_url;
            });
            return next;
          });
        })
        .catch(() => undefined);
    }
  }, []);

  const doScan = useCallback(async (preferredId?: string) => {
    if (accountSwitchLock.current) return;
    accountSwitchLock.current = true;
    setAccountSwitching(true);
    setBusy('scan');
    setError('');
    try {
      const detect = await invoke<{ state: string }>('connector_detect', { kind: 'wechat' });
      const result = await invoke<ScanResult>('wechat_scan');
      setScan(result);
      const nextId = pinnedAccountId
        ? result.accounts.find(item => item.id === pinnedAccountId)?.id ?? ''
        : currentAccount(result.accounts, preferredId ?? settingsRef.current?.active_account_id ?? '')?.id ?? '';
      if (nextId && !pinnedAccountId) await rememberAccount(nextId);
      setAccountId(nextId);
      await refreshKeys();
      if (!result.accounts.length && detect?.state === 'installed') {
        setError(t("没有找到微信 4.x 数据目录。若微信已登录，请按下方的文字步骤授予「完全磁盘访问权限」后重新检测。"));
      }
      if (nextId) await loadSessions(nextId);
    } catch (e) {
      setError(tf("账号未切换：{0}", settingsSaveError(e)));
    } finally {
      accountSwitchLock.current = false;
      setAccountSwitching(false);
      setBusy(null);
    }
  }, [loadSessions, refreshKeys, rememberAccount, pinnedAccountId]);

  useEffect(() => {
    void (async () => {
      try {
        setEnv(await invoke<WechatEnv>('wechat_env'));
        await refreshSavedKeySource();
      } catch {
        /* not fatal */
      }
      // WhatsApp has no key, no capture and no database scan: the embedding page already
      // resolved the account, so pin it and read the whitelisted sessions directly.
      if (pinnedAccountId && pinnedAccountId.endsWith('@s.whatsapp.net')) {
        setScan({
          roots: [],
          skipped: [],
          accounts: [{ id: pinnedAccountId, dir: '', db_count: 0, encrypted_count: 0, total_bytes: 0, modified_at: null, dbs: [] }],
        });
        setAccountId(pinnedAccountId);
        try {
          settingsRef.current = requireSettingsRevision(await invoke<AppSettings>('app_settings_get'));
          setWechatEnabled(settingsRef.current.connectors?.whatsapp_enabled !== false);
        } catch { /* the session read below reports the real problem */ }
        await loadSessions(pinnedAccountId);
        return;
      }
      // Read the remembered account before scanning: with more than one WeChat account on
      // this Mac the saved choice decides which one opens, instead of asking every time.
      try {
        settingsRef.current = requireSettingsRevision(await invoke<AppSettings>('app_settings_get'));
        await doScan(settingsRef.current.active_account_id || undefined);
      } catch (reason) {
        setError(tf("设置加载失败，无法选择微信账号：{0}。请重新打开页面。", String(reason)));
      }
    })();
    // Scan once per pinned account; account switching and manual rescans call doScan directly.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pinnedAccountId]);

  /** Sessions and message senders are the only things that own an avatar on screen. */
  useEffect(() => {
    if (!accountId || !sessions.length) return;
    ensureAvatars(accountId, sessions.map(session => session.username));
  }, [accountId, sessions, ensureAvatars]);

  useEffect(() => {
    if (!accountId || !messages.length) return;
    ensureAvatars(accountId, messages.map(message => message.sender));
  }, [accountId, messages, ensureAvatars]);

  useEffect(() => {
    if (noKeysYet) setShowKeyGuide(true);
  }, [accountId, noKeysYet]);

  // While the guide is open (or before any key exists) watch the extractor's well-known
  // output file. It is a cheap local stat; no chat data and no passphrase is read here.
  useEffect(() => {
    if (!showKeyGuide && !noKeysYet) return;
    void refreshSavedKeySource();
    const timer = window.setInterval(() => void refreshSavedKeySource(), 3000);
    return () => window.clearInterval(timer);
  }, [showKeyGuide, noKeysYet, refreshSavedKeySource]);

  const switchAccount = useCallback(
    async (nextId: string) => {
      if (accountSwitchLock.current || nextId === accountId) return;
      accountSwitchLock.current = true;
      setAccountSwitching(true);
      setError('');
      try {
        await rememberAccount(nextId);
        setAccountId(nextId);
        setSessions([]);
        setSessionsLoaded(false);
        setActive(null);
        setMessages([]);
        setOutcomes([]);
        setSelectedMessages([]);
        setSelectionAnchor(null);
        setWorkGroups([]);
        setWorkInsights(null);
        setKeyImport(null);
        avatarAsked.current.clear();
        setAvatars({});
        await loadSessions(nextId);
      } catch (reason) {
        setError(tf("账号未切换：{0}", settingsSaveError(reason)));
      } finally {
        accountSwitchLock.current = false;
        setAccountSwitching(false);
      }
    },
    [accountId, loadSessions, rememberAccount],
  );

  const applyImportResult = async (result: KeyImportResult) => {
    setKeyImport(result);
    setFreshSource(false);
    setNotice(result.message);
    // Only wipe the box when the pasted material actually stored something; a rejected paste
    // stays visible so it can be corrected instead of retyped from memory.
    if (result.matched > 0) setKeyText('');
    if (result.unmatched.length) {
      setError(tf("有 {0} 个数据库没有匹配到密钥（微信可能从未打开过它们）。", result.unmatched.length));
    }
    await refreshKeys();
  };

  const importKeys = async (file?: File) => {
    setError('');
    setNotice('');
    let text = keyText;
    if (file) {
      if (file.size > 2_000_000) {
        setError(t("密钥文件过大，请确认选择的是口令或密钥 JSON。"));
        return;
      }
      text = await file.text();
      setKeyText(text);
    }
    if (!text.trim()) {
      setError(t("请粘贴密钥或口令，或选择口令 JSON 文件。"));
      return;
    }
    setBusy('import');
    try {
      const result = await invoke<KeyImportResult>('wechat_import_keys', {
        text,
        accountId: account?.id ?? null,
      });
      await applyImportResult(result);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const scanWindowsKeys = async () => {
    setWindowsScanBusy(true); setError(''); setNotice('');
    try {
      const result = await invoke<KeyImportResult>('wechat_windows_key_scan', { accountId: account?.id ?? null });
      setKeyImport(result); await refreshKeys();
      setNotice(result.message || t("Windows 密钥已校验并导入。"));
    } catch (reason) { setError(String(reason)); }
    finally { setWindowsScanBusy(false); }
  };

  const importSavedKey = async () => {
    setError('');
    setNotice('');
    setBusy('import');
    try {
      const result = await invoke<KeyImportResult>('wechat_import_saved_key', {
        accountId: account?.id ?? null,
      });
      await applyImportResult(result);
      await refreshSavedKeySource();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const fixKeyPermissions = async () => {
    setError('');
    setFixingPermissions(true);
    try {
      const next = await invoke<SavedKeySource>('wechat_fix_key_permissions');
      setSavedKeySource(next);
      setNotice(tf("口令文件权限已改为 {0}，只有当前用户可读。", next.mode ?? '600'));
    } catch (e) {
      setError(String(e));
    } finally {
      setFixingPermissions(false);
    }
  };

  /* ── in-app capture ───────────────────────────────────────────────────── */

  /** Backend errors are structured; show the message, not a JSON blob. */
  const captureErrorText = (e: unknown): string => {
    if (typeof e === 'string') return e;
    if (e && typeof e === 'object' && 'message' in e) return String((e as { message: unknown }).message);
    return String(e);
  };

  const refreshCaptureStatus = useCallback(async () => {
    try {
      setCaptureStatus(await invoke<CaptureStatus>('wechat_capture_status'));
    } catch {
      /* The status probe is advisory; a transient failure must not cover the guide. */
    }
  }, []);

  useEffect(() => {
    if (!showKeyGuide && !noKeysYet) return;
    void refreshCaptureStatus();
    const timer = window.setInterval(() => void refreshCaptureStatus(), 1500);
    return () => window.clearInterval(timer);
  }, [showKeyGuide, noKeysYet, refreshCaptureStatus]);

  /** Verify, back up and temporarily re-sign WeChat. Nothing is launched here. */
  const prepareCapture = async () => {
    setError('');
    setLaunchDiagnosis(null);
    setNotice(t("正在校验微信签名并建立原版备份，可能需要几分钟，请不要关闭窗口…"));
    setCaptureBusy('prepare');
    try {
      const result = await invoke<PrepareResult>('wechat_capture_prepare');
      const sanitized =
        result.entitlement_mode === 'deep'
          ? t(" 本次临时副本及内部组件会被重签，并以无沙箱模式运行，可能使用不同的数据目录。结束或取消时会恢复原版。")
          : result.entitlement_mode === 'sanitized'
          ? tf(" 这台 Mac 会拒绝 ad-hoc 签名携带腾讯团队的受限权限，本次临时副本已剔除 {0} 项受限权限（{1}），否则微信在启动前就会被系统拒绝；结束或取消时会换回腾讯原版。", result.dropped_entitlements.length, result.dropped_entitlements.join('、'))
          : '';
      setNotice(
        tf("已备份微信 {0}（{1}）并完成临时重签。{2}下一步：打开微信并完整登录要读取的账号。", result.version, result.build, sanitized),
      );
    } catch (e) {
      setError(captureErrorText(e));
    } finally {
      setCaptureBusy(null);
      await refreshCaptureStatus();
    }
  };

  const openCaptureWeChat = async () => {
    setError('');
    setCaptureBusy('open');
    try {
      const result = await invoke<LaunchAttempt>('wechat_capture_open');
      setLaunchDiagnosis(result.launched ? null : result.diagnosis);
      if (result.launched) {
        setNotice(t("已打开临时微信。请等登录界面出现并完整登录，然后点「我已登录，检查启动」。"));
      } else {
        // The card carries the `open` stderr and the fix, so no error toast that could cover it.
        setNotice(
          result.diagnosis.amfi_rejected
            ? t("这台 Mac 在启动前就拒绝了本次临时签名，无法通过放行解决，请按下面的说明恢复微信。")
            : t("系统没有确认微信启动，请按下面的步骤放行后再试。"),
        );
      }
    } catch (e) {
      setError(captureErrorText(e));
    } finally {
      setCaptureBusy(null);
    }
  };

  const openPrivacySecuritySettings = async () => {
    try {
      await invoke('wechat_open_privacy_security_settings');
      setNotice(t("已打开「隐私与安全性」。请在「安全性」区域找到被阻止的微信，点「仍要打开」。"));
    } catch (e) {
      setError(captureErrorText(e));
    }
  };

  const confirmCaptureLaunch = async () => {
    const transaction = captureStatus?.transaction_id;
    if (!transaction) {
      setError(t("没有找到本次捕获事务，请先点「准备并临时重签」。"));
      return;
    }
    setError('');
    setCaptureBusy('confirm');
    try {
      const check = await invoke<LaunchCheck>('wechat_capture_confirm_launch', { transactionId: transaction });
      if (check.ready_for_preflight) {
        setLaunchDiagnosis(null);
        setNotice(t("已检测到临时微信进程。下一步检查捕获断点。"));
      } else {
        setLaunchDiagnosis(check.diagnosis);
        setNotice(t("还没有检测到稳定的微信进程，原因见下方说明。"));
      }
    } catch (e) {
      setError(captureErrorText(e));
    } finally {
      setCaptureBusy(null);
      await refreshCaptureStatus();
    }
  };

  /** Brief attach with one administrator prompt; the user still logs out *after* this. */
  const runCapturePreflight = async () => {
    setError('');
    setNotice(t("即将弹出管理员授权对话框：请输入本机密码，密码只交给 macOS，本应用不会读取或保存。"));
    setCaptureBusy('preflight');
    try {
      const result = await invoke<PreflightResult>('wechat_capture_preflight');
      setNotice(tf("断点可用（{0} 处）。现在请在微信里退出账号，然后启动监测。", result.pbkdf_locations));
    } catch (e) {
      setError(captureErrorText(e));
    } finally {
      setCaptureBusy(null);
      await refreshCaptureStatus();
    }
  };

  /** Long-running: attach LLDB, wait for the next login, validate, restore and import. */
  const startCapture = async () => {
    if (!account) return;
    setError('');
    setNotice(
      captureStatus?.resume_available
        ? t("正在校验上次已捕获的密钥，随后恢复腾讯原版微信并导入。")
        : t("即将弹出管理员授权对话框：请输入本机密码。授权后保持微信窗口打开，等待界面显示「监测已就绪」再登录。"),
    );
    setCaptureBusy('start');
    try {
      const result = await invoke<CaptureImport>('wechat_capture_start', {
        accountId: account.id,
        timeoutSecs: 300,
      });
      setCaptureOutcome(result);
      setKeyImport({
        matched: result.matched,
        already: result.already,
        unmatched: [],
        accounts: result.accounts,
        message: result.message,
      });
      setNotice(result.message);
      await refreshKeys();
      await refreshSavedKeySource();
    } catch (e) {
      setError(captureErrorText(e));
    } finally {
      setCaptureBusy(null);
      await refreshCaptureStatus();
    }
  };

  /** Cancel an in-flight capture or recover from a failed one. */
  const restoreWeChat = async () => {
    setError('');
    setLaunchDiagnosis(null);
    setCaptureBusy('restore');
    try {
      const result = await invoke<RestoreResult>('wechat_capture_restore');
      setNotice(
        result.restored
          ? tf("已恢复腾讯原版微信 {0}（{1}），可以重新打开微信。", result.version, result.build)
          : tf("当前微信已经是腾讯原版 {0}，无需恢复。", result.version),
      );
      await refreshCaptureStatus();
    } catch (e) {
      setError(captureErrorText(e));
    } finally {
      setCaptureBusy(null);
    }
  };

  /** Drop every view derived from the decrypted snapshot (used when that snapshot is deleted). */
  const resetDecryptedView = () => {
    setSessions([]);
    // With no decrypted copy left, the page must fall back to the unlock panel instead of
    // the "正在准备…" placeholder. `locked` is derived from sessionsLoaded + an empty list.
    setSessionsLoaded(true);
    setActive(null);
    setMessages([]);
    setOutcomes([]);
    setSelectedMessages([]);
    setSelectionAnchor(null);
    setWorkGroups([]);
    setWorkInsights(null);
    setWorkOpen(false);
    setWorkPhase('select');
    setSearch(null);
    setQuery('');
    setWhitelistConfigured(null);
  };

  // `window.confirm` is not surfaced by the macOS webview (it silently returns false), so a
  // destructive action needs its own confirmation step in the page instead of a native dialog.
  const [clearConfirm, setClearConfirm] = useState(false);

  const clearKeys = async () => {
    if (!account) return;
    setClearConfirm(false);
    setError('');
    setBusy('import');
    // Same guard as `clearCache`: live sync must not re-create the copy mid-request.
    clearingRef.current = true;
    try {
      const text = await invoke<string>('wechat_clear_keys', { accountId: account.id });
      setNotice(text);
      setKeyImport(null);
      setFreshSource(false);
      resetDecryptedView();
      await refreshKeys();
    } catch (e) {
      setError(String(e));
    } finally {
      window.setTimeout(() => {
        clearingRef.current = false;
      }, 0);
      setBusy(null);
    }
  };

  const clearCache = async () => {
    if (!account) return;
    setError('');
    // Live sync runs on an interval; without this guard it can re-create the copy while the
    // clear request is in flight, making the action look like a no-op.
    clearingRef.current = true;
    try {
      const text = await invoke<string>('wechat_clear_cache', { accountId: account.id });
      recentMessages.current.clear();
      setNotice(text);
      resetDecryptedView();
      await refreshKeys();
    } catch (e) {
      setError(String(e));
    } finally {
      // Release after the state updates have been committed so the next interval tick does
      // not race the switch back to the onboarding view.
      window.setTimeout(() => {
        clearingRef.current = false;
      }, 0);
    }
  };

  const decrypt = async () => {
    if (!account) return;
    setError('');
    setNotice('');
    setBusy('decrypt');
    try {
      const result = await invoke<DecryptOutcome[]>('wechat_decrypt', {
        accountId: account.id,
        applyWal,
        onlyChat,
      });
      setOutcomes(result);
      const ok = result.filter(o => o.status === 'ok' || o.status === 'cached').length;
      const failed = result.length - ok;
      setNotice(tf("解密完成：{0} 个数据库可用{1}。", ok, failed ? `，${failed} 个失败或缺少密钥` : ''));
      if (ok > 0) {
        setBusy('messages');
        await loadSessions(account.id);
      } else if (result.length > 0) {
        // Nothing decrypted at all: the most likely cause is a key for another account or a
        // rotated passphrase, so surface the acquisition guide instead of a silent failure.
        setForceUnlock(true);
        setShowKeyGuide(true);
        setError(t("没有任何数据库解密成功，通常是密钥与当前账号不匹配或已轮换。请重新捕获口令后再导入。"));
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const openSession = useCallback(
    async (session: SessionItem, keyword = '', focusMessageId?: string | number) => {
      if (!account) return;
      const request = ++sessionRequest.current;
      const cacheKey = `${account.id}\u0000${session.username}`;
      const cached = recentMessages.current.get(cacheKey);
      if (cached) {
        recentMessages.current.delete(cacheKey);
        recentMessages.current.set(cacheKey, cached);
      }
      activeRef.current = session.username;
      scrollIntent.current = 'bottom';
      scrollSnapshot.current = null;
      followNewest.current = true;
      setMediaScanComplete(false);
      setMediaByMessage({});
      setActive(session);
      setMessages(cached?.messages ?? []);
      setHasMore(cached?.hasMore ?? false);
      setError('');
      setMobilePane('chat');
      setBusy('messages');
      setLoadSeq(seq => seq + 1);
      try {
        let list = await invoke<MessageItem[]>('wechat_messages', {
          accountId: account.id,
          username: session.username,
          limit: PAGE,
        });
        // A keyword hit can be older than the first page; pull more pages so the
        // highlighted message is actually on screen instead of silently missing. Twelve
        // pages covers ~1200 recent messages, which keeps the search responsive while
        // handling month-old links in busy groups.
        let page = list;
        for (let attempt = 0; keyword && attempt < 12 && page.length === PAGE; attempt += 1) {
          if (list.some(message => contains(message.text, keyword))) break;
          const last = page[page.length - 1];
          if (!last) break;
          page = await invoke<MessageItem[]>('wechat_messages', {
            accountId: account.id,
            username: session.username,
            limit: PAGE,
            before: last.time,
            beforeId: last.local_id,
            beforeShard: last.source_shard,
          });
          if (!page.length) break;
          list = [...list, ...page];
        }
        if (request !== sessionRequest.current || activeRef.current !== session.username) return;
        const root = messageRef.current;
        if (cached && root && !followNewest.current) {
          scrollIntent.current = 'preserve';
          scrollSnapshot.current = { top: root.scrollTop, height: root.scrollHeight };
        }
        const keys = new Set(list.map(message => `${message.source_shard}:${message.local_id}:${message.time}`));
        const overlaps = cached?.messages.some(message => keys.has(`${message.source_shard}:${message.local_id}:${message.time}`));
        const merged = cached && !keyword && focusMessageId == null && (list.length < PAGE || overlaps)
          ? [...list, ...cached.messages.filter(message => !keys.has(`${message.source_shard}:${message.local_id}:${message.time}`))]
          : list;
        setMessages(merged);
        setHasMore(Boolean(cached?.hasMore) || list.length >= PAGE);
        pendingHighlight.current = keyword;
      pendingFocusMessage.current = focusMessageId == null ? '' : String(focusMessageId);
      } catch (e) {
        if (request === sessionRequest.current && activeRef.current === session.username) setError(String(e));
      } finally {
        if (request === sessionRequest.current && activeRef.current === session.username) setBusy(null);
      }
    },
    [account],
  );

  useEffect(() => {
    if (!accountId || !active || !messages.length || activeRef.current !== active.username) return;
    const key = `${accountId}\u0000${active.username}`;
    recentMessages.current.delete(key);
    recentMessages.current.set(key, { messages: messages.slice(0, 500), hasMore: hasMore || messages.length > 500 });
    while (recentMessages.current.size > 10) {
      const oldest = recentMessages.current.keys().next().value;
      if (oldest === undefined) break;
      recentMessages.current.delete(oldest);
    }
  }, [accountId, active, messages, hasMore]);

  /** Attachments follow the loaded window; debounced so paging does not re-scan twice. */
  useEffect(() => {
    if (!account || !active || !messages.length) {
      setMediaByMessage({});
      return;
    }
    // WhatsApp message types do not use WeChat's media identifiers. Its reader already
    // supplies a readable label, so do not try to resolve them against a WeChat database.
    if (account.id.endsWith('@s.whatsapp.net')) {
      setMediaByMessage({});
      setMediaScanComplete(true);
      return;
    }
    const since = messages[messages.length - 1].time;
    const until = messages[0].time;
    let cancelled = false;
    const timer = window.setTimeout(() => {
      void invoke<MediaReport>('wechat_message_media', {
        accountId: account.id,
        username: active.username,
        since,
        until,
      })
        .then(report => {
          if (cancelled) return;
          const map: Record<string, MediaItem[]> = {};
          for (const item of report.items) {
            (map[`${item.source_shard}:${item.local_id}:${item.time}`] ??= []).push(item);
          }
          setMediaByMessage(map);
          setMediaScanComplete(true);
        })
        .catch(() => {
          if (!cancelled) { setMediaByMessage({}); setMediaScanComplete(true); }
        });
    }, 350);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [account?.id, active?.username, messages]);

  const mediaFor = (message: MessageItem): MediaItem[] => mediaByMessage[`${message.source_shard}:${message.local_id}:${message.time}`] ?? [];

  const transcribeVoice = async (item: MediaItem) => {
    if (!account || !active) return;
    setVoiceBusy(current => ({ ...current, [item.server_id_exact]: true }));
    setVoiceError(current => {
      const next = { ...current };
      delete next[item.server_id_exact];
      return next;
    });
    try {
      const results = await invoke<VoiceResult[]>('wechat_voice_transcribe', {
        accountId: account.id,
        username: active.username,
        items: [{ local_id: item.local_id, server_id: item.server_id_exact }],
      });
      const result = results[0];
      if (result?.text) {
        setVoiceText(current => ({ ...current, [item.server_id_exact]: result.text as string }));
      } else if (result?.error) {
        setVoiceError(current => ({ ...current, [item.server_id_exact]: result.error as string }));
      }
    } catch (e) {
      setVoiceError(current => ({ ...current, [item.server_id_exact]: String(e) }));
    } finally {
      setVoiceBusy(current => ({ ...current, [item.server_id_exact]: false }));
    }
  };

  const playVoice = async (item: MediaItem) => {
    if (!account || !active) return;
    if (loadingVoice !== null) return;
    if (playingVoice === item.server_id_exact) {
      voicePlayer.current?.pause();
      setPlayingVoice(null);
      return;
    }
    try {
      voicePlayer.current?.pause();
      setLoadingVoice(item.server_id_exact);
      setVoiceError(current => { const next = { ...current }; delete next[item.server_id_exact]; return next; });
      const audio = await invoke<{ data_uri: string; duration_secs: number }>('wechat_voice_audio', {
        accountId: account.id,
        username: active.username,
        serverId: item.server_id_exact,
      });
      const player = new Audio(audio.data_uri);
      voicePlayer.current = player;
      player.onended = () => setPlayingVoice(null);
      player.onerror = () => { setPlayingVoice(null); setVoiceError(current => ({ ...current, [item.server_id_exact]: t("语音播放失败，请重试。") })); };
      await player.play();
      setPlayingVoice(item.server_id_exact);
    } catch (e) {
      setVoiceError(current => ({ ...current, [item.server_id_exact]: String(e) }));
    } finally {
      setLoadingVoice(null);
    }
  };

  const revealMedia = (item: MediaItem) => {
    if (!account || !item.rel_path) return;
    void invoke('wechat_media_reveal', { accountId: account.id, path: item.rel_path }).catch(e => setError(String(e)));
  };

  const closeImage = useCallback(() => {
    imageViewerRequest.current += 1;
    setImageViewer(null);
    imageViewerOpener.current?.focus();
  }, []);

  useEffect(() => {
    if (!imageViewer) return;
    const handleKey = (event: KeyboardEvent) => { if (event.key === 'Escape') closeImage(); };
    window.addEventListener('keydown', handleKey);
    return () => window.removeEventListener('keydown', handleKey);
  }, [imageViewer, closeImage]);

  const openImage = (item: MediaItem) => {
    const paths = item.sources.length ? item.sources : item.rel_path ? [item.rel_path] : [];
    if (!account || !paths.length) return;
    imageViewerOpener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const request = ++imageViewerRequest.current;
    setImageViewer({ src: null, error: '' });
    void invoke<string>('wechat_media_open_image', { accountId: account.id, paths })
      .then(path => { if (imageViewerRequest.current === request) setImageViewer({ src: convertFileSrc(path), error: '' }); })
      .catch(reason => { if (imageViewerRequest.current === request) setImageViewer({ src: null, error: tf("无法查看图片：{0}", String(reason)) }); });
  };

  const loadOlder = async () => {
    if (!account || !active || !messages.length) return;
    const last = messages[messages.length - 1];
    setBusy('messages');
    try {
      const older = await invoke<MessageItem[]>('wechat_messages', {
        accountId: account.id,
        username: active.username,
        limit: PAGE,
        before: last.time,
        beforeId: last.local_id,
        beforeShard: last.source_shard,
      });
      if (activeRef.current !== active.username) return;
      const root = messageRef.current;
      scrollIntent.current = 'preserve';
      if (root) scrollSnapshot.current = { top: root.scrollTop, height: root.scrollHeight };
      setMessages(current => [...current, ...older]);
      setHasMore(older.length >= PAGE);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  // Scroll to the newest message, or to the first keyword hit when a search result opened.
  useLayoutEffect(() => {
    const root = messageRef.current;
    if (!root) return;
    const keyword = pendingHighlight.current;
    const focusMessageId = pendingFocusMessage.current;
    pendingHighlight.current = '';
    pendingFocusMessage.current = '';
    if (focusMessageId) {
      const focused = root.querySelector<HTMLElement>(`#msg-${CSS.escape(focusMessageId)}`);
      if (focused) { followNewest.current = false; focused.scrollIntoView({ block: 'center' }); focused.classList.add('wxv-message-focus'); window.setTimeout(() => focused.classList.remove('wxv-message-focus'), 2400); return; }
    }
    if (keyword) {
      const hit = Array.from(root.querySelectorAll<HTMLElement>('[data-hit="true"]'))[0];
      if (hit) {
        followNewest.current = false;
        hit.scrollIntoView({ block: 'center' });
        return;
      }
    }
    if (scrollIntent.current === 'preserve' && scrollSnapshot.current) {
      root.scrollTop = scrollSnapshot.current.top + root.scrollHeight - scrollSnapshot.current.height;
    } else { followNewest.current = true; root.scrollTop = root.scrollHeight; }
    scrollSnapshot.current = null;
  }, [messages, loadSeq]);

  useLayoutEffect(() => {
    const root = messageRef.current;
    const content = messageContentRef.current;
    if (!root || !content || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => {
      if (followNewest.current && root.clientHeight > 0) root.scrollTop = root.scrollHeight;
    });
    observer.observe(content);
    return () => observer.disconnect();
  }, [active?.username]);

  const runSearch = async () => {
    if (!account) return;
    const keyword = query.trim();
    if (!keyword) {
      setSearch(null);
      return;
    }
    setBusy('search');
    setError('');
    try {
      const report = await invoke<SearchReport>('wechat_search', {
        accountId: account.id,
        keyword,
        groupsOnly: tab === 'group',
        maxSessions: 200,
      });
      setSearch(report);
    } catch (e) {
      setError(String(e));
      setSearch(null);
    } finally {
      setBusy(null);
    }
  };

  const clearSearch = () => {
    setSearch(null);
    setQuery('');
  };

  const toggleMessageSelection = (message: MessageItem, extend = false) => {
    if (!active) return;
    const key = messageKey(active.username, message);

    // Shift extends from the last plain click to here, taking everything between the two
    // rows in the order they are displayed. Other sessions' picks stay untouched, so a
    // range can be added to a cross-conversation selection instead of replacing it.
    if (extend && selectionAnchor?.username === active.username) {
      const keys = orderedWithDates.map(row => messageKey(active.username, row.message));
      const from = keys.indexOf(selectionAnchor.key);
      const to = keys.indexOf(key);
      if (from >= 0 && to >= 0) {
        const [start, end] = from <= to ? [from, to] : [to, from];
        const range = orderedWithDates.slice(start, end + 1).map(row => row.message);
        setSelectedMessages(current => {
          const seen = new Set(current.map(item => messageKey(item.session.username, item.message)));
          const additions = range
            .filter(item => !seen.has(messageKey(active.username, item)))
            .map(item => ({ session: active, message: item }));
          return additions.length ? [...current, ...additions] : current;
        });
        setWorkInsights(null);
        return;
      }
    }

    setSelectionAnchor({ username: active.username, key });
    setSelectedMessages(current => {
      if (current.some(item => messageKey(item.session.username, item.message) === key)) {
        return current.filter(item => messageKey(item.session.username, item.message) !== key);
      }
      return [...current, { session: active, message }];
    });
  };

  const clearSelectedMessages = () => {
    setSelectedMessages([]);
    setSelectionAnchor(null);
    setWorkInsights(null);
    setNotice(t("已清空消息选集。"));
  };

  const openWorkPicker = () => {
    if (!canOrganize) return;
    setWorkPhase('select');
    setWorkOpen(true);
    setError('');
    setWorkInsights(null);
    setAiSummary(null);
    setAiConfirm(false);
    setActionPicker(false);
    if (selectedMessages.length) {
      setWorkSelected(Array.from(new Set(selectedMessages.map(item => item.session.username))));
    } else if (active && !workSelected.includes(active.username)) {
      setWorkSelected(current => [...current, active.username]);
    }
  };

  useEffect(() => {
    if (openSummary && ready && !workOpen) {
      openWorkPicker();
      onSummaryRequestHandled?.();
    }
  }, [openSummary, ready]);

  useEffect(() => {
    if (!focusConversation || !ready || !account || !sessions.length) return;
    const session = sessions.find(item => item.username === focusConversation);
    if (!session) return;
    // The session list is refreshed by the live-sync tick; reopening the conversation we are
    // already on would clear the message list and make the pane flash every tick.
    if (active?.username === focusConversation && !focusMessageId) return;
    void openSession(session, '', focusMessageId ?? undefined);
  }, [focusConversation, focusMessageId, ready, account, sessions, openSession, active?.username]);

  useEffect(() => { messagesRef.current = messages; }, [messages]);

  // Every conversation starts a fresh deep-search budget; otherwise a keyword already
  // searched in the previous chat would suppress the search in the new one.
  useEffect(() => { deepSearchKey.current = ''; }, [active?.username]);

  // Header search from the embedding page: highlight matches in the open conversation and,
  // when the loaded window has none, pull older pages for this keyword exactly once.
  useEffect(() => {
    const keyword = searchKeyword.trim();
    setInlineKeyword(keyword);
    if (!keyword || !active) {
      deepSearchKey.current = '';
      return;
    }
    const key = `${active.username}|${keyword}`;
    if (deepSearchKey.current === key) return;
    const timer = window.setTimeout(() => {
      if (deepSearchKey.current === key) return;
      deepSearchKey.current = key;
      if (messagesRef.current.some(message => contains(message.text, keyword))) {
        pendingHighlight.current = keyword;
        const root = messageRef.current;
        const hit = root?.querySelector<HTMLElement>('[data-hit="true"]');
        hit?.scrollIntoView({ block: 'center' });
        return;
      }
      void openSession(active, keyword);
    }, 320);
    return () => window.clearTimeout(timer);
  }, [searchKeyword, active, openSession]);

  const toggleWorkSession = (username: string) => {
    setWorkSelected(current =>
      current.includes(username) ? current.filter(item => item !== username) : [...current, username],
    );
  };

  const selectWorkRange = (range: RangeKey) => {
    setWorkRange(range);
    if (range !== 'custom') {
      const now = new Date();
      setWorkTo(dateInputValue(now));
      if (range === 'today') setWorkFrom(dateInputValue(now));
      if (range === '7d') setWorkFrom(dateInputValue(new Date(now.getTime() - 7 * 86400000)));
      if (range === '30d') setWorkFrom(dateInputValue(new Date(now.getTime() - 30 * 86400000)));
    }
  };

  const runWorkSearch = async () => {
    if (!account || !workKeyword.trim()) {
      setWorkSearch(null);
      return;
    }
    setBusy('search');
    setError('');
    try {
      const report = await invoke<SearchReport>('wechat_search', {
        accountId: account.id,
        keyword: workKeyword.trim(),
        groupsOnly: false,
        maxSessions: 200,
      });
      setWorkSearch(report);
    } catch (e) {
      setError(String(e));
      setWorkSearch(null);
    } finally {
      setBusy(null);
    }
  };

  const selectWorkSearchHits = () => {
    if (!workSearch) return;
    setWorkSelected(current => Array.from(new Set([...current, ...workSearch.hits.map(hit => hit.username)])));
  };

  const loadWorkMessages = async (session: SessionItem, since?: number, until?: number): Promise<WorkGroup> => {
    if (!account) return { session, messages: [], truncated: false };
    let before: number | undefined;
    let beforeId: number | undefined;
    let beforeShard: string | undefined;
    let collected: MessageItem[] = [];
    let truncated = false;

    for (let pageNumber = 0; pageNumber < 50; pageNumber += 1) {
      const page = await invoke<MessageItem[]>('wechat_messages', {
        accountId: account.id,
        username: session.username,
        limit: WORK_PAGE,
        before,
        beforeId,
        beforeShard,
      });
      if (!page.length) break;
      collected = [...collected, ...page];
      const last = page[page.length - 1];
      const oldest = last?.time;
      const oldestSeconds = oldest ? (oldest > 1e12 ? oldest / 1000 : oldest) : undefined;
      if ((since !== undefined && oldestSeconds !== undefined && oldestSeconds <= since) || page.length < WORK_PAGE) break;
      if (collected.length >= WORK_MAX_MESSAGES_PER_SESSION) {
        collected = collected.slice(0, WORK_MAX_MESSAGES_PER_SESSION);
        truncated = true;
        break;
      }
      before = last.time;
      beforeId = last.local_id;
      beforeShard = last.source_shard;
    }

    const messages = collected.filter(message => {
      const seconds = message.time > 1e12 ? message.time / 1000 : message.time;
      return (since === undefined || seconds >= since) && (until === undefined || seconds <= until);
    });
    return { session, messages, truncated };
  };

  const loadWorkSelection = async () => {
    if (!account) return;
    const selectedSessions = sessions.filter(session => workSelected.includes(session.username));
    if (!selectedSessions.length) {
      setError(t("请至少选择一个对话。"));
      return;
    }
    if (workRange === 'custom' && (!workFrom || !workTo)) {
      setError(t("请选择自定义时间范围的开始日期和结束日期。"));
      return;
    }
    const bounds = selectionBounds(workRange, workFrom, workTo);
    if (workRange === 'custom' && bounds.since !== undefined && bounds.until !== undefined && bounds.since > bounds.until) {
      setError(t("自定义时间范围无效：开始日期不能晚于结束日期。"));
      return;
    }
    setBusy('work');
    setError('');
    try {
      const selectedBySession = new Map<string, MessageItem[]>();
      for (const item of selectedMessages) {
        if (!workSelected.includes(item.session.username)) continue;
        const list = selectedBySession.get(item.session.username) ?? [];
        list.push(item.message);
        selectedBySession.set(item.session.username, list);
      }
      const groups = selectedMessages.length
        ? selectedSessions.map(session => ({
            session,
            messages: (selectedBySession.get(session.username) ?? []).sort((a, b) => b.time - a.time),
            truncated: false,
          }))
        : await Promise.all(selectedSessions.map(session => loadWorkMessages(session, bounds.since, bounds.until)));
      setWorkGroups(groups);
      setWorkInsights(null);
      setAiSummary(null);
      setAiConfirm(false);
      setActionPicker(false);
      setWorkPhase('review');
      if (!groups.some(group => group.messages.length > 0)) setNotice(t("当前条件下没有找到消息，请调整时间范围或对话。"));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const selectionTitle = () => {
    if (workRange === 'today') return t("今天");
    if (workRange === '7d') return t("近 7 天");
    if (workRange === '30d') return t("近 30 天");
    if (workRange === 'custom') return tf("{0} 至 {1}", workFrom || '开始', workTo || '结束');
    return t("全部时间");
  };

  /** Exactly what the user reviewed, capped the same way the Rust builder caps it. */
  const aiPlan = useMemo(() => {
    const messages = workGroups
      .filter(group => group.messages.length > 0)
      .flatMap(group =>
        group.messages.slice(0, AI_MESSAGES_PER_SESSION).map(message => ({
          username: group.session.username,
          display: group.session.display,
          is_group: group.session.is_group,
          time: message.time,
          sender: message.is_self ? t("我") : message.sender_name || message.sender || t("未知"),
          text: message.text,
        })),
      );
    return {
      items: messages,
      count: messages.length,
      sessions: new Set(messages.map(message => message.username)).size,
      characters: messages.reduce((sum, message) => sum + message.text.length, 0),
      capped: workGroups.some(group => group.messages.length > AI_MESSAGES_PER_SESSION),
    };
  }, [workGroups]);

  const aiReady = canUseAi && Boolean(agentCurrent?.configured && agentCurrent.available);

  const saveHistory = async (record: NewSummaryRecord) => {
    try {
      await invoke<SummaryRecord[]>('summary_history_add', { record });
      setNotice(tf("{0}已保存到本机，可在左侧「总结记录」查看。", HISTORY_KIND_LABEL[record.kind]));
    } catch (e) {
      setError(tf("保存总结记录失败：{0}", String(e)));
    }
  };

  /** The record keeps what was produced, not the full transcript: enough to re-read the
   *  result without copying every reviewed message into a second file on disk. */
  const recordFromInsights = (insights: WorkInsights, note: string | null = null): NewSummaryRecord | null => {
    if (!account) return null;
    const groups = workGroups.filter(group => group.messages.length > 0);
    const bounds = selectionBounds(workRange, workFrom, workTo);
    return {
      kind: 'local',
      account_id: account.id,
      range_label: selectionTitle(),
      since: bounds.since ?? null,
      until: bounds.until ?? null,
      sessions: groups.map(group => ({
        username: group.session.username,
        display: group.session.display,
        is_group: group.session.is_group,
        messages: group.messages.length,
      })),
      messages: groups.reduce((sum, group) => sum + group.messages.length, 0),
      characters: groups.reduce(
        (sum, group) => sum + group.messages.reduce((inner, message) => inner + message.text.length, 0),
        0,
      ),
      truncated: workGroups.some(group => group.truncated),
      agent_name: '',
      model: '',
      elapsed_ms: 0,
      output: null,
      insights: {
        focus: insights.focus.slice(0, 80).map(item => ({
          category: item.category,
          session: item.session.display,
          day: dayLabel(toDate(item.message.time) ?? new Date()),
          text: item.message.text,
        })),
        timeline: insights.timeline.map(entry => ({
          day: entry.day,
          messages: entry.messages.length,
          sessions: Array.from(new Set(entry.messages.map(message => message.session.display))).slice(0, 3),
        })),
        related: insights.related.map(hit => ({
          username: hit.username,
          display: hit.display,
          hits: hit.hits,
          truncated: hit.truncated,
        })),
        terms: insights.terms,
      },
      note,
    };
  };

  const runAiSummary = async () => {
    if (!account) return;
    if (!agentCurrent?.configured || !agentCurrent.available) {
      setError(canConfigureAgent ? t("请在「设置 → Agent 集成」中配置可用的 Agent。") : t("当前无法使用 AI 总结。"));
      return;
    }
    if (!aiPlan.items.length) {
      setError(t("当前选集没有消息，无法生成总结。"));
      return;
    }
    setBusy('ai');
    setError('');
    setAiConfirm(false);
    try {
      const bounds = selectionBounds(workRange, workFrom, workTo);
      const result = await invoke<AgentSummary>('agent_summarize', {
        accountId: account.id,
        usernames: [...new Set(aiPlan.items.map(message => message.username))],
        messages: aiPlan.items,
        since: bounds.since,
        until: bounds.until,
      });
      setAiSummary(result);
      const sessionMap = new Map<string, HistorySession>();
      for (const item of aiPlan.items) {
        const existing = sessionMap.get(item.username);
        if (existing) existing.messages += 1;
        else sessionMap.set(item.username, {
          username: item.username,
          display: item.display,
          is_group: item.is_group,
          messages: 1,
        });
      }
      void saveHistory({
        kind: 'ai',
        account_id: account.id,
        range_label: selectionTitle(),
        since: bounds.since ?? null,
        until: bounds.until ?? null,
        sessions: [...sessionMap.values()],
        messages: result.messages,
        characters: result.characters,
        truncated: result.truncated,
        agent_name: result.agent_name,
        model: result.model,
        elapsed_ms: result.elapsed_ms,
        output: result.output,
        insights: null,
        note: null,
      });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const closeAiConfirm = () => {
    if (busy === 'ai') return;
    setAiConfirm(false);
  };

  // Escape closes the confirmation dialog, but never while the agent is already running.
  useEffect(() => {
    if (!aiConfirm) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && busy !== 'ai') setAiConfirm(false);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [aiConfirm, busy]);

  const generateWorkInsights = async () => {
    const items = workGroups.flatMap(group => group.messages.map(message => ({ session: group.session, message })));
    if (!items.length) {
      setError(t("当前选集没有消息，无法生成总结。"));
      return;
    }
    const timelineMap = new Map<string, SelectedMessage[]>();
    const focus: WorkInsightItem[] = [];
    for (const item of [...items].sort((a, b) => a.message.time - b.message.time)) {
      const date = toDate(item.message.time);
      const day = date ? dayKey(date) : t("未知日期");
      timelineMap.set(day, [...(timelineMap.get(day) ?? []), item]);
      const category = classifyWorkText(item.message.text);
      if (category) focus.push({ ...item, category });
    }
    const terms = workKeyword.trim() ? [workKeyword.trim()] : extractWorkTerms(items.map(item => item.message));
    const base: WorkInsights = {
      timeline: [...timelineMap.entries()].map(([day, messages]) => ({ day, messages })),
      focus: focus.slice(0, 80),
      related: [],
      terms,
    };
    setWorkInsights(base);
    const persist = (insights: WorkInsights, note: string | null = null) => {
      const record = recordFromInsights(insights, note);
      if (record) void saveHistory(record);
    };
    if (!account || !terms.length) {
      persist(base);
      return;
    }

    setBusy('explore');
    try {
      const reports = await Promise.all(terms.map(term => invoke<SearchReport>('wechat_search', {
        accountId: account.id,
        keyword: term,
        groupsOnly: false,
        maxSessions: 200,
      })));
      const selectedUsernames = new Set(workGroups.map(group => group.session.username));
      const related = new Map<string, SearchHit>();
      for (const report of reports) {
        for (const hit of report.hits) {
          if (!selectedUsernames.has(hit.username)) {
            const previous = related.get(hit.username);
            related.set(hit.username, previous ? { ...previous, hits: previous.hits + hit.hits } : hit);
          }
        }
      }
      const merged: WorkInsights = {
        ...base,
        related: [...related.values()].sort((a, b) => b.hits - a.hits).slice(0, 20),
      };
      setWorkInsights(merged);
      persist(merged);
    } catch (e) {
      setError(String(e));
      persist(base, t("相关会话探索失败，记录里只有项目重点与时间线。"));
    } finally {
      setBusy(null);
    }
  };

  const addRelatedSession = (username: string) => {
    if (!sessions.some(session => session.username === username)) return;
    setSelectedMessages([]);
    setSelectionAnchor(null);
    setWorkInsights(null);
    setAiSummary(null);
    setAiConfirm(false);
    setActionPicker(false);
    setWorkSelected(current => Array.from(new Set([...current, username])));
    setWorkPhase('select');
    setNotice(t("已加入相关会话；点击「展开并预览」读取它的聊天内容。"));
  };

  const renderAiSummary = () => {
    if (!aiSummary) return null;
    return (
      <section className="wxv-ai">
        <header className="wxv-ai-head">
          <div>
            <strong>{t("Agent 总结")}</strong>
            <small>
              {aiSummary.agent_name}
              {aiSummary.model ? ` · ${aiSummary.model}` : ''} · {(aiSummary.elapsed_ms / 1000).toFixed(1)} {t("秒")}
            </small>
          </div>
          <span>
            {aiSummary.sessions} {t("个会话 /")} {aiSummary.messages} {t("条消息")}
            {aiSummary.truncated ? t(" · 已按上限截断") : ''}
          </span>
        </header>
        <div className="wxv-ai-body">{renderAgentMarkdown(aiSummary.output)}</div>
        <footer className="wxv-ai-foot">
          <span>{t("由你配置的本地 Agent 生成，请对照原文核对。")}</span>
          <button type="button" className="wxv-link" onClick={() => void runAiSummary()} disabled={busy === 'ai'}>
            {busy === 'ai' ? t("生成中…") : t("重新生成")}
          </button>
        </footer>
      </section>
    );
  };

  const renderWorkInsights = () => {
    if (!workInsights) return null;
    return (
      <section className="wxv-insights">
        <header className="wxv-insights-head">
          <div><strong>{t("本地初步整理")}</strong><small>{t("基于已选原文的规则分析，不是 AI 结论")}</small></div>
          <span>{workInsights.terms.length ? tf("探索词：{0}", workInsights.terms.join('、')) : t("未提取到探索词")}</span>
        </header>
        <section className="wxv-insight-section">
          <h4>{t("项目重点")}</h4>
          {workInsights.focus.length ? (
            workInsights.focus.slice(0, 24).map((item, index) => (
              <article key={`${item.session.username}-${item.message.local_id}-${index}`} className="wxv-insight-item">
                <span className={`wxv-insight-tag ${item.category}`}>{item.category}</span>
                <div><strong>{item.session.display}</strong><small>{dayLabel(toDate(item.message.time) ?? new Date())}</small><p>{item.message.text}</p></div>
              </article>
            ))
          ) : <p className="wxv-empty-line">{t("暂未识别到待办、决定、风险、问题或数据线索。")}</p>}
        </section>
        <section className="wxv-insight-section">
          <h4>Timeline</h4>
          {workInsights.timeline.map(item => (
            <div key={item.day} className="wxv-timeline-day">
              <strong>{item.day}</strong>
              <span>{item.messages.length} {t("条消息 ·")} {item.messages.slice(0, 3).map(entry => entry.session.display).join('、')}</span>
            </div>
          ))}
        </section>
        <section className="wxv-insight-section">
          <h4>{t("相关聊天群 / 会话")}</h4>
          {workInsights.related.length ? workInsights.related.map(hit => (
            <div key={hit.username} className="wxv-related-session">
              <div><strong>{hit.display}</strong><small>{hit.hits}{hit.truncated ? '+' : ''} {t("条相关命中")}</small></div>
              <button type="button" className="wxv-link" onClick={() => addRelatedSession(hit.username)}>{t("加入选集")}</button>
            </div>
          )) : <p className="wxv-empty-line">{t("暂未发现其他相关会话；可以补充关键字后重新总结。")}</p>}
        </section>
      </section>
    );
  };

  // Header buttons live in the embedding page. They are requests, not state, so a click
  // while the pane is hidden still opens the drawer / downloads once the chat is ready.
  useEffect(() => {
    if (!canOrganize || !workRequest || workRequest === handledWorkRequest.current || !ready) return;
    handledWorkRequest.current = workRequest;
    openWorkPicker();
  }, [workRequest, ready, canOrganize]);

  useEffect(() => {
    onChatState?.({
      active: Boolean(active),
      messages: messages.length,
      selected: selectedMessages.length,
      agentReady: Boolean(agentCurrent?.configured && agentCurrent.available),
    });
  }, [active?.username, messages.length, selectedMessages.length, agentCurrent?.configured, agentCurrent?.available, onChatState]);

  const groups = sessions.filter(session => session.is_group);
  const needle = query.trim().toLowerCase();
  const filteredSessions = useMemo(() => {
    const scope = tab === 'group' ? sessions.filter(s => s.is_group) : sessions;
    if (!needle) return scope;
    return scope.filter(
      s => s.display.toLowerCase().includes(needle) || s.username.toLowerCase().includes(needle),
    );
  }, [sessions, tab, needle]);

  const ordered = useMemo(() => [...messages].reverse(), [messages]);
  const orderedWithDates = useMemo(() => {
    let lastDay = '';
    let lastTime = 0;
    return ordered.map(message => {
      const date = toDate(message.time);
      const key = date ? dayKey(date) : '';
      const divider = key && key !== lastDay ? dayLabel(date as Date) : '';
      // WeChat only stamps a conversation when the silence is long enough to matter;
      // repeating a timestamp under every bubble adds noise without information.
      const stamp = !divider && lastTime && message.time - lastTime >= 300 ? timeOnly(message.time) : '';
      lastDay = key;
      lastTime = message.time;
      return { message, divider, stamp };
    });
  }, [ordered]);

  // Only the conversation that actually matched should be dimmed/highlighted; otherwise
  // a previous search keeps a stale banner on an unrelated chat. The keyword typed in the
  // embedding page's header wins, because that is the search the user just performed.
  const searchHighlight = useMemo(() => {
    if (!search || !active) return '';
    return search.hits.some(hit => hit.username === active.username) ? search.keyword : '';
  }, [search, active]);
  const highlightKeyword = inlineKeyword || searchHighlight;
  const highlightHits = useMemo(
    () => (highlightKeyword ? messages.filter(message => contains(message.text, highlightKeyword)).length : 0),
    [messages, highlightKeyword],
  );

  /* ── session list ─────────────────────────────────────────────────────────── */

  const renderSessions = () => (
    <div className="wxv-scroll">
      {filteredSessions.map(session => (
        <button
          key={session.username}
          type="button"
          className={`wxv-session ${active?.username === session.username ? 'on' : ''}`}
          onClick={() => void openSession(session)}
        >
          <span className="wxv-avatar" style={{ background: avatarColor(session.username) }}>
            {avatars[session.username] ? (
              <img className="wxv-avatar-img" src={avatars[session.username]} alt="" draggable={false} />
            ) : (
              initial(session.display)
            )}
            {session.is_group && <span className="wxv-avatar-group" />}
          </span>
          <span className="wxv-session-body">
            <span className="wxv-session-top">
              <span className="wxv-session-name">{session.display}</span>
              <span className="wxv-session-time">{listTime(session.last_time)}</span>
            </span>
            <span className="wxv-session-bottom">
              <span className="wxv-session-preview">
                {session.is_group && session.member_count ? (
                  <em>{session.member_count} {t("人 ·")} </em>
                ) : null}
                {session.preview ?? t("（无预览）")}
              </span>
              {session.unread ? (
                <span className="wxv-unread">{session.unread > 99 ? '99+' : session.unread}</span>
              ) : null}
            </span>
          </span>
        </button>
      ))}
      {!filteredSessions.length && (
        <p className="wxv-empty-line">{tab === 'group' ? t("没有匹配的群聊。") : t("没有匹配的会话。")}</p>
      )}
    </div>
  );

  const renderSearchResults = () => {
    if (!search) return null;
    return (
      <div className="wxv-scroll">
        <div className="wxv-search-summary">
          <span>
            「{search.keyword}{t("」在")} <strong>{search.hits.length}</strong> {t("个会话中命中")}
          </span>
          <button type="button" className="wxv-link" onClick={clearSearch}>
            {t("返回会话")}
          </button>
          <small>
            {tab === 'group'
              ? tf("扫描最近 {0} 个群聊（未检索单聊）", search.scanned)
              : tf("扫描最近 {0} 个会话{1}", search.scanned, search.skipped ? `，另有 ${search.skipped} 个未扫描` : '')}
          </small>
        </div>
        {search.hits.map(hit => (
          <button
            key={hit.username}
            type="button"
            className={`wxv-hit ${active?.username === hit.username ? 'on' : ''}`}
            onClick={() => {
              const session = sessions.find(s => s.username === hit.username);
              if (session) void openSession(session, search.keyword);
            }}
          >
            <span className="wxv-hit-top">
              <span className="wxv-session-name">{hit.display}</span>
              <span className="wxv-hit-count">
                {hit.hits}
                {hit.truncated ? '+' : ''} {t("条")}
              </span>
            </span>
            {hit.snippets.map((snippet, idx) => (
              <span key={idx} className="wxv-hit-snippet">
                <em>{snippet.sender_name || snippet.sender || t("未知")}</em>
                <span className="wxv-hit-text">
                  <Highlight text={snippet.text} keyword={search.keyword} />
                </span>
              </span>
            ))}
          </button>
        ))}
        {!search.hits.length && <p className="wxv-empty-line">{t("没有命中。搜索只覆盖最近 200 个会话的文字内容。")}</p>}
      </div>
    );
  };

/** Null status means the first probe has not returned yet: stay neutral instead of alarming. */
const checkTone = (value: boolean | undefined) => (value === undefined ? 'pending' : value ? 'ok' : 'bad');
const checkMark = (value: boolean | undefined) =>
  value === undefined ? Icons.minus : value ? Icons.check : Icons.close;

  /* ── unlock panel ─────────────────────────────────────────────────────────── */

  const renderWhitelistRequired = () => (
    <div className="wxv-onboard">
      <div className="wxv-onboard-head">
        <p className="wxv-eyebrow">{t("先定义查看范围")}</p>
        <h2>{t("请先配置聊天白名单")}</h2>
        <p>{t("为避免把整套微信历史记录直接展示出来，聊天记录只开放白名单中的群聊和单聊。")}</p>
      </div>
      <div className="wxv-steps">
        <li className="done">
          <span className="wxv-step-no">1</span>
          <div>
            <strong>{t("打开“聊天白名单”")}</strong>
            <p className="wxv-muted">{t("在设置的「导入管理」中勾选需要查看的群聊和单聊。")}</p>
          </div>
        </li>
        <li>
          <span className="wxv-step-no">2</span>
          <div>
            <strong>{t("保存后返回这里")}</strong>
            <p className="wxv-muted">{t("保存后重新进入聊天记录，白名单会同时约束消息查看、搜索、探索和总结。")}</p>
            {onOpenWhitelist && <button type="button" className="wxv-btn primary" onClick={onOpenWhitelist}>{t("打开导入管理")}</button>}
          </div>
        </li>
      </div>
    </div>
  );

  const importedForOther = keyImport && account
    ? keyImport.accounts.find(entry => !entry.selected && entry.matched + entry.already > 0)
    : undefined;
  const importedForCurrent = keyImport && account
    ? keyImport.accounts.find(entry => entry.selected && entry.matched + entry.already > 0)
    : undefined;

  const renderUnlock = () => {
    const usableDb = (rel: string) => outcomes.some(outcome =>
      outcome.rel === rel && (outcome.status === 'ok' || outcome.status === 'cached'));
    const decryptedForWizard = usableDb('session/session.db') && outcomes.some(outcome =>
      /^message\/message_\d+\.db$/.test(outcome.rel) && (outcome.status === 'ok' || outcome.status === 'cached'));
    const wizardCanNext = !wizard || (wizardStep === 'wizard-permission' || wizardStep === 'wizard-account'
      ? Boolean(account)
      : wizardStep === 'wizard-key'
        ? Boolean(savedKeySource?.exists || captureOutcome?.matched || keyedCount > 0)
        : wizardStep === 'wizard-import'
          ? keyedCount > 0
          : wizardStep === 'wx-connect-decrypt'
            ? ready || decryptedForWizard
            : true);
    return (
    <div className="wxv-onboard">
      <div className="wxv-onboard-head">
        {ready && (
          <button
            type="button"
            className="wxv-link"
            onClick={() => (onOpenChat ? onOpenChat() : setForceUnlock(false))}
          >
            {t("← 返回聊天记录")}
          </button>
        )}
        {mode !== 'settings' && <p className="wxv-eyebrow">{t("只读 · 本地解密")}</p>}
        <h2>{t("连接微信")}</h2>
        <p>
          {t("程序读取微信 4.x 的数据库副本，用你的密钥逐库校验第 1 页 HMAC 后解密到应用数据目录。原始数据库不会被修改。")}
        </p>
      </div>

      <nav className="wxv-connection-progress" aria-label={t("微信连接步骤")}>
        {[
          { id: 'wx-connect-account', label: t("查找账号"), done: Boolean(account) },
          { id: 'wx-connect-key', label: t("获取密钥"), done: keyedCount > 0 },
          { id: 'wx-connect-decrypt', label: t("解密记录"), done: ready },
        ].map((step, index) => <button key={step.id} type="button" onClick={() => document.getElementById(step.id)?.scrollIntoView({ behavior: 'smooth', block: 'start' })}>
          <span className={step.done ? 'complete' : ''}>{step.done ? Icons.check : index + 1}</span>{t(step.label)}
        </button>)}
      </nav>

      <ol className="wxv-steps">
        <li id="wx-connect-account" className={account ? 'done' : ''}>
          <span className="wxv-step-no">{account ? Icons.check : '1'}</span>
          <div>
            <strong>{wizard && wizardStep === 'wizard-permission' ? t("系统授权") : wizard && wizardStep === 'wizard-account' ? t("账号扫描") : scan && scan.accounts.length > 1 ? t("选择账号") : t("当前账号")}</strong>
            {wizard && wizardStep === 'wizard-permission' ? (
              <>
                <p className="wxv-muted">{t("先确认 AIMyChats 可以读取微信本机数据；授权完成后进入下一步扫描账号。")}</p>
                <DiskAccessCard onRescan={() => void doScan()} />
                {account && <div className="wxv-wizard-state">{t("系统授权已完成，可以继续扫描微信账号。")}</div>}
              </>
            ) : scan?.accounts.length ? (
              scan.accounts.length > 1 ? (
                <div className="wxv-accounts">
                  {scan.accounts.map(item => (
                    <label key={item.id} className={`wxv-account ${item.id === accountId ? 'on' : ''}`}>
                      <input
                        type="radio"
                        name="wx-account"
                        checked={item.id === accountId}
                        disabled={accountSwitching}
                        onChange={() => void switchAccount(item.id)}
                      />
                      <span>
                        <strong>{item.id}</strong>
                        <small>
                          {item.db_count} {t("个库 ·")} {formatSize(item.total_bytes)} {t("· 加密")} {item.encrypted_count}
                        </small>
                      </span>
                    </label>
                  ))}
                </div>
              ) : (
                <p className="wxv-muted">
                  {accountId ? <>{t("已自动使用本机微信账号")} <span className="mono">{accountId}</span>。</> : t("尚未确认本机微信账号。")}
                  {account
                    ? tf(" {0} 个库 · {1} · 加密 {2}", account.db_count, formatSize(account.total_bytes), account.encrypted_count)
                    : ''}
                </p>
              )
            ) : (
              <>
                <p className="wxv-muted">
                  {busy === 'scan' ? t("正在定位微信目录…") : t("没有发现账号目录，请确认微信已登录过。")}
                </p>
                {busy !== 'scan' && <DiskAccessCard onRescan={() => void doScan()} />}
              </>
            )}
          </div>
        </li>

        <li id="wx-connect-key" className={keyedCount > 0 ? 'done' : ''}>
          <span className="wxv-step-no">{keyedCount > 0 ? Icons.check : '2'}</span>
          <div>
            <strong>{wizard && wizardStep === 'wizard-import' ? t("导入校验") : t("获取密钥")}</strong>
            <p className="wxv-muted">
              {t("支持账号口令（64 位十六进制，逐库 PBKDF2 派生）、内存密钥")} <code>x'&lt;hex&gt;'</code>{t("、或口令 JSON")}
              <code>{'{"passphrase":"…"}'}</code>{t("。导入时只保留通过第 1 页 HMAC 校验的内容。")}
            </p>

            <div className={`wxv-keyguide ${noKeysYet || freshSource ? 'attention' : ''} ${platform?.platform === 'windows' ? 'windows' : ''}`}>
              <div className="wxv-keyguide-head">
                <div>
                  <strong>{platform?.platform === 'windows' ? t("使用 Windows 方式获取密钥") : t("没有密钥？在本应用里直接获取")}</strong>
                  <p className="wxv-muted">
                    {platform?.platform === 'windows'
                      ? t("Windows 通过读取已登录 Weixin.exe 的可读内存寻找候选密钥，并用本地数据库第 1 页 HMAC 逐库校验；找不到时可导入可信工具导出的密钥 JSON。")
                      : savedKeySource?.exists
                        ? tf("已检测到口令文件（{0}），可以导入并逐库校验。", formatAge(savedKeySource.age_seconds))
                        : captureStatus?.pending
                          ? t("正在进行密钥捕获：按下面的步骤完成即可，中途不要手动改动微信。")
                          : t("按下面四步在本机完成捕获：环境检查 → 安全准备并启动捕获 → 登录时读取口令 → 自动恢复并逐库校验。全程不需要修改系统完整性保护，也不需要另装工具。")}
                  </p>
                </div>
                <div className="wxv-keyguide-head-actions">
                  {freshSource && <span className="wxv-keyguide-fresh">{t("新口令")}</span>}
                  <button type="button" className="wxv-btn" onClick={() => setShowKeyGuide(value => !value)}>
                    {showKeyGuide ? t("收起步骤") : t("查看获取步骤")}
                  </button>
                </div>
              </div>

{platform?.platform === 'windows' && (
                <div className="wxv-windows-keyguide">
                  <strong>{t("Windows 密钥扫描")}</strong>
                  <p className="wxv-muted">{t("请先启动微信并登录目标账号。扫描只读取本机 Weixin.exe 的内存，不上传密钥；候选结果必须通过数据库 HMAC 校验才会保存。")}</p>
                  <div className="wxv-row"><button type="button" className="wxv-btn primary" onClick={() => void scanWindowsKeys()} disabled={windowsScanBusy || !account}>{windowsScanBusy ? t("扫描并校验中…") : t("扫描微信内存并校验")}</button><span className="wxv-muted">{t("支持旧版 x'&lt;key&gt;&lt;salt&gt;' 格式；新版本可导入可信 JSON。")}</span></div>
                  {platform.key_acquisition && <p className="wxv-keyguide-path">{t("当前策略：")}<code>{platform.key_acquisition}</code></p>}
                </div>
              )}

              {showKeyGuide && (
                <>
                  {/* Environment: the reference flow refuses to touch WeChat unless every
                      one of these checks passes, so they are shown before the first action. */}
                  <div id="wizard-detail-key-env" className="wxv-keyguide-step">
                    <strong>{t("1 · 环境检查")}</strong>
                    <ul className="wxv-keyguide-checks">
                      <li>
                        <span className={`wxv-keyguide-check ${checkTone(captureStatus?.platform_supported)}`}>
                          {checkMark(captureStatus?.platform_supported)}
                        </span>
                        {t("Apple Silicon Mac（应用内捕获目前只验证了这一架构）")}
                      </li>
                      <li>
                        <span className={`wxv-keyguide-check ${checkTone(captureStatus?.lldb_available)}`}>
                          {checkMark(captureStatus?.lldb_available)}
                        </span>
                        LLDB{captureStatus?.lldb_available ? t(" 已就绪") : t(" 或 Python 运行环境无法启动：请修复 Xcode Command Line Tools")}
                      </li>
                      <li>
                        {/* A running transaction re-signs WeChat on purpose: showing that as a
                            failure would tell the user to undo the step they just took. */}
                        <span
                          className={`wxv-keyguide-check ${checkTone(
                            captureStatus?.wechat_official ||
                              (Boolean(captureStatus?.pending) && Boolean(captureStatus?.wechat_ad_hoc))
                          )}`}
                        >
                          {checkMark(
                            captureStatus?.wechat_official ||
                              (Boolean(captureStatus?.pending) && Boolean(captureStatus?.wechat_ad_hoc))
                          )}
                        </span>
                        {t("微信")} {captureStatus?.wechat_version ?? t("未找到")}
                        {captureStatus?.wechat_build ? `（${captureStatus.wechat_build}）` : ''}
                        {captureStatus?.wechat_official
                          ? t(" · 腾讯原签名")
                          : captureStatus?.wechat_ad_hoc
                            ? captureStatus?.pending
                              ? t(" · 本次捕获的临时签名（结束后自动换回原版）")
                              : t(" · 当前是临时签名，请先恢复原版")
                            : t(" · 不是可验证的腾讯签名")}
                      </li>
                      <li>
                        <span className={`wxv-keyguide-check ${checkTone(captureStatus?.user_can_write)}`}>
                          {checkMark(captureStatus?.user_can_write)}
                        </span>
                        {t("当前用户可写入微信与「应用程序」（保证异常退出后也能恢复原版）")}
                      </li>
                    </ul>
                    <p className="wxv-keyguide-path">
                      {t("备份目录：")}<span className="mono">{captureStatus?.backup_root ?? '~'}</span>
                    </p>
                    {captureStatus?.message && <p className="wxv-keyguide-note">{captureStatus.message}</p>}
                  </div>

                  {!captureStatus?.pending && (
                    <div id="wizard-detail-key-capture" className="wxv-keyguide-step">
                      <strong>{t("2 · 备份并临时重签微信")}</strong>
                      <p className="wxv-muted">
                        {t("程序会先用")} <code>ditto</code> {t("建立腾讯原版压缩备份和同卷写时复制保护，再把「副本」临时\n                        ad-hoc 签名，最后原子替换安装位置。部分系统使用无沙箱的深度重签，微信可能使用不同的数据目录。")}
                      </p>
                      <div className="wxv-keyguide-confirm">
                        <strong>{t("开始前请确认")}</strong>
                        <span>
                          {t("微信会被退出并需要重新扫码登录；请先处理未保存的内容。若微信或其安装目录不可写，程序会直接停止。")}
                        </span>
                        <p>
                          {t("捕获期间微信会以临时签名运行。结束、取消或失败时程序会自动换回腾讯原版并校验\n                          TeamIdentifier / 版本 / CDHash；无法证明安全时不会覆盖，而是保留恢复资产并报错。")}
                        </p>
                        {captureStatus?.wechat_quarantined && (
                          <p className="wxv-keyguide-note">
                            {t("这台 Mac 的微信仍带")} <code>com.apple.quarantine</code> {t("隔离属性：临时副本是 ad-hoc\n                            签名，腾讯的公证票据随之失效，首次启动会被 Gatekeeper 拦下。macOS 15\n                            起没有右键「打开」入口，需要在「系统设置 → 隐私与安全性 → 安全性」里点一次\n                            「仍要打开」（被拦下后 1 小时内有效）；15.1 还可能连提示都不弹。")}
                          </p>
                        )}
                        <label className="wxv-keyguide-accept">
                          <input
                            type="checkbox"
                            checked={captureRiskAccepted}
                            onChange={event => setCaptureRiskAccepted(event.target.checked)}
                          />
                          {t("我了解上述风险，并确认这台 Mac 与微信数据归我本人所有")}
                        </label>
                        <div className="wxv-row">
                          <button
                            type="button"
                            className="wxv-btn primary"
                            onClick={() => void prepareCapture()}
                            disabled={
                              !captureRiskAccepted ||
                              captureBusy !== null ||
                              !captureStatus?.platform_supported ||
                              !captureStatus?.lldb_available ||
                              !captureStatus?.wechat_official ||
                              !captureStatus?.user_can_write
                            }
                          >
                            {captureBusy === 'prepare' ? t("正在备份并重签…") : t("备份并临时重签")}
                          </button>
                        </div>
                      </div>
                    </div>
                  )}

                  {/* Step 3 only becomes actionable after step 2, but its existence has to be
                      visible beforehand — otherwise the checklist reads 1, 2, 4 and looks broken. */}
                  {!captureStatus?.pending && (
                    <div id="wizard-detail-key-login" className="wxv-keyguide-step">
                      <strong>{t("3 · 打开微信、登录并完成捕获")}</strong>
                      <p className="wxv-muted">
                        {t("完成第 2 步后这里会出现具体操作：打开微信 → 检查启动 → 检查捕获断点（一次管理员授权）→\n                        在微信里退出账号 → 启动监测（再一次授权）→ 重新登录同一账号，程序随后自动恢复并导入。")}
                      </p>
                    </div>
                  )}

                  {captureStatus?.pending && (
                    <div className="wxv-keyguide-step">
                      <strong>{t("3 · 打开微信、登录并完成捕获")}</strong>
                      {captureStatus.entitlement_mode === 'deep' && (
                        <p className="wxv-keyguide-note">{t("临时微信及其内部组件已重签，并以无沙箱模式运行，可能使用不同的数据目录。结束或取消时会恢复官方微信。")}</p>
                      )}
                      {captureStatus.entitlement_mode === 'sanitized' && (
                        <p className="wxv-keyguide-note">
                          {t("本次临时副本已剔除腾讯团队的受限权限（application-identifier / team-identifier /\n                          application-groups / temporary-exception）。这台 Mac 的签名校验不允许 ad-hoc\n                          签名携带它们，保留就会在启动前被系统拒绝（launchd 153）。结束或取消时仍会换回腾讯原版。")}
                        </p>
                      )}
                      <ol className="wxv-keyguide-steps">
                        <li>
                          <strong>{t("打开微信并完整登录")}</strong>{t("：点下面的「打开微信」；如果 macOS\n                          提示无法验证或双击没有反应，按下方说明放行一次。登录后进入要读取的聊天，让本地数据库生成。")}
                        </li>
                        <li>
                          <strong>{t("检查启动")}</strong>{t("：程序确认运行的是本次临时副本后，再检查捕获断点（这一步会弹管理员\n                          授权，密码只交给 macOS）。")}
                        </li>
                        <li>
                          <strong>{t("在微信里退出账号")}</strong>{t("：断点可用后再退出；不要提前退出，否则这次捕获作废。")}
                        </li>
                        <li>
                          <strong>{t("启动监测并授权")}</strong>{t("：再次输入管理员密码。看到「监测已就绪」之前不要重新登录。")}
                        </li>
                        <li>
                          <strong>{t("重新登录同一账号")}</strong>{t("：捕获成功后程序自动逐库校验、恢复腾讯原版微信并导入密钥。")}
                        </li>
                      </ol>

                      {launchDiagnosis && (
                        <div className="wxv-launch-diag">
                          <strong>
                            {launchDiagnosis.amfi_rejected
                              ? t("系统在启动前拒绝了这次临时签名（AMFI）")
                              : t("系统没有启动临时微信")}
                          </strong>
                          <p className="wxv-muted">{t(launchDiagnosis.hint)}</p>
                          <ul className="wxv-launch-diag-facts">
                            <li>
                              {t("隔离属性")} <code>com.apple.quarantine</code>：
                              {launchDiagnosis.quarantined ? t("存在（Gatekeeper 会评估这个副本）") : t("不存在")}
                            </li>
                            <li>
                              {t("签名：")}
                              {launchDiagnosis.resigned
                                ? t("本次临时 ad-hoc 副本")
                                : t("不是本次调试副本，请先检查是否在「本地数据」重新开始")}
                            </li>
                            <li>
                              {t("Gatekeeper 评估：")}
                              {launchDiagnosis.gatekeeper_accepted === null
                                ? t("无法评估")
                                : launchDiagnosis.gatekeeper_accepted
                                  ? t("已通过")
                                  : t("被拒绝")}
                              {launchDiagnosis.gatekeeper_reason
                                ? `（${launchDiagnosis.gatekeeper_reason}）`
                                : ''}
                            </li>
                            {launchDiagnosis.amfi_rejected && (
                              <li>{t("受限权限检查：被拒绝（launchd 153，进程在创建阶段就被杀死）")}</li>
                            )}
                            <li>{t("当前微信进程数：")}{launchDiagnosis.processes}</li>
                          </ul>
                          {launchDiagnosis.open_error && (
                            <p className="wxv-launch-diag-error">{t("open 返回：")}{launchDiagnosis.open_error}</p>
                          )}
                          {!launchDiagnosis.amfi_rejected && (
                            <ol className="wxv-keyguide-steps">
                              <li>
                                {t("在 Finder 里双击")} <code>/Applications/WeChat.app</code>
                                {t("。如果弹出「无法打开 / 无法验证」，点「完成」，不要点「移到废纸篓」。")}
                              </li>
                              <li>
                                {t("1 小时内打开")} <strong>{t("系统设置 → 隐私与安全性 → 安全性")}</strong>
                                {t("，看到「“WeChat” 已被阻止…」就点「仍要打开」→ 输入管理员密码 → 再点「打开」。\n                                macOS 15 起没有右键「打开」入口；15.1 可能完全不弹窗，只能走这里。")}
                              </li>
                              <li>
                                {t("放行后微信会正常启动；扫码登录完成后回到这里点「我已登录，检查启动」。")}
                              </li>
                              <li>
                                {t("如果系统设置里根本没有「仍要打开」，说明不是 Gatekeeper：先用「控制台」查看微信是否启动即退出，\n                                或直接点「取消并恢复微信」结束本次捕获，稍后再排查崩溃原因。")}
                              </li>
                            </ol>
                          )}
                          <div className="wxv-keyguide-actions">
                            {!launchDiagnosis.amfi_rejected && (
                              <>
                                <button
                                  type="button"
                                  className="wxv-btn primary"
                                  onClick={() => void openPrivacySecuritySettings()}
                                >
                                  {t("打开隐私与安全性设置")}
                                </button>
                                <button
                                  type="button"
                                  className="wxv-btn"
                                  onClick={() => void openCaptureWeChat()}
                                  disabled={captureBusy !== null}
                                >
                                  {captureBusy === 'open' ? t("正在打开…") : t("再打开一次微信")}
                                </button>
                                <button
                                  type="button"
                                  className="wxv-btn"
                                  onClick={() => void confirmCaptureLaunch()}
                                  disabled={captureBusy !== null}
                                >
                                  {captureBusy === 'confirm' ? t("正在检查…") : t("重新检查启动")}
                                </button>
                              </>
                            )}
                            <button
                              type="button"
                              className={`wxv-btn ${launchDiagnosis.amfi_rejected ? 'primary' : ''}`}
                              onClick={() => void restoreWeChat()}
                              disabled={captureBusy !== null}
                            >
                              {captureBusy === 'restore' ? t("正在恢复…") : t("取消并恢复微信")}
                            </button>
                          </div>
                        </div>
                      )}

                      <div className="wxv-keyguide-actions">
                        {(captureStatus.stage === 'awaiting_manual_launch' ||
                          captureStatus.stage === 'resigned' ||
                          captureStatus.stage === 'debug_prepared' ||
                          captureStatus.stage === 'backup_verified') && (
                          <>
                            <button
                              type="button"
                              className="wxv-btn primary"
                              onClick={() => void openCaptureWeChat()}
                              disabled={captureBusy !== null}
                            >
                              {captureBusy === 'open' ? t("正在打开…") : t("打开微信")}
                            </button>
                            <button
                              type="button"
                              className="wxv-btn"
                              onClick={() => void confirmCaptureLaunch()}
                              disabled={captureBusy !== null}
                            >
                              {captureBusy === 'confirm' ? t("正在检查…") : t("我已登录，检查启动")}
                            </button>
                          </>
                        )}
                        {captureStatus.stage === 'launched' && (
                          <button
                            type="button"
                            className="wxv-btn primary"
                            onClick={() => void runCapturePreflight()}
                            disabled={captureBusy !== null}
                          >
                            {captureBusy === 'preflight' ? t("正在检查断点…") : t("检查捕获断点（需管理员授权）")}
                          </button>
                        )}
                        {captureStatus.stage === 'preflight_passed' && (
                          <button
                            type="button"
                            className="wxv-btn primary"
                            onClick={() => void startCapture()}
                            disabled={captureBusy !== null}
                          >
                            {captureBusy === 'start' ? t("监测中，等待登录…") : t("我已退出账号，启动监测（需管理员授权）")}
                          </button>
                        )}
                        {captureStatus.stage === 'capturing' && (
                          <>
                            {captureStatus.resume_available ? (
                              // The monitor finished after an app restart: the result is on disk
                              // and only needs validating + restoring, so no second login.
                              <button
                                type="button"
                                className="wxv-btn primary"
                                onClick={() => void startCapture()}
                                disabled={captureBusy !== null}
                              >
                                {captureBusy === 'start'
                                  ? t("正在校验并恢复…")
                                  : t("校验并恢复已捕获的密钥")}
                              </button>
                            ) : (
                              <span className="wxv-keyguide-progress">
                                {captureStatus.capture_phase === 'waiting_authorization'
                                  ? t("等待管理员授权…")
                                  : captureStatus.capture_phase === 'monitoring'
                                    ? t("监测已就绪：现在重新登录同一账号")
                                    : captureStatus.capture_phase === 'captured'
                                      ? t("已捕获密钥，正在结束调试会话，随后校验并恢复微信")
                                      : captureStatus.capture_phase === 'validating'
                                        ? t("正在逐库校验…")
                                        : t("正在恢复腾讯原版微信…")}
                              </span>
                            )}
                          </>
                        )}
                        <button
                          type="button"
                          className="wxv-btn danger"
                          onClick={() => void restoreWeChat()}
                          disabled={captureBusy !== null}
                          title={captureBusy === 'start' ? t("监测进行中：请等它超时或完成后再恢复") : undefined}
                        >
                          {captureBusy === 'restore' ? t("正在恢复…") : t("取消并恢复官方微信")}
                        </button>
                      </div>

                      {captureStatus.prepared_process_exited && captureStatus.stage !== 'capturing' && (
                        <p className="wxv-keyguide-warn">
                          {t("临时微信进程已退出。请点「取消并恢复官方微信」换回腾讯原版后重新开始，不要自行改签名。")}
                        </p>
                      )}
                      {captureStatus.recovery_error_code && (
                        <p className="wxv-keyguide-warn">
                          {t("恢复被安全策略拦下（")}{captureStatus.recovery_error_code}{t("）。程序保留了应用、备份和恢复记录；\n                          请先点「取消并恢复官方微信」，按提示处理后再重新开始。")}
                        </p>
                      )}
                    </div>
                  )}

                  <div id="wizard-detail-key-import" className="wxv-keyguide-step">
                    <strong>{wizard && wizardStep === 'wizard-import' ? t("导入校验") : t("4 · 导入并校验")}</strong>
                    <p className="wxv-muted">
                      {captureOutcome?.matched
                        ? t("本次捕获的口令已通过消息库与会话库校验，并已逐库导入保存。")
                        : savedKeySource?.exists
                          ? t("已检测到口令文件：点「导入已提取的口令」逐库用第 1 页 HMAC 校验。口令不会显示在界面、不会写日志、不会上传。")
                          : t("捕获成功后口令会写入本机文件并自动导入；也可以手动粘贴或选择文件导入。")}
                    </p>
                    <div className="wxv-keyguide-actions">
                      <button
                        type="button"
                        className="wxv-btn"
                        onClick={() => void importSavedKey()}
                        disabled={!savedKeySource?.exists || busy === 'import'}
                      >
                        {busy === 'import' ? t("校验中…") : t("导入已提取的口令")}
                      </button>
                      <button
                        type="button"
                        className="wxv-btn"
                        onClick={() => void refreshSavedKeySource()}
                      >
                        {t("重新检测文件")}
                      </button>
                      {savedKeySource?.exists && !savedKeySource.secure && (
                        <button
                          type="button"
                          className="wxv-btn"
                          onClick={() => void fixKeyPermissions()}
                          disabled={fixingPermissions}
                        >
                          {fixingPermissions ? t("正在修改…") : t("修复口令文件权限")}
                        </button>
                      )}
                      <button
                        type="button"
                        className="wxv-btn"
                        onClick={() => {
                          setShowKeyGuide(false);
                          void doScan(accountId);
                        }}
                        disabled={busy === 'scan'}
                      >
                        {busy === 'scan' ? t("扫描中…") : t("重新扫描账号")}
                      </button>
                      {keyedCount > 0 && (
                        <button
                          type="button"
                          className="wxv-btn danger"
                          onClick={() => setClearConfirm(true)}
                          disabled={busy === 'import' || clearConfirm}
                        >
                          {t("清除本账号密钥")}
                        </button>
                      )}
                      {!savedKeySource?.exists && (
                        <span className="wxv-keyguide-missing">{t("尚未找到口令文件，导入按钮保持禁用。")}</span>
                      )}
                    </div>

                    {clearConfirm && (
                      <div className="wxv-keyguide-danger">
                        <p>
                          {t("确定清除账号")} <span className="mono">{accountId}</span> {t("已保存的")} {keyedCount}{' '}
                          {t("个密钥？这会同时删除该账号已解密的本地副本，之后必须重新捕获或导入口令才能查看。\n                          微信原始数据库不受影响。")}
                        </p>
                        <div className="wxv-row">
                          <button
                            type="button"
                            className="wxv-btn danger"
                            onClick={() => void clearKeys()}
                            disabled={busy === 'import'}
                          >
                            {busy === 'import' ? t("正在清除…") : t("确认清除密钥与解密副本")}
                          </button>
                          <button
                            type="button"
                            className="wxv-btn"
                            onClick={() => setClearConfirm(false)}
                            disabled={busy === 'import'}
                          >
                            {t("取消")}
                          </button>
                        </div>
                      </div>
                    )}

                    {freshSource && (
                      <p className="wxv-keyguide-fresh-note">
                        {t("检测到新写入的口令，导入会替换当前账号里已失效的密钥。")}
                      </p>
                    )}

                    {importedForOther && !importedForCurrent && (
                      <p className="wxv-keyguide-partial">
                        {t("该口令解锁的是账号")} <strong>{importedForOther.account}</strong>（{importedForOther.matched} {t("个新密钥），\n                        当前选择的是")} {account?.id}。
                        <button
                          type="button"
                          className="wxv-link"
                          disabled={accountSwitching}
                          onClick={() => void switchAccount(importedForOther.account)}
                        >
                          {t("切换到")} {importedForOther.account}
                        </button>
                      </p>
                    )}

                    <p className="wxv-keyguide-path">
                      {t("口令文件：")}<span className="mono">{savedKeySource?.path ?? '~/.wcdb-key-tool/wechat-passphrase.json'}</span>
                      {savedKeySource?.exists
                        ? tf("（{0}，权限 {1}）", formatAge(savedKeySource.age_seconds), savedKeySource.mode ?? '未知')
                        : t("（尚未生成）")}
                    </p>
                    {savedKeySource?.exists && !savedKeySource.secure && (
                      <p className="wxv-keyguide-warn">
                        {t("口令文件权限是")} {savedKeySource.mode ?? t("未知")}{t("，其他本机用户可能读取它。点「修复口令文件权限」改成 600。")}
                      </p>
                    )}
                    <p className="wxv-keyguide-path">
                      {t("需要新增微信号时：在微信登录新账号 → 点「重新扫描账号」→ 从第 1 步重新捕获一次。\n                      每个账号单独捕获、单独校验，口令不通用。")}
                    </p>
                  </div>
                </>
              )}
            </div>

            {/* Capture already writes the passphrase file and imports it automatically, so this
                box is only for keys obtained elsewhere — keep it available, not mandatory. */}
            <details className="wxv-keyguide-manual">
              <summary>{t("手动导入（可选）：粘贴或选择密钥文件")}</summary>
              <textarea
                className="wxv-textarea"
                rows={3}
                value={keyText}
                onChange={e => setKeyText(e.target.value)}
                placeholder="粘贴 64 位十六进制口令，或 x'<key><salt>'，或口令 JSON"
                spellCheck={false}
              />
              <div className="wxv-row">
                <label className="wxv-file">
                  {t("选择文件")}
                  <input
                    type="file"
                    accept=".json,application/json,text/plain"
                    onChange={e => void importKeys(e.target.files?.[0])}
                  />
                </label>
                <button
                  type="button"
                  className="wxv-btn"
                  onClick={() => void importKeys()}
                  disabled={busy === 'import'}
                >
                  {busy === 'import' ? t("校验中…") : t("导入并校验")}
                </button>
              </div>
            </details>
            <div className="wxv-row">
              <span className="wxv-key-state">
                {encryptedDbs.length ? tf("已就绪 {0}/{1} 个加密库", keyedCount, encryptedDbs.length) : t("等待发现账号")}
              </span>
            </div>
            {keyedCount > 0 && keyVerifiedSecs && (
              <p className="wxv-keyguide-path">
                {t("密钥最近校验：")}{formatKeyDate(keyVerifiedSecs)}（{keyAgeDays} {t("天前）。微信更新或重新登录后口令会轮换，需要重新提取并导入。")}
              </p>
            )}
            {accountMissingKeys && keyedCount > 0 && (
              <p className="wxv-keyguide-partial">
                {t("还有")} {encryptedDbs.length - keyedCount} {t("个加密库没有匹配到密钥，通常是微信从未打开过的库，不影响已就绪的聊天记录。")}
              </p>
            )}
          </div>
        </li>

        <li id="wx-connect-decrypt" className={ready ? 'done' : ''}>
          <span className="wxv-step-no">{ready ? Icons.check : '3'}</span>
          <div>
            <strong>{wizard && wizardStep === 'wx-connect-decrypt' ? t("解密记录") : t("解密并打开")}</strong>
            <p className="wxv-muted">
              {t("解密会逐页校验，任一页失败即整体失败，不会产出半成品。已解密过的库直接复用缓存。")}
            </p>
            <div className="wxv-row">
              <button
                type="button"
                className="wxv-btn primary"
                onClick={() => void decrypt()}
                disabled={!account || busy === 'decrypt' || keyedCount === 0}
              >
                {busy === 'decrypt' ? t("解密中…") : t("解密当前账号")}
              </button>
              <label className="wxv-check">
                <input type="checkbox" checked={onlyChat} onChange={e => setOnlyChat(e.target.checked)} />
                {t("只处理聊天相关库")}
              </label>
              <label className="wxv-check">
                <input type="checkbox" checked={applyWal} onChange={e => setApplyWal(e.target.checked)} />
                {t("尝试回填 WAL")}
              </label>
              <button type="button" className="wxv-btn" onClick={() => void clearCache()}>
                {t("清理解密缓存")}
              </button>
            </div>
            {wizard && notice && <div className="wxv-wizard-state">{notice}</div>}
            <p className="wxv-muted">
              {t("解密副本保存在应用数据目录；清理后会回到未解密状态，聊天记录页在重新解密前不显示内容。")}
            </p>
            {!!outcomes.length && (
              <>
                <button type="button" className="wxv-link" onClick={() => setShowLog(v => !v)}>
                  {showLog ? t("收起") : t("查看")}{t("解密详情（")}{outcomes.length} {t("个库）")}
                </button>
                {showLog && (
                  <table className="wxv-table">
                    <thead>
                      <tr>
                        <th>{t("数据库")}</th>
                        <th>{t("状态")}</th>
                        <th>{t("页数")}</th>
                        <th>{t("说明")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {outcomes.map(outcome => (
                        <tr key={`${outcome.account}-${outcome.rel}`}>
                          <td className="mono">{outcome.rel}</td>
                          <td className={`wxv-status ${outcome.status}`}>{outcome.status}</td>
                          <td>{outcome.pages || '—'}</td>
                          <td className="wxv-note">
                            {outcome.error ?? outcome.wal?.status ?? (outcome.status === 'cached' ? t("命中缓存") : '')}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </>
            )}
          </div>
        </li>
      </ol>

      {wizard && wizardStep !== 'wizard-method' && (
        <div className="wizard-detail-actions">
          <button type="button" className="wxv-btn" onClick={onWizardBack}>{t("上一步")}</button>
          <button type="button" className="wxv-btn primary" onClick={onWizardNext} disabled={!wizardCanNext}>
            {wizardStep === 'wizard-permission' ? t("下一步：账号扫描") : wizardStep === 'wizard-account' ? t("下一步：获取密钥") : wizardStep === 'wizard-key' ? t("下一步：导入校验") : wizardStep === 'wizard-import' ? t("下一步：解密记录") : t("下一步：完成")}
          </button>
        </div>
      )}

      <details className="wxv-warn">
        <summary>{t("密钥获取与安全说明")}</summary>
        <p>
          {t("官方微信启用了 Hardened Runtime 和沙盒，直接附加调试器可能被系统拒绝。\n          应用内获取流程会先备份官方微信，再经你确认后使用临时签名捕获，完成或取消时恢复并校验原版。")}
        </p>
        <p>
          <strong>{t("不要自行重签微信。")}</strong>{t("签名或权限处理不当可能导致微信无法启动。\n          如不希望使用应用内捕获，也可以手动导入可信来源的密钥文件。")}
        </p>
        {env && (
          <p className="wxv-muted mono">
            {t("密钥文件：")}{env.keys_path}
            <br />
            {t("解密缓存：")}{env.cache_root}
          </p>
        )}
      </details>
    </div>
  );
  };

  /* ── chat pane ───────────────────────────────────────────────────────────── */

  const renderMessages = () => (
    <div className="wxv-messages" ref={messageRef} onScroll={event => {
      const root = event.currentTarget;
      if (root.clientHeight > 0) followNewest.current = root.scrollHeight - root.scrollTop - root.clientHeight <= 24;
    }}>
      <div ref={messageContentRef}>
      {busy === 'messages' && !messages.length && <LoadingSkeleton label={t("正在加载聊天记录")} kind="messages" />}
      {hasMore && (
        <button type="button" className="wxv-more" onClick={() => void loadOlder()} disabled={busy === 'messages'}>
          {busy === 'messages' ? t("加载中…") : t("加载更早的消息")}
        </button>
      )}
      {orderedWithDates.map(({ message, divider, stamp }) => {
        const base = kindBase(message.kind);
        const isWhatsApp = message.source_shard === 'whatsapp';
        const system = !isWhatsApp && (base === 10000 || base === 10002);
        const placeholder = isPlaceholder(message.text);
        const media = !isWhatsApp && !placeholder && base !== 1;
        const imageMessage = !isWhatsApp && base === 3;
        const voiceMessage = !isWhatsApp && base === 34;
        const hit = !!highlightKeyword && contains(message.text, highlightKeyword);
        const selected = selectedMessages.some(item => messageKey(item.session.username, item.message) === messageKey(active?.username ?? '', message));
        const selectMessage = (extend = false) => toggleMessageSelection(message, extend);
        if (system) {
          return (
            <div
              key={`${message.local_id}-${message.time}`}
              className={`wxv-system ${selected ? 'selected' : ''}`}
              onClick={event => {
                // A hit on the image opens the full-size copy even when the nested button
                // does not receive the click itself (WebKit hit-testing on the bubble).
                // Image messages are mostly picture: a plain click opens the readable copy,
                // a shift-click still toggles the selection. Without this, clicking a
                // 250 px thumbnail only highlighted the bubble and never showed the content.
                const image = mediaFor(message).find(entry => entry.kind === 'image');
                if (image && !event.shiftKey && (event.target as HTMLElement).closest('.wxv-bubble')) {
                  event.stopPropagation();
                  openImage(image);
                  return;
                }
                selectMessage(event.shiftKey);
              }}
              role="button"
              tabIndex={0}
              aria-pressed={selected}
              onKeyDown={event => {
                if (event.key === 'Enter' || event.key === ' ') selectMessage(event.shiftKey);
              }}
            >
              {divider && <div className="wxv-divider">{divider}</div>}
              {stamp && <div className="wxv-divider">{stamp}</div>}
              <span>{message.text}</span>
            </div>
          );
        }
        return (
          <div key={`${message.local_id}-${message.time}`}>
            {divider && <div className="wxv-divider">{divider}</div>}
            {stamp && <div className="wxv-divider wxv-stamp">{stamp}</div>}
            <div
              className={`wxv-msg ${message.is_self ? 'self' : ''} ${placeholder ? 'sticker' : ''} ${selected ? 'selected' : ''}`}
              data-hit={hit ? 'true' : 'false'}
              id={`msg-${message.local_id}`}
              onClick={event => {
                // A hit on the image opens the full-size copy even when the nested button
                // does not receive the click itself (WebKit hit-testing on the bubble).
                // Image messages are mostly picture: a plain click opens the readable copy,
                // a shift-click still toggles the selection. Without this, clicking a
                // 250 px thumbnail only highlighted the bubble and never showed the content.
                const image = mediaFor(message).find(entry => entry.kind === 'image');
                if (image && !event.shiftKey && (event.target as HTMLElement).closest('.wxv-bubble')) {
                  event.stopPropagation();
                  openImage(image);
                  return;
                }
                selectMessage(event.shiftKey);
              }}
              role="button"
              tabIndex={0}
              aria-pressed={selected}
              onKeyDown={event => {
                if (event.key === 'Enter' || event.key === ' ') selectMessage(event.shiftKey);
              }}
            >
              {onChatState && <time className="wxv-timeline-time">{message.time > 0 ? new Date(message.time * 1000).toLocaleTimeString(dateLocale(), { hour: '2-digit', minute: '2-digit' }) : '—'}</time>}
              {(!message.is_self || onChatState) && (
                <span className="wxv-avatar sm" style={{ background: avatarColor(message.sender ?? message.text) }}>
                  {message.sender && avatars[message.sender] ? (
                    <img
                      className="wxv-avatar-img"
                      src={avatars[message.sender]}
                      alt=""
                      draggable={false}
                    />
                  ) : (
                    initial(message.sender_name || message.sender || (message.is_self ? t("我") : '?'))
                  )}
                </span>
              )}
              <div className="wxv-msg-body">
                {(onChatState || (active?.is_group && !message.is_self)) && (
                  <span className="wxv-sender">{message.is_self ? t("我") : (message.sender_name || message.sender || t("未知成员"))}</span>
                )}
                <div className={`wxv-bubble ${imageMessage ? 'image-only' : ''} ${voiceMessage ? 'voice-only' : ''} ${placeholder ? 'media' : ''} ${media ? 'media' : ''} ${hit ? 'hit' : ''}`}>
                  {imageMessage || voiceMessage ? null : placeholder ? (
                    message.text
                  ) : (
                    <Highlight text={message.text} keyword={highlightKeyword} />
                  )}
                  {imageMessage && !mediaFor(message).length && <div className="wxv-attach image"><span className="wxv-image-placeholder" role="status">{!mediaScanComplete && <i className="wxv-image-spinner" aria-hidden="true"/>}{mediaScanComplete ? t("图片不可用") : t("正在加载图片…")}</span></div>}
                  {voiceMessage && !mediaFor(message).length && <span className="wxv-attach-chip" role="status">{!mediaScanComplete && <i className="wxv-image-spinner" aria-hidden="true" />}{mediaScanComplete ? t("语音暂不可用") : t("正在加载语音…")}</span>}
                  {mediaFor(message).map(item => {
                    if (item.kind === 'voice') {
                      const text = voiceText[item.server_id_exact];
                      const failure = voiceError[item.server_id_exact];
                      const busyVoice = Boolean(voiceBusy[item.server_id_exact]);
                      return (
                        <div className="wxv-attach voice" key={`${item.local_id}-${item.server_id_exact}`} onClick={event => event.stopPropagation()}>
                          <button type="button" className={`wxv-voice-bubble ${playingVoice === item.server_id_exact ? 'playing' : ''}`} onClick={() => void playVoice(item)} disabled={loadingVoice === item.server_id_exact} aria-label={loadingVoice === item.server_id_exact ? t("正在加载语音") : playingVoice === item.server_id_exact ? t("暂停语音") : t("播放语音")}>
                            {loadingVoice === item.server_id_exact ? <i className="wxv-image-spinner" aria-hidden="true" /> : <svg className="wxv-voice-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" aria-hidden="true"><path d="M4 10v4l4 3V7l-4 3Z" fill="currentColor" stroke="none"/><path d="M11 8.5a5 5 0 0 1 0 7"/><path d="M14 5a10 10 0 0 1 0 14"/></svg>}<span>{item.duration ?? 0}″</span>
                          </button>
                          <button
                            type="button"
                            className="wxv-attach-link"
                            onClick={() => void transcribeVoice(item)}
                            disabled={busyVoice}
                          >
                            {busyVoice ? t("转写中…") : text ? t("重新转写") : t("转写")}
                          </button>
                          {text && <p className="wxv-attach-text">{text}</p>}
                          {failure && <p className="wxv-attach-error">{failure}</p>}
                        </div>
                      );
                    }
                    if (item.kind === 'image') {
                      return (
                        <div className="wxv-attach image" key={`${item.local_id}-${item.server_id_exact}`} onClick={event => event.stopPropagation()}>
                          {item.sources.length ? (
                            <MediaThumb accountId={account?.id ?? ''} item={item} onOpen={() => openImage(item)} />
                          ) : (
                            <span className="wxv-attach-chip">{t("图片 · 本地缺失")}</span>
                          )}
                        </div>
                      );
                    }
                    if (item.kind === 'video') {
                      return (
                        <div className="wxv-attach video" key={`${item.local_id}-${item.server_id_exact}`} onClick={event => event.stopPropagation()}>
                          {item.thumb ? (
                            <button type="button" className="wxv-attach-thumb" onClick={() => revealMedia(item)} title={t("在访达中显示")}>
                              <img src={item.thumb} alt={t("视频封面")} loading="lazy" />
                              <span className="wxv-attach-badge">{item.duration ? `${item.duration}″` : t("视频")}</span>
                            </button>
                          ) : (
                            <span className="wxv-attach-chip">{t("视频")}{item.duration ? ` · ${item.duration}″` : ''}</span>
                          )}
                          {item.rel_path && (
                            <button type="button" className="wxv-attach-link" onClick={() => revealMedia(item)}>
                              {t("在访达中显示")}
                            </button>
                          )}
                        </div>
                      );
                    }
                    if (item.kind === 'file') {
                      return (
                        <div className="wxv-attach file" key={`${item.local_id}-${item.server_id_exact}`} onClick={event => event.stopPropagation()}>
                          <span className="wxv-attach-chip">
                            📄 {item.name ?? t("文件")}
                            {item.size ? ` · ${formatSize(item.size)}` : ''}
                            {!item.exists ? t(" · 本地缺失") : ''}
                          </span>
                          {item.rel_path && (
                            <button type="button" className="wxv-attach-link" onClick={() => revealMedia(item)}>
                              {t("打开")}
                            </button>
                          )}
                        </div>
                      );
                    }
                    return null;
                  })}
                </div>
              </div>
            </div>
          </div>
        );
      })}
      {!messages.length && busy !== 'messages' && <p className="wxv-empty-line">{t("这个会话没有可读取的文字消息。")}</p>}
      </div>
    </div>
  );

  const renderChat = () => {
    if (!active) {
      return (
        <div className="wxv-placeholder">
          <div className="wxv-placeholder-mark">{Icons.chat}</div>
          <h3>{t("选择一个会话开始查看")}</h3>
          <p>{t("左侧按最近消息排序；群聊会在名称下方显示成员数与未读。")}</p>
          <p className="wxv-muted">{t("在搜索框按回车，可以按关键字跨会话检索文字内容。")}</p>
        </div>
      );
    }
    return (
      <>
        <header className="wxv-chat-head">
          <button type="button" className="wxv-back" onClick={() => setMobilePane('list')} aria-label={t("返回会话列表")}>
            {Icons.chevronLeft}
          </button>
          <div className="wxv-chat-title">
            <h2>{active.display}</h2>
            <div className="wxv-chat-meta">
              {active.is_group ? (
                <span className="wxv-badge group">{t("群聊")}{active.member_count ? tf(" · {0} 人", active.member_count) : ''}</span>
              ) : (
                <span className="wxv-badge">{t("单聊")}</span>
              )}
              {active.unread ? <span className="wxv-badge unread">{active.unread} {t("条未读")}</span> : null}
              <span className="mono">{active.username}</span>
            </div>
          </div>
          <div className="wxv-chat-actions">
            {selectedMessages.length > 0 && (
              <button type="button" className="wxv-selection-count" onClick={openWorkPicker}>
                {t("已选")} {selectedMessages.length} {t("条")}
              </button>
            )}
            {canOrganize && (
              <button type="button" className="wxv-btn" onClick={openWorkPicker} disabled={!messages.length}>
                {t("整理工作内容")}
              </button>
            )}
            <button type="button" className="wxv-btn" onClick={exportNotSupported}>
              {t("导出")}
            </button>
          </div>
        </header>

        {highlightKeyword && (
          <div className="wxv-filterbar">
            {highlightHits
              ? tf("正在高亮「{0}」的 {1} 处命中", highlightKeyword, highlightHits)
              : tf("「{0}」在当前记录中没有命中，已尝试向前加载更多消息", highlightKeyword)}
            <button
              type="button"
              className="wxv-link"
              onClick={() => {
                setInlineKeyword('');
                clearSearch();
                onClearSearch?.();
              }}
            >
              {t("取消高亮")}
            </button>
          </div>
        )}

        {renderMessages()}

        <footer className="wxv-chat-foot">
          <span>
            {t("已加载最近")} {messages.length} {t("条")}
            {hasMore ? t("，可继续向前加载") : t("，已到本地记录起点")}
          </span>
          <span>{selectedMessages.length
            ? tf("已选 {0} 条 · 点击消息可切换，Shift + 点击连续选择", selectedMessages.length)
            : t("点击图片查看原图，Shift + 点击消息可加入跨会话选集")}</span>
          {selectedMessages.length > 0 && (
            <button type="button" className="wxv-link" onClick={clearSelectedMessages}>{t("清空选中")}</button>
          )}
          <span className="mono">{messages.length ? listTime(messages[0].time) : ''}</span>
        </footer>
      </>
    );
  };

  /* ── work selection drawer ──────────────────────────────────────────────── */

  const renderWorkSelector = () => (
    <aside className="wxv-digest wxv-workspace">
      <header>
        <div>
          <h3>{workPhase === 'select' ? t("选择工作内容") : t("已选择的聊天记录")}</h3>
          <p className="wxv-muted">
            {workPhase === 'select' ? t("先限定范围，再展开多个会话核对") : tf("{0} · {1} 个会话", selectionTitle(), workGroups.length)}
          </p>
        </div>
        <button type="button" className="wxv-icon" onClick={() => setWorkOpen(false)} aria-label={t("关闭")}>
          {Icons.close}
        </button>
      </header>

      {workPhase === 'select' ? (
        <>
          <section className="wxv-work-section">
            <div className="wxv-work-label">{t("1. 选择时间")}</div>
            <div className="wxv-row wxv-work-chips">
              {(['today', '7d', '30d', 'all', 'custom'] as RangeKey[]).map(range => (
                <button
                  key={range}
                  type="button"
                  className={`wxv-chip ${workRange === range ? 'on' : ''}`}
                  onClick={() => selectWorkRange(range)}
                >
                  {range === 'today' ? t("今天") : range === '7d' ? t("近 7 天") : range === '30d' ? t("近 30 天") : range === 'all' ? t("全部") : t("自定义")}
                </button>
              ))}
            </div>
            {workRange === 'custom' && (
              <div className="wxv-date-grid">
                <label>{t("开始日期")}<input type="date" value={workFrom} onChange={e => setWorkFrom(e.target.value)} /></label>
                <label>{t("结束日期")}<input type="date" value={workTo} onChange={e => setWorkTo(e.target.value)} /></label>
              </div>
            )}
          </section>

          <section className="wxv-work-section">
            <div className="wxv-work-label">{t("2. 指定对话或按关键字定位")}</div>
            <div className="wxv-work-search">
              <input
                value={workKeyword}
                onChange={e => {
                  setWorkKeyword(e.target.value);
                  if (!e.target.value.trim()) setWorkSearch(null);
                }}
                onKeyDown={e => {
                  if (e.key === 'Enter') void runWorkSearch();
                }}
                placeholder={t("输入项目名、客户名或关键字，回车定位")}
                spellCheck={false}
              />
              <button type="button" className="wxv-btn" onClick={() => void runWorkSearch()} disabled={busy === 'search'}>
                {busy === 'search' ? t("定位中…") : t("定位")}
              </button>
            </div>
            {workSearch && (
              <div className="wxv-work-hits">
                <div className="wxv-work-hit-head">
                  <span>“{workSearch.keyword}{t("”命中")} {workSearch.hits.length} {t("个会话")}</span>
                  <button type="button" className="wxv-link" onClick={selectWorkSearchHits}>{t("选择全部命中")}</button>
                </div>
                {workSearch.hits.map(hit => (
                  <button
                    key={hit.username}
                    type="button"
                    className={`wxv-work-hit ${workSelected.includes(hit.username) ? 'on' : ''}`}
                    onClick={() => toggleWorkSession(hit.username)}
                  >
                    <span className="wxv-work-check">{workSelected.includes(hit.username) ? Icons.check : null}</span>
                    <span><strong>{hit.display}</strong><small>{hit.hits}{hit.truncated ? '+' : ''} {t("条命中")}</small></span>
                  </button>
                ))}
                {!workSearch.hits.length && <p className="wxv-empty-line">{t("没有找到包含该关键字的会话。")}</p>}
              </div>
            )}
            <div className="wxv-work-label row-label">
              {t("对话列表")} <span>{workSelected.length} {t("个已选")}{selectedMessages.length ? tf(" · {0} 条消息", selectedMessages.length) : ''}</span>
              <button type="button" className="wxv-link" onClick={() => setWorkSelected(sessions.map(session => session.username))}>{t("全选")}</button>
              <button type="button" className="wxv-link" onClick={() => setWorkSelected([])}>{t("清空")}</button>
            </div>
            {selectedMessages.length > 0 && (
              <div className="wxv-work-selection-note">
                {t("当前将使用已点选的")} {selectedMessages.length} {t("条消息；切换为按时间范围读取将清空消息选集。")}
                <button type="button" className="wxv-link" onClick={clearSelectedMessages}>{t("改为按范围读取")}</button>
              </div>
            )}
            <div className="wxv-work-sessions">
              {sessions.map(session => (
                <label key={session.username} className={`wxv-work-session ${workSelected.includes(session.username) ? 'on' : ''}`}>
                  <input type="checkbox" checked={workSelected.includes(session.username)} onChange={() => toggleWorkSession(session.username)} />
                  <span className="wxv-work-session-text"><strong>{session.display}</strong><small>{session.is_group ? t("群聊") : t("单聊")} · {listTime(session.last_time)}</small></span>
                </label>
              ))}
            </div>
          </section>

          <div className="wxv-work-footer">
            <span>{workSelected.length ? tf("将展开 {0} 个会话", workSelected.length) : t("请选择至少一个会话")}</span>
            <button type="button" className="wxv-btn primary" onClick={() => void loadWorkSelection()} disabled={!workSelected.length || busy === 'work'}>
              {busy === 'work' ? t("读取中…") : t("展开并预览")}
            </button>
          </div>
        </>
      ) : (
        <>
          <div className="wxv-work-review-head">
            <div className="wxv-digest-stats">
              <span>{selectionTitle()}</span>
              <span>{workGroups.length} {t("个会话")}</span>
              <span>{workGroups.reduce((sum, group) => sum + group.messages.length, 0)} {t("条消息")}</span>
            </div>
            <button type="button" className="wxv-link" onClick={() => setWorkPhase('select')}>{t("返回调整")}</button>
          </div>
          {renderAiSummary()}
          <div className="wxv-work-review">
            {workGroups.map(group => (
              <section key={group.session.username} className="wxv-work-group">
                <header>
                  <div><strong>{group.session.display}</strong><small>{group.session.is_group ? t("群聊") : t("单聊")} · {group.messages.length} {t("条")}</small></div>
                  {group.truncated && <span className="wxv-work-truncated">{t("已达单会话 5000 条上限")}</span>}
                </header>
                {[...group.messages].reverse().map(message => (
                  <article key={`${group.session.username}-${message.local_id}-${message.time}`} className={`wxv-work-message ${message.is_self ? 'self' : ''}`}>
                    <div><strong>{message.is_self ? t("我") : message.sender_name || message.sender || t("未知")}</strong><time>{dayLabel(toDate(message.time) ?? new Date())}</time></div>
                    <p><Highlight text={message.text} keyword={workKeyword} /></p>
                  </article>
                ))}
              </section>
            ))}
            {!workGroups.some(group => group.messages.length > 0) && <p className="wxv-empty-line">{t("当前筛选条件没有消息。")}</p>}
          </div>
          {renderWorkInsights()}
          <div className="wxv-work-next">
            <div><strong>{t("下一步")}</strong><span>{t("先核对原文，再选择处理方式")}</span></div>
            <div className="wxv-row">
              <button
                type="button"
                className="wxv-btn primary"
                onClick={() => {
                  setError('');
                  setNotice('');
                  setAiConfirm(false);
                  setActionPicker(value => !value);
                }}
                disabled={busy === 'explore' || busy === 'ai' || !workGroups.some(group => group.messages.length > 0)}
                aria-expanded={actionPicker}
              >
                {busy === 'ai' ? t("Agent 总结中…") : busy === 'explore' ? t("探索中…") : workInsights || aiSummary ? t("重新整理这段聊天") : t("整理这段聊天")}
              </button>
            </div>
            {actionPicker && (
              <div className="wxv-action-picker">
                <button
                  type="button"
                  className="wxv-action-option"
                  onClick={() => {
                    setActionPicker(false);
                    setAiConfirm(false);
                    void generateWorkInsights();
                  }}
                >
                  <span className="wxv-action-icon">{Icons.list}</span>
                  <span className="wxv-action-text">
                    <strong>{t("本地快速整理")}</strong>
                    <small>{t("关键词归类成风险 / 决定 / 待办，并找相关会话。不联网。")}</small>
                  </span>
                </button>
                {canUseAi && <button
                  type="button"
                  className="wxv-action-option"
                  onClick={() => {
                    if (!aiReady) return;
                    setActionPicker(false);
                    setError('');
                    setNotice('');
                    setAiConfirm(true);
                  }}
                  disabled={!aiReady}
                  title={aiReady ? tf("使用 {0} 生成", agentCurrent?.agent_name) : t("当前没有可用的 Agent")}
                >
                  <span className="wxv-action-icon">{Icons.agent}</span>
                  <span className="wxv-action-text">
                    <strong>{t("AI 总结")}</strong>
                    <small>
                      {aiReady
                        ? tf("交给 {0}，原文可能离开本机。", agentCurrent?.agent_name)
                        : canConfigureAgent ? t("请在「设置 → Agent 集成」中配置可用的 Agent。") : t("当前没有可用的 Agent。")}
                    </small>
                  </span>
                </button>}
              </div>
            )}
            {aiConfirm && createPortal((
              <div
                className="wxv-modal-layer"
                role="presentation"
                onMouseDown={event => {
                  if (event.target === event.currentTarget) closeAiConfirm();
                }}
              >
                <div className="wxv-modal-card" role="dialog" aria-modal="true" aria-labelledby="ai-confirm-title">
                  <header className="wxv-modal-head">
                    <span className="wxv-modal-icon">{Icons.agent}</span>
                    <div className="wxv-modal-title">
                      <h3 id="ai-confirm-title">
                        {aiReady ? tf("使用 {0} 生成总结", agentCurrent?.agent_name) : t("尚未配置可用的本地 Agent")}
                      </h3>
                      <p>
                        {aiReady
                          ? t("先核对范围，再把原文交给该 Agent。")
                          : t("当前没有可用的 Agent。")}
                      </p>
                    </div>
                    <button type="button" className="wxv-modal-close" onClick={closeAiConfirm} aria-label={t("关闭")}>
                      {Icons.close}
                    </button>
                  </header>

                  {aiReady ? (
                    <>
                      <dl className="wxv-modal-stats">
                        <div>
                          <dt>{t("会话")}</dt>
                          <dd>{aiPlan.sessions}</dd>
                        </div>
                        <div>
                          <dt>{t("消息")}</dt>
                          <dd>{aiPlan.count}</dd>
                        </div>
                        <div>
                          <dt>{t("字数")}</dt>
                          <dd>{t("约")} {Math.min(aiPlan.characters, AI_MAX_CHARS).toLocaleString(dateLocale())}</dd>
                        </div>
                      </dl>
                      {(aiPlan.capped || aiPlan.characters > AI_MAX_CHARS) && (
                        <p className="wxv-modal-note">
                          {aiPlan.capped ? tf("每个会话最多取最近 {0} 条。", AI_MESSAGES_PER_SESSION) : ''}
                          {aiPlan.characters > AI_MAX_CHARS ? t("超出上限的内容会按时间顺序截断。") : ''}
                        </p>
                      )}
                      <p className="wxv-modal-warn">
                        {Icons.warning}
                        {t("聊天原文会离开本机，交给该 Agent 处理。")}
                      </p>
                      <footer className="wxv-modal-actions">
                        <button type="button" className="wxv-btn" onClick={closeAiConfirm} disabled={busy === 'ai'}>
                          {t("取消")}
                        </button>
                        <button
                          type="button"
                          className="wxv-btn primary"
                          onClick={() => void runAiSummary()}
                          disabled={busy === 'ai' || !aiPlan.count}
                        >
                          {busy === 'ai' ? t("生成中…") : t("确认生成")}
                        </button>
                      </footer>
                    </>
                  ) : (
                    <footer className="wxv-modal-actions">
                      <button type="button" className="wxv-btn" onClick={closeAiConfirm}>
                        {t("知道了")}
                      </button>
                    </footer>
                  )}
                </div>
              </div>
            ), document.body)}
          </div>
        </>
      )}
    </aside>
  );

  /* ── shell ───────────────────────────────────────────────────────────────── */

  return (
    <div className={`wxv ${mode === 'settings' ? 'mode-settings' : ''} ${wizard ? 'wizard-mode' : ''} ${workOpen && !forceUnlock ? 'with-digest' : ''} pane-${mobilePane}`} data-wizard-step={wizardStep}>
      <aside className="wxv-rail">
        <div className="wxv-rail-head">
          <div>
            <strong>{account?.id ?? t("未发现账号")}</strong>
            <small>
              {ready
                ? tf("{0} 个会话 · {1} 个群聊", sessions.length, groups.length)
                : encryptedDbs.length
                  ? tf("密钥 {0}/{1}", keyedCount, encryptedDbs.length)
                  : t("等待扫描")}
            </small>
            {ready && (
              <small className={`wxv-sync-status ${wechatEnabled ? syncState : 'error'}`}>
                {!wechatEnabled
                  ? t("已断开 · 到「连接器」重新连接")
                  : syncState === 'syncing' ? t("正在同步最新消息…") : syncState === 'error' ? t("同步暂不可用") : lastSyncAt ? tf("准实时同步 · {0}", timeOnly(Math.floor(lastSyncAt.getTime() / 1000))) : t("准实时同步已开启")}
              </small>
            )}
          </div>
          {ready && (
            <button
              type="button"
              className={`wxv-sync-toggle ${liveSync ? 'on' : ''}`}
              onClick={() => setLiveSync(value => !value)}
              title={liveSync ? t("关闭准实时同步") : t("开启准实时同步")}
              aria-pressed={liveSync}
            >
              {liveSync ? Icons.pause : Icons.play}
            </button>
          )}
          <button
            type="button"
            className={`wxv-icon ${busy === 'scan' ? 'spin' : ''}`}
            onClick={() => void doScan(accountId)}
            disabled={busy === 'scan'}
            title={t("重新扫描")}
          >
            {Icons.refresh}
          </button>
        </div>

        <div className="wxv-searchbox">
          <svg viewBox="0 0 16 16" aria-hidden="true">
            <circle cx="7" cy="7" r="5" />
            <path d="M11 11l4 4" />
          </svg>
          <input
            value={query}
            onChange={e => {
              setQuery(e.target.value);
              if (!e.target.value) setSearch(null);
            }}
            onKeyDown={e => {
              if (e.key === 'Enter') void runSearch();
            }}
            placeholder={ready ? t("搜索会话，回车检索聊天内容") : t("搜索会话")}
            spellCheck={false}
          />
          {query && (
            <button type="button" className="wxv-clear" onClick={clearSearch} aria-label={t("清空")}>
              {Icons.close}
            </button>
          )}
        </div>

        <div className="wxv-tabs">
          <button type="button" className={tab === 'all' ? 'on' : ''} onClick={() => setTab('all')}>
            {t("全部")} {sessions.length}
          </button>
          <button type="button" className={tab === 'group' ? 'on' : ''} onClick={() => setTab('group')}>
            {t("群聊")} {groups.length}
          </button>
        </div>

        {ready ? (search ? renderSearchResults() : renderSessions()) : (
          <div className="wxv-rail-empty">
            {busy === 'scan' ? t("正在扫描微信数据目录…") : needsWhitelist ? t("请先配置聊天白名单。") : t("解密完成后，会话会出现在这里。")}
          </div>
        )}

      </aside>

      <section className="wxv-main">
        {mode === 'settings' ? (
          renderUnlock()
        ) : ready ? (
          renderChat()
        ) : !account && busy !== 'scan' ? (
          // Nothing to read yet. Show the unlock panel instead of an endless "正在准备…" —
          // that panel is where the Full Disk Access instructions live.
          renderUnlock()
        ) : needsWhitelist ? (
          renderWhitelistRequired()
        ) : locked ? (
          <div className="wxv-placeholder">
            <h3>{t("还没有可查看的聊天记录")}</h3>
            <p className="wxv-muted">{t("在「本地数据」里导入或捕获密钥，然后解密当前账号。")}</p>
            {onOpenData && (
              <button type="button" className="wxv-btn primary" onClick={onOpenData}>
                {t("前往本地数据")}
              </button>
            )}
          </div>
        ) : (
          <div className="wxv-placeholder">
            <h3>{t("正在准备…")}</h3>
            <p className="wxv-muted">{t("首次解密需要读取整个数据库，请稍候。")}</p>
          </div>
        )}
        {notice && !wizard && <div className="wxv-toast">{notice}</div>}
        {error && isTauri() && (
          <div className="wxv-toast error">
            <span>{t(error)}</span>
            <button type="button" className="wxv-icon" onClick={() => setError('')} aria-label={t("关闭提示")}>
              {Icons.close}
            </button>
          </div>
        )}
      </section>

      {imageViewer && createPortal(<div className="wxv-image-viewer" role="dialog" aria-modal="true" aria-label={t("查看图片")} onClick={closeImage}>
        <button type="button" className="wxv-image-viewer-close" aria-label={t("关闭图片")} onClick={closeImage} autoFocus>×</button>
        {imageViewer.error ? <p role="alert">{imageViewer.error}</p> : imageViewer.src ? <img src={imageViewer.src} alt={t("聊天图片原图")} onClick={event => event.stopPropagation()} onError={() => setImageViewer({ src: null, error: t("图片加载失败。") })}/> : <p role="status"><i className="wxv-image-spinner" aria-hidden="true"/>{t("正在加载原图…")}</p>}
      </div>, document.body)}
      {/* The capture guide needs the full column width; the digest drawer would clip it. */}
      {workOpen && !forceUnlock && renderWorkSelector()}
    </div>
  );
}
