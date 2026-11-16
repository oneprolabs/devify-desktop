import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { invoke, isTauri } from '@tauri-apps/api/core';

/** Where a citation points. Mirrors the server's evidence targets so the same
 *  drawer serves answers, reports, topics and the chat event feed. */
export type EvidenceTarget = {
  kind: 'event' | 'note' | 'message';
  id: number;
  title?: string;
  excerpt?: string;
  conversation_id?: number | null;
};

type EvidenceRow = {
  id: number;
  message_id?: number;
  conversation_id?: number | null;
  sender_name?: string | null;
  occurred_at?: string | null;
  reference_key?: string | null;
};

type DrawerEvent = {
  id: number;
  title: string;
  content?: string | null;
  status?: string | null;
  tags?: Array<{ label?: string }>;
  last_activity_at?: string | null;
  evidence?: EvidenceRow[];
};

type DrawerNote = {
  id: number;
  title: string;
  content?: string | null;
  origin?: string | null;
  start_at?: string | null;
  created_at?: string | null;
  evidence?: EvidenceRow[];
  event_ids?: number[];
};

type DrawerMessage = {
  id: number;
  sender_name?: string | null;
  sender_key?: string | null;
  direction?: string | null;
  occurred_at?: string | null;
  preview?: string | null;
};

type Conversation = { id: number; title?: string | null; source_conversation_id?: string | null };
type Page<T> = { list?: T[]; pagination?: { total?: number; page?: number; page_size?: number; previous?: string | null } };

const KIND_LABEL: Record<EvidenceTarget['kind'], string> = { message: '原始对话', note: '整理信息', event: '事件' };
const STATUS_LABEL: Record<string, string> = { open: '未完成', in_progress: '进行中', done: '已完成' };

function stamp(value?: string | null): string {
  if (!value) return '—';
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime())
    ? value
    : parsed.toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
}
function clock(value?: string | null): string {
  if (!value) return '';
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
}
function statusLabel(value?: string | null): string {
  return STATUS_LABEL[value ?? ''] ?? '状态未设置';
}

