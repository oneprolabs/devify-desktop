import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  HISTORY_KIND_LABEL,
  SummaryDetail,
  type NewSummaryRecord,
  type SummaryRecord,
} from './summary';
import { classifyWorkText, extractWorkTerms } from './workInsights';
import { currentSessionSummaries } from './chatPresentation';

/** Minimal conversation shape shared with the chat page. */
export type SummarySession = {
  username: string;
  display: string;
  is_group: boolean;
  last_time: number | null;
};

type MessageItem = {
  local_id: number;
  time: number;
  sender: string | null;
  sender_name: string | null;
  kind: number;
  text: string;
  is_self: boolean;
};

type AgentCurrent = {
  configured: boolean;
  agent_id: string;
  agent_name: string;
  available: boolean;
};

type AgentSummary = {
  agent_name: string;
  model: string;
  output: string;
  sessions: number;
  messages: number;
  characters: number;
  truncated: boolean;
  elapsed_ms: number;
};

type SummaryMode = 'local' | 'ai';
type RangeId = 'today' | 'yday' | 'd3' | 'week';

const RANGES: Array<{ id: RangeId; label: string }> = [
  { id: 'today', label: '今天' },
  { id: 'yday', label: '昨天' },
  { id: 'd3', label: '近 3 天' },
  { id: 'week', label: '本周' },
];

const WORK_PAGE = 500;
const WORK_MAX_MESSAGES_PER_SESSION = 5000;

const pad = (value: number) => (value < 10 ? `0${value}` : String(value));
const dayKey = (date: Date) => `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
const monthDay = (date: Date) => `${pad(date.getMonth() + 1)}/${pad(date.getDate())}`;

function toDate(seconds: number): Date | null {
  if (!seconds) return null;
  const date = new Date(seconds > 1e12 ? seconds : seconds * 1000);
  return Number.isNaN(date.getTime()) ? null : date;
}

function startOfDay(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

/** Calendar bounds for the four pills, plus the date hint shown next to them. */
function rangeBounds(id: RangeId): { since: number; until: number; label: string; hint: string } {
  const now = new Date();
  const today = startOfDay(now);
  const epoch = (date: Date) => Math.floor(date.getTime() / 1000);
  if (id === 'today') {
    return { since: epoch(today), until: epoch(now), label: '今天', hint: monthDay(today) };
  }
  if (id === 'yday') {
    const start = new Date(today);
    start.setDate(start.getDate() - 1);
    const end = new Date(today.getTime() - 1000);
    return { since: epoch(start), until: epoch(end), label: '昨天', hint: monthDay(start) };
  }
  if (id === 'd3') {
    const start = new Date(today);
    start.setDate(start.getDate() - 2);
    return { since: epoch(start), until: epoch(now), label: '近 3 天', hint: `${monthDay(start)} – ${monthDay(now)}` };
  }
  const start = new Date(today);
  start.setDate(start.getDate() - ((today.getDay() + 6) % 7));
  return { since: epoch(start), until: epoch(now), label: '本周', hint: `${monthDay(start)} – ${monthDay(now)}` };
}

async function fetchRange(
  accountId: string,
  username: string,
  since: number,
  until: number,
): Promise<{ messages: MessageItem[]; truncated: boolean }> {
  let before: number | undefined;
  let beforeId: number | undefined;
  let collected: MessageItem[] = [];
  let truncated = false;
  for (let page = 0; page < 50; page += 1) {
    const batch = await invoke<MessageItem[]>('wechat_messages', {
      accountId,
      username,
      limit: WORK_PAGE,
      before,
      beforeId,
    });
    if (!batch.length) break;
    collected = [...collected, ...batch];
    const last = batch[batch.length - 1];
    const oldest = last?.time;
    const oldestSeconds = oldest ? (oldest > 1e12 ? oldest / 1000 : oldest) : undefined;
    if ((oldestSeconds !== undefined && oldestSeconds <= since) || batch.length < WORK_PAGE) break;
    if (collected.length >= WORK_MAX_MESSAGES_PER_SESSION) {
      collected = collected.slice(0, WORK_MAX_MESSAGES_PER_SESSION);
      truncated = true;
      break;
    }
    before = last.time;
    beforeId = last.local_id;
  }
  const messages = collected.filter(message => {
    const seconds = message.time > 1e12 ? message.time / 1000 : message.time;
    return seconds >= since && seconds <= until;
  });
  return { messages, truncated };
}

/* ── card content ─────────────────────────────────────────────────────────── */

type CardItem = { text: string; tag?: string };
type CardSection = { title: string; items: CardItem[] };

/** Agent output is Markdown; the card only needs short plain sentences. */
function plainText(value: string): string {
  return value
    .replace(/!\[[^\]]*\]\([^)]*\)/g, '')
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/[*`~]/g, '')
    .replace(/\s+/g, ' ')
    .trim();
}

