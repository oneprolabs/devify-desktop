import { t, dateLocale, sortLocale } from '../../../locale';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

export type MessageRef = { local_id: number; time: number; source_shard: string };
type Message = MessageRef & { text: string; sender: string | null; sender_name: string | null; is_self: boolean };

export function CollectionMessages({ accountId, username, since, until, selected, onChange, busy }: {
  accountId: string; username: string; since: number | null; until: number | null;
  selected: MessageRef[]; onChange: (value: MessageRef[]) => void; busy: boolean;
}) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [search, setSearch] = useState('');
  const [jumpDate, setJumpDate] = useState('');
  const listRef = useRef<HTMLDivElement>(null);
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
  const chosen = (message: MessageRef) => selected.some(item => item.local_id === message.local_id && item.time === message.time && item.source_shard === message.source_shard);
  const toggle = (message: MessageRef) => onChange(chosen(message)
    ? selected.filter(item => item.local_id !== message.local_id || item.time !== message.time || item.source_shard !== message.source_shard)
    : [...selected, { local_id: message.local_id, time: message.time, source_shard: message.source_shard }]);
  const visible = messages.filter(message => `${message.text} ${message.sender_name || message.sender || ''}`.toLowerCase().includes(search.trim().toLowerCase()));
  const selectVisible = () => onChange([...selected, ...visible.filter(message => !chosen(message)).map(({ local_id, time, source_shard }) => ({ local_id, time, source_shard }))]);
  const jump = (value: string) => {
    setJumpDate(value);
    if (!value || !visible.length) return;
    const target = new Date(`${value}T00:00:00`).getTime();
    const index = visible.reduce((best, message, i) => Math.abs(message.time * 1000 - target) < Math.abs(visible[best].time * 1000 - target) ? i : best, 0);
    const list = listRef.current;
    const row = list?.children.item(index) as HTMLElement | null;
    if (list && row) list.scrollTop = row.offsetTop;
  };
  return <div className="collection-message-picker">
    <div className="collection-message-head">
      <input type="search" aria-label={t("搜索消息内容")} placeholder={t("搜索消息内容")} value={search} onChange={event => setSearch(event.target.value)} />
      <label className="collection-jump">{t("跳转到日期")}<input type="date" aria-label={t("跳转到日期")} value={jumpDate} onChange={event => jump(event.target.value)} /></label>
      <span>{t("已选")} {selected.length} {t("条")}</span>
      <button type="button" disabled={busy || loading || !visible.length} onClick={selectVisible}>{t("全选当前结果")}</button>
      <button type="button" disabled={busy || !selected.length} onClick={() => onChange([])}>{t("清空")}</button>
    </div>
    {loading && <p>{t("正在读取所选日期内的消息…")}</p>}
    {error && <p className="collection-error" role="alert">{t(error)}</p>}
    {!loading && !error && !visible.length && <p>{messages.length ? t("没有匹配的消息。") : t("此范围内没有消息。")}</p>}
    <div className="collection-message-list" ref={listRef}>{visible.map((message, index) => <label className="collection-message" key={`${message.local_id}-${message.time}-${index}`}>
      <input type="checkbox" checked={chosen(message)} disabled={busy} onChange={() => toggle(message)} />
      <span><small>{message.is_self ? t("我") : message.sender_name || message.sender || t("未知")} · {new Date(message.time * 1000).toLocaleString(dateLocale())}</small><span>{message.text}</span></span>
    </label>)}</div>
  </div>;
}
