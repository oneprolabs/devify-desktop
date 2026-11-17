import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';

type Conversation = {
  id: number;
  title: string;
  platform: string;
  kind: string;
  source_conversation_id: string;
  message_count: number;
  last_message_at: string | null;
  last_message_preview: string;
  last_message_sender: string;
};
type CloudMessage = {
  id: number;
  conversation_id: number;
  sender_key: string;
  sender_name: string;
  direction: string;
  occurred_at: string | null;
  record_kind: string;
  preview: string;
};
type Page<T> = { list?: T[]; pagination?: { total?: number; page?: number; page_size?: number } };
type SyncResult = { status: string; uploaded: number; skipped: number; message_count: number; error?: string | null };

export type CloudFocus = { conversationId: number; messageId?: number };

const CONVERSATION_PAGE_SIZE = 25;
const MESSAGE_PAGE_SIZE = 100;

const platformLabel = (platform: string) => ({ wechat: '微信', whatsapp: 'WhatsApp', line: 'LINE' } as Record<string, string>)[platform] || platform;
const dateLabel = (value?: string | null) => value
  ? new Date(value).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
  : '时间未知';
const timeLabel = (value?: string | null) => value
  ? new Date(value).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit' })
  : '';
const pageCount = (total: number, size: number) => Math.max(1, Math.ceil(total / Math.max(1, size)));

