import { formatSize } from '../../../shared/content/localDataFormat';
import type { Account, ScanResult, DecryptOutcome } from '../../connectors/wechat/nativeTypes';
import { useWeChatConnection } from '../../connectors/wechat/useWeChatConnection';
import { WeChatConnectionPanel } from '../../connectors/wechat/WeChatConnectionPanel';
import { WeChatConnectionWizard } from '../../connectors/wechat/WeChatConnectionWizard';
import { MediaThumb } from './MediaThumb';
import type {
  SessionItem,
  MessageItem,
  MediaItem,
  MediaReport,
  VoiceResult,
  SearchHit,
  SearchReport,
  AgentCurrent,
  AgentSummary,
} from './nativeTypes';
import { toDate, pad, dayKey, dayLabel, timeOnly, listTime, avatarColor, initial, Highlight } from './presentation';
import { isSummaryText } from '../summary/summaryInput';
import { t, tf, dateLocale } from '../../../shared/i18n/locale';
import { RemoteChatFallback } from '../RemoteChatFallback';
import { LoadingSkeleton } from '../../../shared/ui/LoadingSkeleton';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { convertFileSrc, isTauri } from '@tauri-apps/api/core';
import { desktopInvoke as invoke } from '../../../shared/api/desktopInvoke';
import './wechat.css';
import { Icons } from '../../../shared/ui/icons';
import {
  HISTORY_KIND_LABEL,
  renderAgentMarkdown,
  type HistorySession,
  type NewSummaryRecord,
  type SummaryRecord,
} from '../../../shared/content/summary';
import { classifyWorkText, extractWorkTerms, type InsightCategory } from '../summary/workInsights';
import { useCapability, useInternalAccount } from '../../auth/entitlements';
import { isSettingsConflict, requireSettingsRevision, settingsSaveError, type AppSettings } from '../../settings/settingsModel';

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

const PAGE = 100;
const WORK_PAGE = 500;
const WORK_MAX_MESSAGES_PER_SESSION = 5000;
/** Matches the per-conversation cap in the Rust transcript builder. */
const AI_MESSAGES_PER_SESSION = 400;
/** Matches `SUMMARY_MAX_CHARS` in the Rust transcript builder. */
const AI_MAX_CHARS = 45_000;

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

