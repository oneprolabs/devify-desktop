import { useEffect, useRef, useState } from 'react';
import { desktopInvoke as invoke } from './desktopInvoke';
import { t } from './locale';
import { WizardActionButton } from './WizardActionButton';

type Part = { id: number; kind: string; text?: string; upload_item_id?: number | null; media_url?: string };
type Message = { id: number; sender_name: string; sender_key: string; preview: string; occurred_at: string; parts?: Part[] };
type Page = { list?: Message[]; pagination?: { page: number; total: number; page_size: number } };
function LazyImage({ spaceId, item, mediaUrl }: { spaceId: string; item?: number | null; mediaUrl?: string }) {
  const node = useRef<HTMLSpanElement>(null);
  const [src, setSrc] = useState('');
  const [error, setError] = useState('');
  useEffect(() => {
    if (!node.current) return;
    let active = true;
    const root = node.current.closest('.wxv-messages') ?? node.current.closest('.remote-chat-fallback');
    const observer = new IntersectionObserver(entries => {
      if (!entries.some(entry => entry.isIntersecting)) return;
      observer.disconnect();
      void invoke<string>('chat_remote_image', { spaceId, itemId: item, sha256: mediaUrl?.match(/\/chat-media\/([a-fA-F0-9]{64})$/)?.[1] }).then(value => { if (active) setSrc(value); }).catch(reason => { if (active) setError(String(reason)); });
    }, { root, rootMargin: `${(root as HTMLElement | null)?.clientHeight ?? 0}px 0px` });
    observer.observe(node.current);
    return () => { active = false; observer.disconnect(); };
  }, [spaceId, item, mediaUrl]);
  return <span ref={node} className="remote-chat-image">{src ? <img src={src} alt={t('图片')} /> : error || t('图片待加载')}</span>;
}

/** Activated only after the native reader confirms that this selected chat is missing. */
export function RemoteChatFallback({ accountId, username }: { accountId: string; username: string }) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [space, setSpace] = useState('');
  const [conversation, setConversation] = useState<number | null>(null);
  const [page, setPage] = useState(1);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState('');
  const request = useRef(0);
  const load = async (target?: number) => {
    const id = ++request.current; setBusy(true); setError('');
    try {
      let spaceId = space; let conversationId = conversation;
      if (!spaceId || !conversationId) {
        // These reads are independent; the lookup itself is authenticated in native code.
        const [settings, found] = await Promise.all([
          invoke<{ devify: { active_space_id?: string } }>('app_settings_get'),
          invoke<{ conversation: { id: number } | null }>('chat_remote_lookup', { accountId, username }),
        ]);
        if (id !== request.current) return;
        spaceId = settings.devify.active_space_id || '';
        conversationId = found.conversation?.id ?? null;
        setSpace(spaceId); setConversation(conversationId);
        if (!conversationId) { setMessages([]); return; }
      }
      const data = await invoke<Page>('devify_conversation_messages', { spaceId, conversationId, page: target || 1, latest: !target, pageSize: 100 });
      if (id !== request.current) return;
      setMessages(data.list ?? []); setPage(data.pagination?.page ?? 1);
    } catch (reason) { if (id === request.current) setError(String(reason)); }
    finally { if (id === request.current) setBusy(false); }
  };
  useEffect(() => { void load(); return () => { request.current += 1; }; }, [accountId, username]);
  const detail = async (message: Message) => {
    const id = request.current;
    const found = await invoke<Message>('devify_message_detail', { spaceId: space, conversationId: conversation, messageId: message.id });
    if (id === request.current) setMessages(current => current.map(row => row.id === message.id ? { ...row, parts: found.parts ?? [] } : row));
  };
  return <section className="remote-chat-fallback"><p className="imports-hint">{t('本地没有这段记录，正在查看云端备份。')}</p>
    {busy && <p role="status">{t('正在读取云端备份…')}</p>}
    {error && <div role="alert"><p>{error}</p><WizardActionButton onAction={() => load()}>{t('重试')}</WizardActionButton></div>}
    {!busy && !error && !messages.length && <p>{t('未找到可访问的云端备份。可以检查当前空间与访问权限。')}</p>}
    {!busy && messages.map(message => <article className="remote-chat-message" key={message.id}><small>{message.sender_name || message.sender_key} · {new Date(message.occurred_at).toLocaleString()}</small>
      {message.parts ? message.parts.map(part => <div key={part.id}>{part.text && <p>{part.text}</p>}{part.kind === 'image' && (part.upload_item_id || part.media_url) && <LazyImage spaceId={space} item={part.upload_item_id} mediaUrl={part.media_url} />}</div>) : <><p>{message.preview}</p><WizardActionButton className="connector-quiet" onAction={() => detail(message)}>{t('查看完整消息')}</WizardActionButton></>}
    </article>)}
    {!busy && page > 1 && <WizardActionButton onAction={() => load(page - 1)}>{t('更早的记录')}</WizardActionButton>}
  </section>;
}