export function EvidenceDrawer({ spaceId, target, onClose, onNavigate, onOpenDetail }: {
  spaceId: string | null;
  target: EvidenceTarget;
  onClose: () => void;
  onNavigate: (target: EvidenceTarget) => void;
  onOpenDetail?: (target: EvidenceTarget) => void;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const closeButton = useRef<HTMLButtonElement>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [event, setEvent] = useState<DrawerEvent | null>(null);
  const [notes, setNotes] = useState<DrawerNote[]>([]);
  const [note, setNote] = useState<DrawerNote | null>(null);
  const [conversation, setConversation] = useState<Conversation | null>(null);
  const [messages, setMessages] = useState<DrawerMessage[]>([]);
  const [page, setPage] = useState(1);
  const [loadingMore, setLoadingMore] = useState(false);
  const [expanded, setExpanded] = useState(false);

  const loadThread = useCallback(async (conversationId: number, focusMessageId?: number) => {
    if (!spaceId) return;
    const result = await invoke<Page<DrawerMessage>>('devify_conversation_messages', {
      spaceId,
      conversationId,
      page: 1,
      pageSize: 100,
      focusMessageId,
    });
    const list = result.list ?? [];
    setMessages(list);
    setPage(result.pagination?.page ?? 1);
  }, [spaceId]);

  useEffect(() => {
    let active = true;
    setError('');
    setLoading(false);
    setEvent(null);
    setNotes([]);
    setNote(null);
    setConversation(null);
    setMessages([]);
    setPage(1);
    setExpanded(false);
    if (!spaceId || !isTauri()) return;

    if (target.kind === 'event') {
      setLoading(true);
      void Promise.all([
        invoke<DrawerEvent>('devify_analysis_detail', { kind: 'events', spaceId, itemId: target.id }),
        invoke<DrawerNote[]>('devify_event_notes', { spaceId, eventId: target.id }).catch(() => []),
      ])
        .then(([detail, list]) => {
          if (!active) return;
          setEvent(detail);
          setNotes((list ?? []).filter(Boolean));
        })
        .catch(reason => { if (active) setError(String(reason)); })
        .finally(() => { if (active) setLoading(false); });
      return () => { active = false; };
    }

    if (target.kind === 'note') {
      setLoading(true);
      void invoke<DrawerNote>('devify_note_detail', { spaceId, noteId: target.id })
        .then(detail => { if (active) setNote(detail); })
        .catch(reason => { if (active) setError(String(reason)); })
        .finally(() => { if (active) setLoading(false); });
      return () => { active = false; };
    }

    const conversationId = target.conversation_id ?? null;
    if (!conversationId) return;
    setLoading(true);
    void invoke<Conversation>('devify_conversation_detail', { spaceId, conversationId })
      .then(detail => { if (active) setConversation(detail); })
      .catch(() => { /* the thread still renders without the conversation title */ });
    void loadThread(conversationId, target.id)
      .catch(reason => { if (active) setError(String(reason)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [spaceId, target.kind, target.id, target.conversation_id, loadThread]);

  useEffect(() => {
    const trigger = document.activeElement as HTMLElement | null;
    closeButton.current?.focus();
    return () => { trigger?.focus?.(); };
  }, []);
  useEffect(() => {
    const onKeydown = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose(); };
    window.addEventListener('keydown', onKeydown);
    return () => window.removeEventListener('keydown', onKeydown);
  }, [onClose]);

  // The drawer body is the scroll container; wait for the frame so the cited
  // message lands in the middle instead of at the top of the list.
  useEffect(() => {
    if (target.kind !== 'message' || !messages.length) return;
    const frame = window.requestAnimationFrame(() => {
      const element = document.getElementById(`evidence-message-${target.id}`);
      const scroller = panel.current?.querySelector('.evidence-body');
      if (!element || !scroller) return;
      const delta = element.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
      scroller.scrollTop += delta - scroller.clientHeight / 2 + element.clientHeight / 2;
    });
    return () => window.cancelAnimationFrame(frame);
  }, [target.kind, target.id, messages]);

  const loadEarlier = async () => {
    const conversationId = target.conversation_id ?? null;
    if (!spaceId || !conversationId || loadingMore || page <= 1) return;
    setLoadingMore(true);
    try {
      const previous = Math.max(1, page - 1);
      const result = await invoke<Page<DrawerMessage>>('devify_conversation_messages', {
        spaceId,
        conversationId,
        page: previous,
        pageSize: 100,
      });
      setMessages(current => [...(result.list ?? []), ...current]);
      setPage(result.pagination?.page ?? previous);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoadingMore(false);
    }
  };

  const openMessage = (row: EvidenceRow) => {
    if (!row.message_id) return;
    onNavigate({ kind: 'message', id: row.message_id, conversation_id: row.conversation_id ?? null, title: row.sender_name ?? undefined });
  };

  const title = target.kind === 'message'
    ? conversation?.title || conversation?.source_conversation_id || target.title || '原始对话'
    : event?.title || note?.title || target.title || KIND_LABEL[target.kind];
  const subtitle = target.kind === 'message' && messages.length ? '引用消息已高亮' : '';
  const longNote = (note?.content ?? '').length > 600;

  return createPortal(<aside className="evidence-drawer" role="dialog" aria-modal="true" aria-label="查看依据">
    <div className="evidence-scrim" onClick={onClose}/>
    <div className="evidence-panel" ref={panel} tabIndex={-1}>
      <header className="evidence-head">
        <div className="evidence-head-main">
          <span className="evidence-kind">{KIND_LABEL[target.kind]}</span>
          <h2 className="evidence-title">{title}</h2>
          {subtitle && <p className="evidence-sub">{subtitle}</p>}
        </div>
        <button ref={closeButton} type="button" className="evidence-close" aria-label="关闭依据" onClick={onClose}>
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true">
            <path d="M6 18L18 6M6 6l12 12" strokeLinecap="round"/>
          </svg>
        </button>
      </header>

      <div className="evidence-body">
        {loading && <p className="evidence-hint" aria-busy="true">正在加载依据…</p>}
        {!loading && error && <p className="evidence-hint bad" role="alert">{error}</p>}

        {!loading && !error && target.kind === 'message' && !target.conversation_id &&
          <p className="evidence-hint bad" role="alert">这条消息缺少所属会话，无法展示原文。</p>}

        {!loading && !error && target.kind === 'message' && target.conversation_id && <>
          {page > 1 && <button type="button" className="evidence-more" disabled={loadingMore} onClick={() => void loadEarlier()}>
            {loadingMore ? '加载中…' : '加载更早的消息'}
          </button>}
          {!messages.length && <p className="evidence-hint">这个会话还没有已同步的消息。</p>}
          {messages.map(message => <article
            key={message.id}
            id={`evidence-message-${message.id}`}
            className={`evidence-msg${message.id === target.id ? ' focus' : ''}${message.direction === 'outgoing' ? ' out' : ''}`}
          >
            <div className="evidence-msg-meta">
              <span className="evidence-sender">{message.sender_name || message.sender_key || '未知发送者'}</span>
              {message.occurred_at && <time dateTime={message.occurred_at}>{clock(message.occurred_at)}</time>}
              {message.id === target.id && <span className="evidence-flag">引用</span>}
            </div>
            <p className="evidence-bubble">{message.preview || '（无正文）'}</p>
          </article>)}
        </>}

        {!loading && !error && target.kind === 'note' && note && <>
          <div className="evidence-meta">
            <span className="evidence-tag">{note.origin === 'manual' ? '人工整理' : '自动整理'}</span>
            {(note.start_at || note.created_at) && <span>{stamp(note.start_at || note.created_at)}</span>}
          </div>
          <p className={`evidence-text${!expanded && longNote ? ' clamped' : ''}`}>{note.content || '这条整理没有正文。'}</p>
          {longNote && <button type="button" className="evidence-link" onClick={() => setExpanded(value => !value)}>{expanded ? '收起全文' : '展开全文'}</button>}

          <section className="evidence-section">
            <div className="evidence-section-title">原文证据<span className="evidence-count">{note.evidence?.length ?? 0}</span></div>
            {!note.evidence?.length
              ? <p className="evidence-hint">这条整理还没有原文证据。</p>
              : <div className="evidence-list">{note.evidence.map(row => <button
                key={row.id}
                type="button"
                className="evidence-row"
                onClick={() => openMessage(row)}
              >
                <span className="evidence-row-main">
                  <span className="evidence-row-title">#{row.message_id}{row.sender_name ? ` · ${row.sender_name}` : ''}</span>
                  <span className="evidence-row-sub">{row.reference_key || '原始消息'}</span>
                </span>
                <span className="evidence-row-action">查看原文 →</span>
              </button>)}</div>}
          </section>

          <section className="evidence-section">
            <div className="evidence-section-title">关联事件<span className="evidence-count">{note.event_ids?.length ?? 0}</span></div>
            {!note.event_ids?.length
              ? <p className="evidence-hint">这条整理还没有关联事件。</p>
              : <div className="evidence-list">{note.event_ids.map(id => <button
                key={id}
                type="button"
                className="evidence-row"
                onClick={() => onNavigate({ kind: 'event', id })}
              >
                <span className="evidence-row-main"><span className="evidence-row-title">事件 #{id}</span></span>
                <span className="evidence-row-action">查看事件 →</span>
              </button>)}</div>}
          </section>
        </>}

        {!loading && !error && target.kind === 'event' && event && <>
          <div className="evidence-meta">
            <span className={`evidence-tag ${event.status === 'done' ? 'done' : event.status === 'in_progress' ? 'rel' : ''}`}>{statusLabel(event.status)}</span>
            {(event.tags ?? []).map((tag, index) => tag.label ? <span className="evidence-tag ac" key={`${tag.label}-${index}`}>{tag.label}</span> : null)}
            {event.last_activity_at && <span>{stamp(event.last_activity_at)}</span>}
          </div>
          <p className="evidence-text">{event.content || '暂无情况描述。'}</p>

          <section className="evidence-section">
            <div className="evidence-section-title">整理结论<span className="evidence-count">{notes.length}</span></div>
            {!notes.length
              ? <p className="evidence-hint">这个事件还没有关联的整理信息。</p>
              : <div className="evidence-list">{notes.map(item => <button
                key={item.id}
                type="button"
                className="evidence-row"
                onClick={() => onNavigate({ kind: 'note', id: item.id, title: item.title })}
              >
                <span className="evidence-row-main">
                  <span className="evidence-row-title">{item.title}</span>
                  <span className="evidence-row-sub">{stamp(item.start_at || item.created_at)}</span>
                </span>
                <span className="evidence-row-action">查看整理 →</span>
              </button>)}</div>}
          </section>

          <section className="evidence-section">
            <div className="evidence-section-title">原文证据<span className="evidence-count">{event.evidence?.length ?? 0}</span></div>
            {!event.evidence?.length
              ? <p className="evidence-hint">这个事件还没有原文证据。</p>
              : <div className="evidence-list">{event.evidence.map(row => <button
                key={row.id}
                type="button"
                className="evidence-row"
                onClick={() => openMessage(row)}
              >
                <span className="evidence-row-main">
                  <span className="evidence-row-title">{stamp(row.occurred_at)}{row.sender_name ? ` · ${row.sender_name}` : ''}</span>
                  <span className="evidence-row-sub">{row.reference_key || '原始消息'}</span>
                </span>
                <span className="evidence-row-action">查看原文 →</span>
              </button>)}</div>}
          </section>
        </>}
      </div>

      <footer className="evidence-foot">
        <span className="evidence-foot-hint">
          {target.kind === 'message' ? '只读预览 · 展示引用消息前后的对话' : target.kind === 'note' ? '只读预览 · 整理过的记录' : '只读预览 · 编辑请到详情页'}
        </span>
        {target.kind === 'event' && onOpenDetail && <button
          type="button"
          className="evidence-btn"
          onClick={() => { onOpenDetail(target); onClose(); }}
        >打开事件详情</button>}
      </footer>
    </div>
  </aside>, document.body);
}