function parseOutput(output: string): { lead: string; sections: CardSection[] } {
  const lines = output.replace(/\r\n?/g, '\n').split('\n');
  let lead = '';
  const sections: CardSection[] = [];
  let current: CardSection | null = null;
  for (const raw of lines) {
    const line = raw.trim();
    if (!line) continue;
    const heading = /^#{1,4}\s+(.*)$/.exec(line);
    if (heading) {
      const title = plainText(heading[1]);
      // 「概览」 is the lead paragraph of the card, not its own section.
      current = title === '概览' ? null : { title, items: [] };
      if (current) sections.push(current);
      continue;
    }
    const bullet = /^[-*+]\s+(.*)$/.exec(line) ?? /^\d+[.)]\s+(.*)$/.exec(line);
    const text = plainText(bullet ? bullet[1] : line);
    if (!text) continue;
    if (!current) {
      lead = lead ? `${lead} ${text}` : text;
      continue;
    }
    if (bullet || !current.items.length) current.items.push({ text });
    else current.items[current.items.length - 1].text += ` ${text}`;
  }
  return { lead, sections: sections.filter(section => section.items.length > 0) };
}

function localCard(record: SummaryRecord): { lead: string; sections: CardSection[] } {
  const focus = record.insights?.focus ?? [];
  const timeline = record.insights?.timeline ?? [];
  const related = record.insights?.related ?? [];
  const counts = new Map<string, number>();
  focus.forEach(item => counts.set(item.category, (counts.get(item.category) ?? 0) + 1));
  const summary = [...counts.entries()].map(([category, count]) => `${category} ${count}`).join(' · ');
  const lead = focus.length
    ? `按关键词归类了 ${record.messages} 条消息：${summary}。这是规则整理，不是 AI 结论。`
    : `按关键词归类了 ${record.messages} 条消息，没有识别到待办、决定或风险线索。这是规则整理，不是 AI 结论。`;
  const sections: CardSection[] = [];
  if (focus.length) {
    sections.push({
      title: '项目重点',
      items: focus.slice(0, 8).map(item => ({ text: item.text, tag: item.category })),
    });
  }
  if (timeline.length) {
    sections.push({
      title: '时间线',
      items: [...timeline].reverse().slice(0, 6).map(entry => ({
        text: `${entry.day} · ${entry.messages} 条${entry.sessions.length ? ` · ${entry.sessions.join('、')}` : ''}`,
      })),
    });
  }
  if (related.length) {
    sections.push({
      title: '相关会话',
      items: related.slice(0, 5).map(item => ({ text: `${item.display} · ${item.hits}${item.truncated ? '+' : ''} 条相关命中` })),
    });
  }
  return { lead, sections };
}

function cardContent(record: SummaryRecord) {
  if (record.kind === 'local') return localCard(record);
  if (record.output) {
    const parsed = parseOutput(record.output);
    if (parsed.lead || parsed.sections.length) return parsed;
  }
  return { lead: record.note || '这条记录没有保存正文。', sections: [] as CardSection[] };
}

/* ── component ────────────────────────────────────────────────────────────── */

/** Empty-state copy that names the scope that produced no records. */
function summaryScopeEmpty(scopedToSession: boolean, total: number): string {
  if (scopedToSession && total > 0) return '当前会话暂无总结。';
  return '还没有保存过总结记录。';
}