export default function WeChatLocal({
  mode = 'chat',
  onOpenChat,
  onOpenData,
  onOpenWhitelist,
  wizard = false,
  wizardStep = 'wizard-method',
  wizardManual = false,
  wizardFinishing = false,
  wizardInstall = { ready: null, message: '' },
  onWizardInstallRecheck,
  onWizardStep,
  onWizardBack,
  onWizardNext,
  focusConversation,
  focusMessageId,
  searchKeyword = '',
  onClearSearch,
  sessionRevision = 0,
  accountId: pinnedAccountId,
  onChatState,
  onSummarySelection,
}: {
  mode?: 'chat' | 'settings';
  onOpenChat?: () => void;
  onOpenData?: () => void;
  onOpenWhitelist?: () => void;
  wizard?: boolean;
  wizardStep?: string;
  wizardManual?: boolean;
  wizardFinishing?: boolean;
  wizardInstall?: { ready: boolean | null; message: string };
  onWizardInstallRecheck?: () => void;
  onWizardStep?: (step: string) => void;
  onWizardBack?: () => void;
  onWizardNext?: () => void;
  focusConversation?: string | null;
  focusMessageId?: string | number | null;
  /** Keyword typed in the embedding page's header. Highlights matches in the open chat. */
  searchKeyword?: string;
  onClearSearch?: () => void;
  sessionRevision?: number;
  /** The account the embedding page already resolved (WeChat or WhatsApp). */
  accountId?: string | null;
  /** Reports loaded and selected messages to the page-level actions. */
  onChatState?: (state: { active: boolean; messages: number; selected: number; agentReady: boolean }) => void;
  onSummarySelection?: (messages: import('../summary/summaryInput').SummaryMessage[]) => void;
} = {}) {
  // 整理工作内容 runs the local summary agent, which is an internal-account feature.
  const { internal: internalAccount } = useInternalAccount();
  const { allowed: workDigestGranted } = useCapability('ai.work_digest');
  const { allowed: aiSummaryGranted } = useCapability('ai.agent_summary');
  const { allowed: canConfigureAgent } = useCapability('ai.local_agent');
  const canOrganize = internalAccount && workDigestGranted;
  const canUseAi = internalAccount && aiSummaryGranted;
  const [scan, setScan] = useState<ScanResult | null>(null);
  const [accountId, setAccountId] = useState('');
  const settingsRef = useRef<AppSettings | null>(null);
  const accountSwitchLock = useRef(false);
  const [accountSwitching, setAccountSwitching] = useState(false);
  const [busy, setBusy] = useState<'scan' | 'import' | 'messages' | 'search' | 'work' | 'explore' | 'ai' | null>(null);
  const [sessions, setSessions] = useState<SessionItem[]>([]);
  const [sessionsLoaded, setSessionsLoaded] = useState(false);
  const [active, setActive] = useState<SessionItem | null>(null);
  const [messages, setMessages] = useState<MessageItem[]>([]);
  const [localMissing, setLocalMissing] = useState<string | null>(null);
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
  const [liveSync, setLiveSync] = useState(false);
  const [wechatEnabled, setWechatEnabled] = useState(true);
  const [syncIntervalSeconds, setSyncIntervalSeconds] = useState(30);
  const [syncState, setSyncState] = useState<'idle' | 'syncing' | 'ready' | 'error'>('idle');
  const [lastSyncAt, setLastSyncAt] = useState<Date | null>(null);
  const [whitelistConfigured, setWhitelistConfigured] = useState<boolean | null>(null);
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
  const ready = sessionsLoaded && sessions.length > 0;
  const locked = sessionsLoaded && sessions.length === 0;
  const needsWhitelist = sessionsLoaded && whitelistConfigured === false;

  const focusLookup = useRef('');
  const focusConversationRef = useRef(focusConversation);
  focusConversationRef.current = focusConversation;
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
        const list = await invoke<SessionItem[]>('chat_sessions', { accountId: id, focusUsername: focusConversationRef.current || undefined });
        const focus = focusConversationRef.current;
        const shown = !list.length && focus ? [{ username: focus, display: focus, is_group: focus.endsWith('@chatroom') || focus.endsWith('@qy_g'), last_time: null, member_count: null, preview: null, unread: null }] : list;
        setSessions(shown);
        setSessionsLoaded(true);
        return shown;
      } catch (e) {
        const message = String(e);
        const focus = focusConversationRef.current;
        setSessions(focus ? [{ username: focus, display: focus, is_group: focus.endsWith('@chatroom') || focus.endsWith('@qy_g'), last_time: null, member_count: null, preview: null, unread: null }] : []);
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

  /** Drop every view derived from the decrypted snapshot (used when that snapshot is deleted). */
  const resetDecryptedView = useCallback(() => {
    setSessions([]);
    // With no decrypted copy left, the page must fall back to the unlock panel instead of
    // the "正在准备…" placeholder. `locked` is derived from sessionsLoaded + an empty list.
    setSessionsLoaded(true);
    setActive(null);
    setMessages([]);
    setSelectedMessages([]);
    setSelectionAnchor(null);
    setWorkGroups([]);
    setWorkInsights(null);
    setWorkOpen(false);
    setWorkPhase('select');
    setSearch(null);
    setQuery('');
    setWhitelistConfigured(null);
  }, []);

  const connection = useWeChatConnection({
    account, accountId, accountSwitching, mode, wizard, wizardStep, wizardManual, onWizardStep,
    setNotice, setError, setBusy, loadSessions, resetReader: resetDecryptedView,
    clearMessageCache: () => recentMessages.current.clear(), clearing: clearingRef,
  });
  const { forceUnlock, resetAccountResult, initialize: initializeConnection } = connection;
  const { encryptedDbs, keyedCount, refreshKeys } = connection.keys;

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
      const list = await invoke<SessionItem[]>('chat_sessions', { accountId: account.id, focusUsername: focusConversationRef.current || undefined });
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
            return [...latest, ...current.filter(message => !keys.has(`${message.source_shard}:${message.local_id}:${message.time}`))].slice(0, 200);
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
      await initializeConnection();
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
        resetAccountResult();
        setSelectedMessages([]);
        setSelectionAnchor(null);
        setWorkGroups([]);
        setWorkInsights(null);
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
    [accountId, loadSessions, rememberAccount, resetAccountResult],
  );

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
      setLocalMissing(null);
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
        setMessages(keyword ? merged.slice(-200) : merged.slice(0, 200));
        setHasMore(Boolean(cached?.hasMore) || list.length >= PAGE);
        pendingHighlight.current = keyword;
      pendingFocusMessage.current = focusMessageId == null ? '' : String(focusMessageId);
      } catch (e) {
        if (request === sessionRequest.current && activeRef.current === session.username) { if (String(e).startsWith('LOCAL_CHAT_MISSING:')) setLocalMissing(session.username); else setError(String(e)); }
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
    recentMessages.current.set(key, { messages: messages.slice(0, 200), hasMore: hasMore || messages.length > 200 });
    while (recentMessages.current.size > 3) {
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
      setMessages(current => [...current, ...older].slice(-200));
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
    if (!focusConversation || !ready || !account) return;
    if (!sessions.some(item => item.username === focusConversation)) { const key = `${account.id}:${focusConversation}`; if (focusLookup.current !== key) { focusLookup.current = key; void loadSessions(account.id); } return; }
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

  useEffect(() => {
    onChatState?.({
      active: Boolean(active),
      messages: messages.length,
      selected: selectedMessages.length,
      agentReady: Boolean(agentCurrent?.configured && agentCurrent.available),
    });
  }, [active?.username, messages.length, selectedMessages.length, agentCurrent?.configured, agentCurrent?.available, onChatState]);

  useEffect(()=>{
    onSummarySelection?.(selectedMessages.filter(item=>isSummaryText(item.message)).map(({session,message})=>({username:session.username,display:session.display,is_group:session.is_group,sender:message.sender_name||message.sender||'',time:message.time,text:message.text,local_id:message.local_id,source_shard:message.source_shard})));
  },[selectedMessages,onSummarySelection]);

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

  /* ── chat pane ───────────────────────────────────────────────────────────── */

  const renderMessages = () => (
    <div className="wxv-messages" ref={messageRef} onScroll={event => {
      const root = event.currentTarget;
      if (root.clientHeight > 0) followNewest.current = root.scrollHeight - root.scrollTop - root.clientHeight <= 24;
    }}>
      <div ref={messageContentRef}>
      {busy === 'messages' && !messages.length && <LoadingSkeleton label={t("正在加载聊天记录")} kind="messages" />}
      {localMissing && account && <RemoteChatFallback key={`${account.id}:${localMissing}`} accountId={account.id} username={localMissing} />}
      {!localMissing && hasMore && (
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
        const videoMessage = !isWhatsApp && base === 43;
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
                const video = mediaFor(message).find(entry => entry.kind === 'video');
                if (video && !event.shiftKey && (event.target as HTMLElement).closest('.wxv-bubble')) {
                  event.stopPropagation();
                  revealMedia(video);
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
                  <span className="wxv-sender">{message.is_self ? t("我") : (message.sender_name || message.sender || t("未知成员"))}{onChatState && <time className="wxv-message-time">{message.time > 0 ? new Date(message.time * 1000).toLocaleTimeString(dateLocale(), { hour: '2-digit', minute: '2-digit' }) : '—'}</time>}</span>
                )}
                <div className={`wxv-bubble ${imageMessage || videoMessage ? 'image-only' : ''} ${voiceMessage ? 'voice-only' : ''} ${placeholder ? 'media' : ''} ${media ? 'media' : ''} ${hit ? 'hit' : ''}`}>
                  {imageMessage || voiceMessage || videoMessage ? null : placeholder ? (
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
                            <span className="wxv-attach-chip">{item.duration ? `${item.duration}″` : t("视频")}</span>
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
      {!localMissing && !messages.length && busy !== 'messages' && <p className="wxv-empty-line">{t("这个会话没有可读取的文字消息。")}</p>}
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

        {!localMissing && <footer className={`wxv-chat-foot${selectedMessages.length ? ' has-selection' : ''}`}>
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
        </footer>}
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

  const checkAccountExists = async () => { const result = await invoke<ScanResult>('wechat_scan'); setScan(result); return result.accounts.some(item => item.id === accountId); };
  const accountSelection = { accountId, scan, accountSwitching, busy, doScan, switchAccount };
  const connectionWizard = {
    enabled: wizard, step: wizardStep, manual: wizardManual, finishing: wizardFinishing,
    install: wizardInstall, onInstallRecheck: onWizardInstallRecheck, onStep: onWizardStep,
    onBack: onWizardBack, onNext: onWizardNext,
  };

  /* ── shell ───────────────────────────────────────────────────────────────── */

  return (
    <div className={`wxv ${mode === 'settings' ? 'mode-settings' : ''} ${wizard ? 'wizard-mode' : ''} ${workOpen && !forceUnlock ? 'with-digest' : ''} pane-${mobilePane}`} data-wizard-step={wizard && wizardStep !== 'wizard-method' ? 'wizard-checklist' : wizardStep}>
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
          wizard && wizardStep !== 'wizard-method' ? <WeChatConnectionWizard connection={connection} selection={accountSelection} wizard={connectionWizard} ready={ready} error={error} checkAccountExists={checkAccountExists} /> : <WeChatConnectionPanel connection={connection} selection={accountSelection} wizard={connectionWizard} mode={mode} ready={ready} notice={notice} onOpenChat={onOpenChat} />
        ) : ready ? (
          renderChat()
        ) : !account && busy !== 'scan' ? (
          // Nothing to read yet. Show the unlock panel instead of an endless "正在准备…" —
          // that panel is where the Full Disk Access instructions live.
          <WeChatConnectionPanel connection={connection} selection={accountSelection} wizard={connectionWizard} mode={mode} ready={ready} notice={notice} onOpenChat={onOpenChat} />
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
        {error && isTauri() && !wizard && (
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
