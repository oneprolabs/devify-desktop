import { t, tf, ctf, dateLocale, sortLocale } from '../../../shared/i18n/locale';
import { errorText } from '../../../shared/i18n/errorText';
import { useEffect, useMemo, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { CollectionMessages, type MessageRef } from './CollectionMessages';
import { AccountPicker } from '../../connectors/AccountPicker';
import wechatIcon from '../../../assets/connectors/wechat.svg';
import whatsappIcon from '../../../assets/connectors/whatsapp.svg';
import './collection.css';

type Account = { id: string; modified_at?: number | null; source?: string; display?: string | null };
type Session = { username: string; display: string; is_group: boolean; last_time: number | null };
type Settings = { active_account_id: string; devify: { active_space_id: string | null; base_url: string } };
type Space = { id: string; name: string };
type Preview = { fingerprint: string; sessions: Array<{ username: string; display: string; message_count: number }>; message_count: number };
function boundary(value: string, nextDay: boolean): number | null {
  if (!value) return null;
  const date = new Date(`${value}T00:00:00`);
  if (nextDay) date.setDate(date.getDate() + 1);
  return Math.floor(date.getTime() / 1000);
}
function localDate(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
}
function daysAgo(days: number): string { const date = new Date(); date.setDate(date.getDate() - days); return localDate(date); }
function activityLabel(seconds: number | null | undefined): string {
  if (!seconds) return t("无记录");
  const date = new Date(seconds * 1000);
  return date.toDateString() === new Date().toDateString()
    ? tf("今天 {0}", date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' }))
    : date.toLocaleDateString(dateLocale(), { month: 'numeric', day: 'numeric' });
}
export function Collection({ onSettings, onComplete }: { onSettings: () => void; onComplete: () => void }) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [accountId, setAccountId] = useState('');
  const [sessionLoading, setSessionLoading] = useState(false);
  const [sessionError, setSessionError] = useState('');
  const [sessions, setSessions] = useState<Session[]>([]);
  const [space, setSpace] = useState<Space | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [messageSelection, setMessageSelection] = useState<Record<string, MessageRef[]>>({});
  const [query, setQuery] = useState('');
  const [kind, setKind] = useState<'all' | 'group' | 'direct'>('all');
  const [since, setSince] = useState(() => daysAgo(6));
  const [rangeMode, setRangeMode] = useState<'1' | '7' | 'custom'>('7');
  const [until, setUntil] = useState(() => daysAgo(0));
  const [loading, setLoading] = useState(isTauri());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [preview, setPreview] = useState<Preview | null>(null);
  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    void (async () => {
      try {
        const [stored, found] = await Promise.all([invoke<Settings>('app_settings_get'), invoke<Account[]>('chat_accounts')]);
        if (!active) return;
        setSettings(stored);
        const accounts = found ?? [];
        setAccounts(accounts);
        const initial = accounts.find(item => item.id === stored.active_account_id) ?? [...accounts].sort((a, b) => (b.modified_at ?? 0) - (a.modified_at ?? 0))[0];
        setAccountId(initial?.id ?? '');
        if (stored.devify.active_space_id) {
          const spaces = await invoke<Space[]>('devify_spaces');
          if (active) setSpace(spaces.find(item => item.id === stored.devify.active_space_id) ?? null);
        }
      } catch (e) { if (active) setError(String(e)); }
      finally { if (active) setLoading(false); }
    })();
    return () => { active = false; };
  }, []);
  useEffect(() => {
    if (!accountId || !isTauri()) { setSessions([]); return; }
    let active = true;
    setSessionLoading(true); setSessionError(''); setSessions([]);
    void invoke<Session[]>('chat_sessions', { accountId })
      .then(list => { if (active) setSessions(list); })
      .catch(e => { if (active) setSessionError(String(e)); })
      .finally(() => { if (active) setSessionLoading(false); });
    return () => { active = false; };
  }, [accountId]);
  const switchAccount = (id: string) => {
    setSessionLoading(true); setSessions([]); setPreview(null); setSelected([]); setMessageSelection({}); setSessionError(''); setAccountId(id);
    // Multi-batch manual uploads only advance for the active account; switching the source
    // here keeps the queue, the chat page and the next batch on the same account.
    void invoke('chat_account_activate', { accountId: id }).catch(() => {});
  };
  const change = (id: string) => { setPreview(null); if (selected.includes(id)) setMessageSelection(current => { const next = { ...current }; delete next[id]; return next; }); setSelected(current => current.includes(id) ? current.filter(item => item !== id) : [...current, id]); };
  const setMode = (id: string, individual: boolean) => { setPreview(null); setMessageSelection(current => { const next = { ...current }; if (individual) next[id] = []; else delete next[id]; return next; }); };
  const changeDates = (start: string, end: string) => {
    if (start === since && end === until) return true;
    if (Object.values(messageSelection).some(items => items.length) && !window.confirm(t("更改日期范围会清空已逐条选择的消息，是否继续？"))) return false;
    setSince(start); setUntil(end); setPreview(null);
    setMessageSelection(current => Object.fromEntries(Object.keys(current).map(key => [key, []])));
    return true;
  };
  const recent = (days: number) => {
    const end = new Date(); const start = new Date(end); start.setDate(start.getDate() - days + 1);
    return changeDates(localDate(start), localDate(end));
  };
  const payload = () => ({ accountId, spaceId: settings?.devify.active_space_id, usernames: selected, since: boundary(since, false), until: boundary(until, true), messageSelection });
  const buildPreview = async () => {
    if (since && until && since > until) { setError(t("结束日期不能早于开始日期。")); return; }
    setBusy(true); setError(''); setPreview(null);
    try { setPreview(await invoke<Preview>('devify_upload_preview', payload())); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  const submit = async () => {
    if (!preview || busy) return;
    setBusy(true); setError('');
    try { await invoke('devify_upload_selected', { ...payload(), fingerprint: preview.fingerprint }); onComplete(); }
    catch (e) { setError(String(e)); setPreview(null); }
    finally { setBusy(false); }
  };
  const groupCount = useMemo(() => sessions.filter(session => session.is_group).length, [sessions]);
  // Filter first, then order by newest activity so the conversation that just received
  // a message always sits at the top of whatever list the user is looking at.
  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return sessions
      .filter(session => (kind === 'all' || (kind === 'group' ? session.is_group : !session.is_group))
        && (!needle || session.display.toLowerCase().includes(needle) || session.username.toLowerCase().includes(needle)))
      .sort((a, b) => (b.last_time ?? 0) - (a.last_time ?? 0) || a.display.localeCompare(b.display, sortLocale()));
  }, [sessions, query, kind]);
  const visibleSelected = useMemo(
    () => visible.reduce((count, session) => count + (selected.includes(session.username) ? 1 : 0), 0),
    [visible, selected],
  );
  // Bulk actions are scoped to the filtered list so hidden sessions keep their state.
  const bulkSelect = (mode: 'select' | 'clear' | 'invert') => {
    if (busy) return;
    const names = visible.map(session => session.username);
    const visibleNames = new Set(names);
    const next = mode === 'select'
      ? [...selected, ...names.filter(name => !selected.includes(name))]
      : mode === 'clear'
        ? selected.filter(name => !visibleNames.has(name))
        : [...selected.filter(name => !visibleNames.has(name)), ...names.filter(name => !selected.includes(name))];
    const removed = selected.filter(name => !next.includes(name));
    setPreview(null);
    setSelected(next);
    if (removed.length) setMessageSelection(current => {
      const copy = { ...current };
      removed.forEach(name => delete copy[name]);
      return copy;
    });
  };
  // The client never special-cases platform admins: an account that can see a space in
  // `devify_spaces` is a member of it, so upload permission is the server's call.
  const blocked = !space;
  const whatsappSource = accounts.find(account => account.id === accountId)?.source === 'whatsapp';
  const bulkDisabled = busy || blocked || !isTauri();
  return <section className="collection">
    <header className="task-detail-bar">{t("采集")} <span>/</span> {t("新建采集")}</header>
    <div className="collection-body">
      <h1>{t("选择要采集的聊天")}</h1>
      <p className="collection-intro">{t("从本机已读取的会话中挑选，确认范围后上传。")}</p>
      {!isTauri() && <p className="collection-notice">{t("请在桌面应用中选择本机聊天。")}</p>}
      {loading && <p className="collection-notice">{t("正在读取会话和上传空间…")}</p>}
      {error && <p className="collection-error" role="alert">{errorText(error)}</p>}
      {!loading && isTauri() && (!accountId || blocked) && <div className="collection-notice">{!accountId ? t("请先连接微信并选择账号。") : t("请先登录并保存可用的上传目标空间。")} <button type="button" onClick={onSettings}>{t("打开账户设置")}</button></div>}
      {accounts.length > 0 && <div className="collection-account"><label>{t("账号")}</label><AccountPicker accounts={accounts} value={accountId} disabled={busy} onChange={switchAccount} /><span>{t("每次采集仅包含当前账号的白名单会话。")}</span></div>}
      {whatsappSource && <p className="collection-notice">{t("WhatsApp 先上传文字内容；图片、语音和文件暂不打包，消息里会保留「[图片]」这类标签。")}</p>}
      {sessionError && <p className="collection-error" role="alert">{t("读取该账号会话失败：")}{sessionError}</p>}
      {sessionLoading && <p className="collection-notice">{t("正在读取所选账号的会话…")}</p>}
      <p className="collection-local-hint">{t("以下为本机记录，用于选择上传范围，不代表已上传。按最近活跃排序，最新消息在前。")}</p>
      <div className="collection-filter">
        <input type="search" value={query} onChange={event => setQuery(event.target.value)} placeholder={t("搜索会话名或 wxid")} aria-label={t("搜索会话")} disabled={busy} />
        <div className="collection-pills" role="group" aria-label={t("按会话类型筛选")}>
          <button type="button" className={kind === 'all' ? 'active' : ''} aria-pressed={kind === 'all'} onClick={() => setKind('all')}>{t("全部")} {sessions.length}</button>
          <button type="button" className={kind === 'group' ? 'active' : ''} aria-pressed={kind === 'group'} onClick={() => setKind('group')}>{t("群聊")} {groupCount}</button>
          <button type="button" className={kind === 'direct' ? 'active' : ''} aria-pressed={kind === 'direct'} onClick={() => setKind('direct')}>{t("单聊")} {sessions.length - groupCount}</button>
        </div>
      </div>
      <div className="collection-list">{visible.map(session => <div key={session.username} className={selected.includes(session.username) ? 'collection-session selected' : 'collection-session'}>
        <div className="collection-row"><label className="collection-session-label"><input type="checkbox" checked={selected.includes(session.username)} disabled={busy} onChange={() => change(session.username)} /><img className="collection-source-icon" src={whatsappSource ? whatsappIcon : wechatIcon} alt={whatsappSource ? 'WhatsApp' : t("微信")} /><span className="collection-name">{session.display}</span></label><small>{session.is_group ? t("群聊") : t("单聊")} · {activityLabel(session.last_time)}</small></div>
        {selected.includes(session.username) && <div className="collection-session-options"><div className="collection-range">
          <button type="button" className={rangeMode === '1' ? 'active' : ''} disabled={busy} onClick={() => { if (recent(1)) setRangeMode('1'); }}>{t("近 1 天")}</button>
          <button type="button" className={rangeMode === '7' ? 'active' : ''} disabled={busy} onClick={() => { if (recent(7)) setRangeMode('7'); }}>{t("近 7 天")}</button>
          <button type="button" className={rangeMode === 'custom' ? 'active' : ''} disabled={busy} onClick={() => setRangeMode('custom')}>{t("自定义日期")}</button>
          <button type="button" className={Object.prototype.hasOwnProperty.call(messageSelection, session.username) ? 'active' : ''} disabled={busy} onClick={() => setMode(session.username, !Object.prototype.hasOwnProperty.call(messageSelection, session.username))}>{t("逐条挑选")}</button>
          <span>{t("日期范围适用于所有已选会话")}</span>
        </div>{rangeMode === 'custom' && <div className="collection-custom-dates"><label>{t("开始日期")} <input type="date" value={since} disabled={busy} onChange={e => changeDates(e.target.value, until)} /></label><label>{t("结束日期")} <input type="date" value={until} disabled={busy} onChange={e => changeDates(since, e.target.value)} /></label></div>}</div>}
        {selected.includes(session.username) && Object.prototype.hasOwnProperty.call(messageSelection, session.username) && settings && <CollectionMessages accountId={accountId} username={session.username} since={boundary(since, false)} until={boundary(until, true)} selected={messageSelection[session.username]} busy={busy} onChange={items => { setPreview(null); setMessageSelection(current => ({ ...current, [session.username]: items })); }} />}
      </div>)}{!loading && !sessionLoading && !sessionError && sessions.length === 0 && <p className="collection-notice">{t("暂无可采集的白名单会话。")}</p>}{!sessionLoading && sessions.length > 0 && visible.length === 0 && <p className="collection-notice">{t("没有符合当前筛选条件的会话。")}</p>}</div>
      <div className="collection-bulk" role="group" aria-label={t("批量选择当前筛选结果")}>
        <span>{t("当前")} {visible.length} {t("个 · 已选")} {visibleSelected}</span>
        <button type="button" disabled={bulkDisabled || !visible.length} onClick={() => bulkSelect('select')}>{t("全选")}</button>
        <button type="button" disabled={bulkDisabled || !visible.length} onClick={() => bulkSelect('invert')}>{t("反选")}</button>
        <button type="button" disabled={bulkDisabled || !visibleSelected} onClick={() => bulkSelect('clear')}>{t("取消")}</button>
      </div>
      {preview && <div className="collection-review" role="region" aria-label={t("上传预览")}><h2>{t("确认上传范围")}</h2><p>{whatsappSource ? 'WhatsApp' : t("微信")}{t("账号")} {accountId} {t("· 目标空间")} {space?.name}</p><p>{t("时间：")}{since || t("不限起始")} {t("至")} {until || t("不限结束")} · {preview.sessions.length} {t("个会话 ·")} {preview.message_count} {t("条消息")}</p><ul>{preview.sessions.map(item => <li key={item.username}>{item.display} · {item.message_count} {t("条")}</li>)}</ul><p>{t("所选内容将上传至")} {settings?.devify.base_url}{t("，该空间的有效成员可查看。")}</p><button type="button" onClick={() => setPreview(null)} disabled={busy}>{t("返回修改")}</button> <button type="button" className="collection-primary" onClick={() => void submit()} disabled={busy}>{busy ? t("上传中…") : t("确认上传")}</button></div>}
    </div>
    <footer className="collection-footer"><span>{t("已选择")} {selected.length} {t("个会话")}{preview ? tf(" · {0} 条消息", preview.message_count) : ''}</span><button type="button" className="collection-primary" disabled={busy || loading || sessionLoading || blocked || !accountId || !selected.length || Object.values(messageSelection).some(items => !items.length) || !isTauri()} onClick={() => void buildPreview()}>{busy && !preview ? t("正在读取…") : t("预览上传范围 ↑")}</button></footer>
  </section>;
}