export function CloudRecords({ spaceId, focus, onFocusHandled, onOpenSettings }: {
  spaceId: string | null;
  focus?: CloudFocus | null;
  onFocusHandled?: () => void;
  onOpenSettings?: () => void;
}) {
  const [query, setQuery] = useState('');
  const [debounced, setDebounced] = useState('');
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [conversationPage, setConversationPage] = useState(1);
  const [conversationTotal, setConversationTotal] = useState(0);
  const [conversationsLoading, setConversationsLoading] = useState(false);
  const [conversationsError, setConversationsError] = useState('');
  const [selected, setSelected] = useState<Conversation | null>(null);
  const [messages, setMessages] = useState<CloudMessage[]>([]);
  const [messagePage, setMessagePage] = useState(1);
  const [messageTotal, setMessageTotal] = useState(0);
  const [messagesLoading, setMessagesLoading] = useState(false);
  const [messagesError, setMessagesError] = useState('');
  const [syncing, setSyncing] = useState(false);
  const [notice, setNotice] = useState('');
  /** Message the reader should jump to; kept locally so the parent can clear its request. */
  const [anchor, setAnchor] = useState<CloudFocus | null>(null);
  const handledFocus = useRef('');

  useEffect(() => {
    const timer = window.setTimeout(() => setDebounced(query.trim()), 300);
    return () => window.clearTimeout(timer);
  }, [query]);

  const loadConversations = useCallback(async (page = 1) => {
    if (!isTauri() || !spaceId) return;
    setConversationsLoading(true); setConversationsError('');
    try {
      const result = await invoke<Page<Conversation>>('devify_conversations', { spaceId, search: debounced, page, pageSize: CONVERSATION_PAGE_SIZE });
      const list = result.list ?? [];
      setConversations(list);
      setConversationPage(result.pagination?.page ?? page);
      setConversationTotal(result.pagination?.total ?? list.length);
      setSelected(current => (current && list.some(item => item.id === current.id) ? current : list[0] ?? null));
    } catch (reason) {
      setConversations([]); setConversationTotal(0); setConversationsError(String(reason));
    } finally { setConversationsLoading(false); }
  }, [debounced, spaceId]);

  useEffect(() => { void loadConversations(1); }, [loadConversations]);

  const loadMessages = useCallback(async (conversation: Conversation, page?: number, focusMessageId?: number) => {
    if (!isTauri() || !spaceId) return;
    setMessagesLoading(true); setMessagesError('');
    try {
      let target = page;
      if (!target && !focusMessageId) {
        // Without a focus anchor the reader opens on the newest page.
        const probe = await invoke<Page<CloudMessage>>('devify_conversation_messages', { spaceId, conversationId: conversation.id, page: 1, pageSize: 1 });
        target = pageCount(probe.pagination?.total ?? 0, MESSAGE_PAGE_SIZE);
      }
      const result = await invoke<Page<CloudMessage>>('devify_conversation_messages', {
        spaceId, conversationId: conversation.id, page: target ?? 1, pageSize: MESSAGE_PAGE_SIZE, focusMessageId,
      });
      const list = result.list ?? [];
      setMessages(list);
      setMessagePage(result.pagination?.page ?? target ?? 1);
      setMessageTotal(result.pagination?.total ?? list.length);
    } catch (reason) {
      setMessages([]); setMessageTotal(0); setMessagesError(String(reason));
    } finally { setMessagesLoading(false); }
  }, [spaceId]);

  const focusKey = focus ? `${focus.conversationId}:${focus.messageId ?? ''}` : '';

  useEffect(() => {
    if (!focus) return;
    if (handledFocus.current === focusKey) return;
    const found = conversations.find(item => item.id === focus.conversationId);
    if (found) {
      handledFocus.current = focusKey;
      setAnchor({ conversationId: found.id, messageId: focus.messageId });
      setSelected(found);
      onFocusHandled?.();
      return;
    }
    if (conversationsLoading || !spaceId) return;
    // Evidence can point at a conversation outside the loaded page, so fetch it directly
    // instead of waiting for the user to page to it.
    handledFocus.current = focusKey;
    setAnchor({ conversationId: focus.conversationId, messageId: focus.messageId });
    void invoke<Conversation>('devify_conversation_detail', { spaceId, conversationId: focus.conversationId })
      .then(detail => setSelected(detail))
      .catch(() => setAnchor(null));
    onFocusHandled?.();
  }, [conversations, conversationsLoading, focus, focusKey, onFocusHandled, spaceId]);

  useEffect(() => {
    if (!selected) { setMessages([]); setMessageTotal(0); return; }
    const messageId = anchor?.conversationId === selected.id ? anchor.messageId : undefined;
    void loadMessages(selected, undefined, messageId);
  }, [selected, anchor, loadMessages]);

  useEffect(() => {
    const messageId = anchor?.messageId;
    if (!messageId || !messages.some(message => message.id === messageId)) return;
    document.getElementById(`cloud-message-${messageId}`)?.scrollIntoView({ block: 'center' });
  }, [messages, anchor]);

  const syncNow = async () => {
    setSyncing(true); setNotice('');
    try {
      const result = await invoke<SyncResult>('devify_sync_now');
      setNotice(result.error
        ? `同步未完成：${result.error}`
        : result.message_count
          ? `已上传 ${result.message_count} 条消息，等待解析`
          : '没有需要同步的新消息');
      await loadConversations(1);
    } catch (reason) {
      setNotice(String(reason).startsWith('SYNC_SCOPE_CONFIRM:') ? '请到聊天页确认当前空间的首次同步范围。' : String(reason));
    } finally { setSyncing(false); }
  };

  if (!isTauri()) return null;
  if (!spaceId) {
    return <div className="cloud-records cloud-records-blank">
      <h2>还没有选择空间</h2>
      <p>云端记录来自已上传的聊天。登录并选择空间后，这里会显示已保存的会话和消息。</p>
      {onOpenSettings && <button type="button" onClick={onOpenSettings}>打开账户设置</button>}
    </div>;
  }

  const conversationPages = pageCount(conversationTotal, CONVERSATION_PAGE_SIZE);
  const messagePages = pageCount(messageTotal, MESSAGE_PAGE_SIZE);

  return <div className="cloud-records">
    <aside className="cloud-records-sidebar">
      <header>
        <div><strong>云端记录</strong><small>{conversationTotal} 个会话</small></div>
        <button type="button" className="cloud-icon" onClick={() => void loadConversations(conversationPage)} title="刷新" aria-label="刷新云端记录">↻</button>
      </header>
      <input className="cloud-search" value={query} onChange={event => setQuery(event.target.value)} placeholder="搜索已上传的会话" />
      <div className="cloud-sync-bar">
        <span>{syncing ? '正在读取本机记录并上传…' : notice || '已上传的会话与消息'}</span>
        <button type="button" onClick={() => void syncNow()} disabled={syncing}>{syncing ? '同步中…' : '同步本机记录'}</button>
      </div>
      {conversationsError && <p className="cloud-error">{conversationsError}</p>}
      <div className="cloud-conversation-list">
        {conversations.map(conversation => <button
          type="button" key={conversation.id}
          className={selected?.id === conversation.id ? 'selected' : ''}
          onClick={() => { setAnchor(null); setSelected(conversation); }}
        >
          <span className="cloud-avatar">{conversation.title.slice(0, 1) || '#'}</span>
          <span className="cloud-conversation-body">
            <strong>{conversation.title || conversation.source_conversation_id}</strong>
            <small>{conversation.last_message_sender ? `${conversation.last_message_sender}：` : ''}{conversation.last_message_preview || '暂无消息预览'}</small>
            <em>{platformLabel(conversation.platform)} · {conversation.message_count} 条 · {dateLabel(conversation.last_message_at)}</em>
          </span>
        </button>)}
        {!conversations.length && !conversationsLoading && <div className="cloud-empty">
          <h3>还没有聊天记录</h3>
          <p>打开「自动化策略」启用自动同步，或在这里点「同步本机记录」上传白名单会话。</p>
        </div>}
      </div>
      {conversationPages > 1 && <footer className="cloud-pager">
        <button type="button" disabled={conversationPage <= 1} onClick={() => void loadConversations(conversationPage - 1)}>上一页</button>
        <span>{conversationPage} / {conversationPages}</span>
        <button type="button" disabled={conversationPage >= conversationPages} onClick={() => void loadConversations(conversationPage + 1)}>下一页</button>
      </footer>}
    </aside>

    <section className="cloud-reader">
      {selected ? <>
        <header className="cloud-reader-head">
          <div>
            <h2>{selected.title || selected.source_conversation_id}</h2>
            <p>{platformLabel(selected.platform)} · {selected.kind === 'group' ? '群聊' : '单聊'} · {selected.message_count} 条消息 · {selected.source_conversation_id}</p>
          </div>
          {messagePages > 1 && <div className="cloud-pager">
            <button type="button" disabled={messagePage <= 1} onClick={() => void loadMessages(selected, messagePage - 1)}>上一页</button>
            <span>{messagePage} / {messagePages}</span>
            <button type="button" disabled={messagePage >= messagePages} onClick={() => void loadMessages(selected, messagePage + 1)}>下一页</button>
          </div>}
        </header>
        {messagesError && <p className="cloud-error">{messagesError}</p>}
        <div className="cloud-message-list">
          {messages.map(message => <article
            key={message.id}
            id={`cloud-message-${message.id}`}
            className={[message.direction === 'outgoing' ? 'outgoing' : '', message.id === anchor?.messageId ? 'focused' : ''].filter(Boolean).join(' ')}
          >
            <header><strong>{message.sender_name || message.sender_key || '未知发送者'}</strong><time>{timeLabel(message.occurred_at)}</time></header>
            <p>{message.preview || '（空消息）'}</p>
          </article>)}
          {!messages.length && !messagesLoading && !messagesError && <div className="cloud-empty">
            <h3>这个会话还没有消息</h3>
            <p>解析完成后消息会出现在这里。</p>
          </div>}
          {messagesLoading && <p className="cloud-loading">正在读取消息…</p>}
        </div>
      </> : <div className="cloud-empty cloud-empty-main">
        <h3>选择一个会话</h3>
        <p>左侧列出已上传的会话，选择后可以在这里查看消息。</p>
      </div>}
    </section>
  </div>;
}
