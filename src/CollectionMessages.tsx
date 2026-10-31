import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

export type MessageRef = { local_id: number; time: number };
type Message = MessageRef & { text: string; sender: string | null; sender_name: string | null; is_self: boolean };

export function CollectionMessages({ accountId, username, since, until, selected, onChange, busy }: {
  accountId: string; username: string; since: number | null; until: number | null;
  selected: MessageRef[]; onChange: (value: MessageRef[]) => void; busy: boolean;
}) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  useEffect(() => {
    let active = true;
    setLoading(true); setError(''); setMessages([]);
    void invoke<Message[]>('wechat_collection_messages', { accountId, username, since, until })
      .then(items => { if (active) setMessages(items); })
      .catch(e => { if (active) setError(String(e)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [accountId, username, since, until]);
  const chosen = (message: MessageRef) => selected.some(item => item.local_id === message.local_id && item.time === message.time);
  const toggle = (message: MessageRef) => onChange(chosen(message)
    ? selected.filter(item => item.local_id !== message.local_id || item.time !== message.time)
    : [...selected, { local_id: message.local_id, time: message.time }]);
  return <div className="collection-message-picker">
    <div className="collection-message-head"><span>逐条选择 · 已选 {selected.length} 条</span><button type="button" disabled={busy || loading || !messages.length} onClick={() => onChange(messages.map(({ local_id, time }) => ({ local_id, time })))}>全选</button><button type="button" disabled={busy || !selected.length} onClick={() => onChange([])}>清空</button></div>
    {loading && <p>正在读取所选日期内的消息…</p>}
    {error && <p className="collection-error" role="alert">{error}</p>}
    {!loading && !error && !messages.length && <p>此范围内没有消息。</p>}
    <div className="collection-message-list">{messages.map((message, index) => <label className="collection-message" key={`${message.local_id}-${message.time}-${index}`}>
      <input type="checkbox" checked={chosen(message)} disabled={busy} onChange={() => toggle(message)} />
      <span><small>{message.is_self ? '我' : message.sender_name || message.sender || '未知'} · {new Date(message.time * 1000).toLocaleString('zh-CN')}</small><span>{message.text}</span></span>
    </label>)}</div>
  </div>;
}
