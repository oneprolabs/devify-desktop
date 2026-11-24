import { t, tf } from '../../../shared/i18n/locale';
import type { MessageItem, SessionItem } from './nativeTypes';

export function toDate(seconds: number | null): Date | null {
  if (!seconds) return null;
  const date = new Date(seconds > 1e12 ? seconds : seconds * 1000);
  return Number.isNaN(date.getTime()) ? null : date;
}

export function pad(value: number): string {
  return value < 10 ? `0${value}` : String(value);
}

export function dayKey(date: Date): string {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

export function dayLabel(date: Date): string {
  const today = new Date();
  const yesterday = new Date(today.getTime() - 86400000);
  const time = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  if (dayKey(date) === dayKey(today)) return tf("今天 {0}", time);
  if (dayKey(date) === dayKey(yesterday)) return tf("昨天 {0}", time);
  if (date.getFullYear() === today.getFullYear()) return tf("{0}月{1}日 {2}", date.getMonth() + 1, date.getDate(), time);
  return tf("{0}年{1}月{2}日 {3}", date.getFullYear(), date.getMonth() + 1, date.getDate(), time);
}

export function timeOnly(seconds: number | null): string {
  const date = toDate(seconds);
  return date ? `${pad(date.getHours())}:${pad(date.getMinutes())}` : '';
}

/** WeChat-style session list timestamp: time for today, date otherwise. */
export function listTime(seconds: number | null): string {
  const date = toDate(seconds);
  if (!date) return '';
  const today = new Date();
  if (dayKey(date) === dayKey(today)) return timeOnly(seconds);
  const yesterday = new Date(today.getTime() - 86400000);
  if (dayKey(date) === dayKey(yesterday)) return t("昨天");
  if (date.getFullYear() === today.getFullYear()) return `${date.getMonth() + 1}/${date.getDate()}`;
  return `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())}`;
}

export function avatarColor(seed: string): string {
  let hash = 0;
  for (let index = 0; index < seed.length; index += 1) {
    hash = (hash * 31 + seed.charCodeAt(index)) % 360;
  }
  return `hsl(${hash} 38% 52%)`;
}

export function initial(name: string): string {
  const trimmed = name.trim();
  return trimmed ? Array.from(trimmed)[0] : '?';
}

export function messageAvatar(
  message: Pick<MessageItem, 'source_shard' | 'is_self' | 'sender' | 'sender_name' | 'text'>,
  session: Pick<SessionItem, 'username' | 'display' | 'is_group'> | null,
  accountId: string,
  accountDisplay?: string | null,
) {
  if (message.source_shard === 'whatsapp') {
    if (message.is_self) {
      // Native account discovery uses the phone number when no profile name is available.
      const display = accountDisplay?.trim();
      const nickname = display && display !== accountId.split('@')[0] ? display : t("我");
      return { username: accountId, label: initial(nickname), color: avatarColor(accountId || 'whatsapp:self') };
    }
    if (session && !session.is_group) {
      return {
        username: session.username,
        label: initial(session.display.trim() || message.sender_name || '?'),
        color: avatarColor(session.username),
      };
    }
    if (session?.is_group && (!message.sender || message.sender.endsWith('@g.us'))) {
      const name = message.sender_name?.trim() || t("未知成员");
      return { username: null, label: initial(name), color: avatarColor('whatsapp:unknown-member') };
    }
  }
  return {
    username: message.sender,
    label: initial(message.sender_name || message.sender || (message.is_self ? t("我") : '?')),
    color: avatarColor(message.sender ?? message.text),
  };
}

export function Highlight({ text, keyword }: { text: string; keyword: string }) {
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