export function ChatSummary({
  accountId,
  sessions,
  session,
  summaries,
  query,
  onReload,
  onOpenRaw,
  onBusyChange,
}: {
  accountId: string | null;
  sessions: SummarySession[];
  session: SummarySession | null;
  summaries: SummaryRecord[];
  query: string;
  onReload: () => Promise<void>;
  onOpenRaw: (username: string) => void;
  onBusyChange?: (busy: boolean) => void;
}) {
  const [open, setOpen] = useState(false);
  const [range, setRange] = useState<RangeId>('today');
  const [mode, setMode] = useState<SummaryMode>('local');
  const [agent, setAgent] = useState<AgentCurrent | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [confirmAi, setConfirmAi] = useState(false);
  const [reader, setReader] = useState<SummaryRecord | null>(null);
  const pageRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    void invoke<AgentCurrent>('agent_current').then(setAgent).catch(() => setAgent(null));
  }, []);

  useEffect(() => {
    setNotice('');
    setError('');
    setReader(null);
  }, [accountId, session?.username]);

  useEffect(() => {
    onBusyChange?.(busy);
  }, [busy, onBusyChange]);

  const scoped = useMemo(() => {
    const bySession = currentSessionSummaries(summaries, accountId, session?.username);
    const keyword = query.trim().toLowerCase();
    return keyword ? bySession.filter(record => [record.range_label, record.output ?? '', ...record.sessions.map(item => item.display)].join(' ').toLowerCase().includes(keyword)) : bySession;
  }, [summaries, accountId, session, query]);

  const bounds = rangeBounds(range);
  const chatUsernames = useMemo(() => session ? [session.username] : [], [session]);

  const agentReady = Boolean(agent?.configured && agent.available);
  const buildLocal = async (usernames: string[]): Promise<NewSummaryRecord> => {
    if (!accountId) throw new Error('还没有可用的微信账号。');
    const selected = usernames
      .map(username => sessions.find(item => item.username === username))
      .filter((item): item is SummarySession => Boolean(item));
    if (!selected.length) throw new Error('请至少选择一个聊天。');
    const groups = await Promise.all(selected.map(async item => ({
      session: item,
      ...(await fetchRange(accountId, item.username, bounds.since, bounds.until)),
    })));
    const total = groups.reduce((sum, group) => sum + group.messages.length, 0);
    if (!total) throw new Error('这个时间范围没有可总结的聊天记录。');

    const timeline = new Map<string, { messages: number; sessions: Set<string> }>();
    const focus: NonNullable<NewSummaryRecord['insights']>['focus'] = [];
    const flat: MessageItem[] = [];
    let characters = 0;
    groups.forEach(group => {
      group.messages.forEach(message => {
        const date = toDate(message.time);
        const day = date ? dayKey(date) : '未知日期';
        const entry = timeline.get(day) ?? { messages: 0, sessions: new Set<string>() };
        entry.messages += 1;
        entry.sessions.add(group.session.display);
        timeline.set(day, entry);
        characters += message.text.length;
        flat.push(message);
        const category = classifyWorkText(message.text);
        if (category) {
          focus.push({
            category,
            session: group.session.display,
            day: date ? `${monthDay(date)} ${pad(date.getHours())}:${pad(date.getMinutes())}` : '未知日期',
            text: message.text,
          });
        }
      });
    });

    const terms = extractWorkTerms(flat);
    const related: NonNullable<NewSummaryRecord['insights']>['related'] = [];

    return {
      kind: 'local',
      account_id: accountId,
      range_label: bounds.label,
      since: bounds.since,
      until: bounds.until,
      sessions: groups.map(group => ({
        username: group.session.username,
        display: group.session.display,
        is_group: group.session.is_group,
        messages: group.messages.length,
      })),
      messages: total,
      characters,
      truncated: groups.some(group => group.truncated),
      agent_name: '',
      model: '',
      elapsed_ms: 0,
      output: null,
      insights: {
        focus: focus.slice(0, 80),
        timeline: [...timeline.entries()].map(([day, entry]) => ({
          day,
          messages: entry.messages,
          sessions: [...entry.sessions].slice(0, 3),
        })),
        related,
        terms,
      },
      note: null,
    };
  };

  const buildAgent = async (usernames: string[]): Promise<NewSummaryRecord> => {
    if (!accountId) throw new Error('还没有可用的微信账号。');
    const result = await invoke<AgentSummary>('agent_summarize', {
      accountId,
      usernames,
      messages: null,
      since: bounds.since,
      until: bounds.until,
    });
    return {
      kind: 'ai',
      account_id: accountId,
      range_label: bounds.label,
      since: bounds.since,
      until: bounds.until,
      sessions: usernames.map(username => {
        const item = sessions.find(candidate => candidate.username === username);
        return {
          username,
          display: item?.display ?? username,
          is_group: item?.is_group ?? false,
          messages: 0,
        };
      }),
      messages: result.messages,
      characters: result.characters,
      truncated: result.truncated,
      agent_name: result.agent_name,
      model: result.model,
      elapsed_ms: result.elapsed_ms,
      output: result.output,
      insights: null,
      note: null,
    };
  };

  const generate = useCallback(async () => {
    if (busy) return;
    if (!chatUsernames.length) {
      setError('请至少选择一个聊天。');
      return;
    }
    setBusy(true);
    setError('');
    setNotice('');
    setConfirmAi(false);
    try {
      const record = mode === 'ai'
        ? await buildAgent(chatUsernames)
        : await buildLocal(chatUsernames);
      await invoke<SummaryRecord[]>('summary_history_add', { record });
      await onReload();
      setOpen(false);
      setNotice(mode === 'ai' ? '已用本地 Agent 生成总结并保存。' : '已在本机整理并保存。');
      pageRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }, [busy, chatUsernames, mode, bounds.label, bounds.since, bounds.until, accountId, sessions, onReload]);

  const startGenerate = () => {
    if (mode === 'ai' && !confirmAi) {
      setConfirmAi(true);
      return;
    }
    void generate();
  };

  const remove = async (id: string) => {
    try {
      await invoke<SummaryRecord[]>('summary_history_delete', { id });
      setReader(null);
      await onReload();
      setNotice('已删除这条本机记录。');
    } catch (reason) {
      setError(String(reason));
    }
  };

  const openRaw = (record: SummaryRecord) => {
    const username = session?.username ?? record.sessions[0]?.username;
    if (username) onOpenRaw(username);
  };

  return <div className="chat-summary-page" ref={pageRef}>
    <div className="chat-summary-column">
      <div className="chat-summary-intro">
        <p>
          {scoped.length
            ? `${scoped.length} 份总结 · 只保存在本机，不参与事件分析`
            : '总结是某段时间或所选消息的快照，之后不会随事件分析变化。'}
        </p>
        <button
          type="button"
          className="chat-summary-cta"
          onClick={() => {
            setOpen(value => !value);
            setError('');
            setNotice('');
          }}
        >
          {open ? '收起' : '＋ 生成总结'}
        </button>
      </div>

      {open && <section className="chat-summary-panel" aria-label="生成聊天总结">
        <div className="chat-summary-row">
          <span className="chat-summary-label">时间</span>
          <div className="chat-summary-time">
            <div className="chat-summary-pills">
              {RANGES.map(item => <button
                key={item.id}
                type="button"
                className={range === item.id ? 'on' : ''}
                onClick={() => { setRange(item.id); setError(''); }}
              >{item.label}</button>)}
            </div>
            <span className="chat-summary-hint">{bounds.hint} · 最长一周，更长的范围请分多次</span>
          </div>
        </div>

        <div className="chat-summary-row">
          <span className="chat-summary-label">方式</span>
          <div className="chat-summary-time">
            <div className="chat-summary-pills">
              <button type="button" className={mode === 'local' ? 'on' : ''} onClick={() => { setMode('local'); setConfirmAi(false); }}>本地整理</button>
              <button
                type="button"
                className={mode === 'ai' ? 'on' : ''}
                onClick={() => { setMode('ai'); setError(''); }}
                disabled={!agentReady}
                title={agentReady ? `使用 ${agent?.agent_name}` : '先在「Agent 集成」页面配置并测试一个本地 Agent'}
              >AI 总结</button>
            </div>
            <span className="chat-summary-hint">
              {mode === 'ai'
                ? (agentReady ? `由 ${agent?.agent_name} 生成，原文可能离开本机` : '尚未配置可用的本地 Agent')
                : '规则归类，不联网，结果只保存在本机'}
            </span>
          </div>
        </div>

        <p className="chat-summary-note">
          将总结「{bounds.label}」的 {chatUsernames.length} 个聊天，结果保存到「总结记录」，最多保留最近 50 条。
        </p>

        {error && <p className="chat-summary-error">{error}</p>}

        {confirmAi && <div className="chat-summary-confirm">
          <span>原文会交给本机 Agent「{agent?.agent_name}」，可能发送到该 Agent 配置的模型服务。</span>
          <button type="button" onClick={() => setConfirmAi(false)}>取消</button>
          <button type="button" className="primary" onClick={() => void generate()}>确认生成</button>
        </div>}

        <div className="chat-summary-actions">
          <button type="button" onClick={() => { setOpen(false); setConfirmAi(false); setError(''); }}>取消</button>
          <button type="button" className="primary" onClick={startGenerate} disabled={busy || !chatUsernames.length}>
            {busy ? '生成中…' : '开始生成'}
          </button>
        </div>
      </section>}

      {notice && <p className="chat-summary-notice">{notice}</p>}
      {error && !open && <p className="chat-summary-error">{error}</p>}

      {busy && <article className="chat-summary-card doing" aria-live="polite">
        <header className="chat-summary-card-head">
          <strong>{bounds.label}</strong>
          <span className="chat-summary-kind">生成中</span>
          <span className="chat-summary-card-meta"><i className="chat-summary-spinner"/>正在{mode === 'ai' ? '交给本地 Agent' : '本机整理'}…</span>
        </header>
        <div className="chat-summary-skeleton"><i/><i/><i/></div>
      </article>}

      {!busy && scoped.map(record => {
        const content = cardContent(record);
        const sections = content.sections.slice(0, 4);
        return <article className="chat-summary-card" key={record.id}>
          <header className="chat-summary-card-head">
            <strong>{record.range_label || '聊天总结'}</strong>
            <span className="chat-summary-kind">{HISTORY_KIND_LABEL[record.kind] ?? record.kind}</span>
            {record.sessions.length > 1 && <span className="chat-summary-kind">历史跨会话总结 · {record.sessions.length} 个会话</span>}
            {record.agent_name && <span className="chat-summary-kind">{record.agent_name}{record.model ? ` · ${record.model}` : ''}</span>}
            <span className="chat-summary-card-meta">{record.messages} 条消息 · {record.created_at}</span>
          </header>
          {content.lead && <p className="chat-summary-lead">{content.lead}</p>}
          {sections.map(section => <section className="chat-summary-section" key={section.title}>
            <h4>{section.title}</h4>
            {section.items.map((item, index) => <div className="chat-summary-item" key={`${section.title}-${index}`}>
              <i>•</i>
              <span>
                {item.tag && <em className={`chat-summary-tag ${item.tag}`}>{item.tag}</em>}
                {item.text}
              </span>
            </div>)}
          </section>)}
          {content.sections.length > sections.length && <button type="button" className="chat-summary-more" onClick={() => setReader(record)}>
            还有 {content.sections.length - sections.length} 节，查看完整记录 →
          </button>}
          <footer className="chat-summary-card-foot">
            <button type="button" onClick={() => openRaw(record)}>查看原始消息 ↗</button>
            <span>依据 {record.messages} 条消息 · 约 {record.characters} 字{record.truncated ? ' · 已截断' : ''}</span>
            <span className="grow"/>
            <button type="button" className="primary" onClick={() => setReader(record)}>完整记录 →</button>
          </footer>
        </article>;
      })}

      {!busy && !scoped.length && <div className="chat-summary-empty">
        {query.trim()
          ? `没有匹配「${query.trim()}」的总结记录。`
          : summaryScopeEmpty(Boolean(session), summaries.length)}<br/>
        选好范围后在本机生成，结果只保存在本机。
      </div>}
    </div>

    {reader && <div
      className="chat-summary-modal"
      role="presentation"
      onMouseDown={event => { if (event.target === event.currentTarget) setReader(null); }}
    >
      <div className="chat-summary-modal-card" role="dialog" aria-modal="true" aria-label="完整总结记录">
        <SummaryDetail record={reader} onClose={() => setReader(null)} onDelete={() => void remove(reader.id)}/>
      </div>
    </div>}
  </div>;
}
