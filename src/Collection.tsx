import { useEffect, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { CollectionMessages, type MessageRef } from './CollectionMessages';

type Account = { id: string; modified_at: number | null };
type Scan = { accounts: Account[] };
type Session = { username: string; display: string; is_group: boolean; last_time: number | null };
type Settings = { active_account_id: string; devify: { active_space_id: number | null; base_url: string } };
type Space = { id: number; name: string; is_global_admin: boolean; admin_takeover_active: boolean };
type Preview = { fingerprint: string; sessions: Array<{ username: string; display: string; message_count: number }>; message_count: number };
function boundary(value: string, nextDay: boolean): number | null {
  if (!value) return null;
  const date = new Date(`${value}T00:00:00`);
  if (nextDay) date.setDate(date.getDate() + 1);
  return Math.floor(date.getTime() / 1000);
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
  const [since, setSince] = useState('');
  const [until, setUntil] = useState('');
  const [search, setSearch] = useState('');
  const [loading, setLoading] = useState(isTauri());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [preview, setPreview] = useState<Preview | null>(null);
  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    void (async () => {
      try {
        const [stored, scan] = await Promise.all([invoke<Settings>('app_settings_get'), invoke<Scan>('wechat_scan')]);
        if (!active) return;
        setSettings(stored);
        const found = scan.accounts ?? [];
        setAccounts(found);
        const initial = found.find(item => item.id === stored.active_account_id) ?? [...found].sort((a, b) => (b.modified_at ?? 0) - (a.modified_at ?? 0))[0];
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
    void invoke<Session[]>('wechat_sessions', { accountId })
      .then(list => { if (active) setSessions(list); })
      .catch(e => { if (active) setSessionError(String(e)); })
      .finally(() => { if (active) setSessionLoading(false); });
    return () => { active = false; };
  }, [accountId]);
  const switchAccount = (id: string) => {
    setSessionLoading(true); setSessions([]); setPreview(null); setSelected([]); setMessageSelection({}); setSearch(''); setSessionError(''); setAccountId(id);
  };
  const change = (id: string) => { setPreview(null); if (selected.includes(id)) setMessageSelection(current => { const next = { ...current }; delete next[id]; return next; }); setSelected(current => current.includes(id) ? current.filter(item => item !== id) : [...current, id]); };
  const setMode = (id: string, individual: boolean) => { setPreview(null); setMessageSelection(current => { const next = { ...current }; if (individual) next[id] = []; else delete next[id]; return next; }); };
  const resetMessageSelection = () => setMessageSelection(current => Object.fromEntries(Object.keys(current).map(key => [key, []])));
  const payload = () => ({ accountId, spaceId: settings?.devify.active_space_id, usernames: selected, since: boundary(since, false), until: boundary(until, true), messageSelection });
  const buildPreview = async () => {
    if (since && until && since > until) { setError('结束日期不能早于开始日期。'); return; }
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
  const visible = sessions.filter(session => `${session.display} ${session.username}`.toLowerCase().includes(search.trim().toLowerCase()));
  const blocked = !space || (space.is_global_admin && !space.admin_takeover_active);
  return <section className="collection">
    <header className="task-detail-bar">采集 <span>/</span> 新建采集</header>
    <div className="collection-body">
      <h1>选择要采集的聊天</h1>
      <p className="collection-intro">从本机已读取的会话中挑选，确认范围后上传。</p>
      {!isTauri() && <p className="collection-notice">请在桌面应用中选择本机聊天。</p>}
      {loading && <p className="collection-notice">正在读取会话和上传空间…</p>}
      {error && <p className="collection-error" role="alert">{error}</p>}
      {!loading && isTauri() && (!accountId || blocked) && <div className="collection-notice">{!accountId ? '请先连接微信并选择账号。' : space?.is_global_admin ? '请先在账户设置中接管目标空间。' : '请先登录并保存可用的上传目标空间。'} <button type="button" onClick={onSettings}>打开账户设置</button></div>}
      {accounts.length > 0 && <div className="collection-account"><label>微信账号<select value={accountId} disabled={busy} onChange={event => switchAccount(event.target.value)}>{accounts.map(account => <option key={account.id} value={account.id}>{account.id}</option>)}</select></label><span>每次采集仅包含当前账号的白名单会话。</span></div>}
      {sessionError && <p className="collection-error" role="alert">读取该账号会话失败：{sessionError}</p>}
      {sessionLoading && <p className="collection-notice">正在读取所选账号的会话…</p>}
      <div className="collection-filters"><label>搜索会话<input type="search" value={search} disabled={busy} onChange={e => setSearch(e.target.value)} placeholder="搜索会话" /></label><label>开始日期<input type="date" value={since} disabled={busy} onChange={e => { setSince(e.target.value); setPreview(null); resetMessageSelection(); }} /></label><label>结束日期<input type="date" value={until} disabled={busy} onChange={e => { setUntil(e.target.value); setPreview(null); resetMessageSelection(); }} /></label></div>
      <div className="collection-list">{visible.map(session => <div key={session.username} className="collection-session">
        <div className="collection-row"><label className="collection-session-label"><input type="checkbox" checked={selected.includes(session.username)} disabled={busy} onChange={() => change(session.username)} /><span className="collection-source">微信</span><span className="collection-name">{session.display}</span></label><small>{session.is_group ? '群聊' : '单聊'}</small>{selected.includes(session.username) && <button type="button" className="collection-mode" disabled={busy} onClick={() => setMode(session.username, !Object.prototype.hasOwnProperty.call(messageSelection, session.username))}>{Object.prototype.hasOwnProperty.call(messageSelection, session.username) ? '改为整段' : '逐条选择'}</button>}</div>
        {selected.includes(session.username) && Object.prototype.hasOwnProperty.call(messageSelection, session.username) && settings && <CollectionMessages accountId={accountId} username={session.username} since={boundary(since, false)} until={boundary(until, true)} selected={messageSelection[session.username]} busy={busy} onChange={items => { setPreview(null); setMessageSelection(current => ({ ...current, [session.username]: items })); }} />}
      </div>)}{!loading && !sessionLoading && !sessionError && sessions.length === 0 && <p className="collection-notice">暂无可采集的白名单会话。</p>}{sessions.length > 0 && !visible.length && <p className="collection-notice">没有匹配的会话。</p>}</div>
      {preview && <div className="collection-review" role="region" aria-label="上传预览"><h2>确认上传范围</h2><p>微信账号 {accountId} · 目标空间 {space?.name}</p><p>时间：{since || '不限起始'} 至 {until || '不限结束'} · {preview.sessions.length} 个会话 · {preview.message_count} 条消息</p><ul>{preview.sessions.map(item => <li key={item.username}>{item.display} · {item.message_count} 条</li>)}</ul><p>所选内容将上传至 {settings?.devify.base_url}，该空间的有效成员可查看。</p><button type="button" onClick={() => setPreview(null)} disabled={busy}>返回修改</button> <button type="button" className="collection-primary" onClick={() => void submit()} disabled={busy}>{busy ? '上传中…' : '确认上传'}</button></div>}
    </div>
    <footer className="collection-footer"><span>已选择 {selected.length} 个会话{preview ? ` · ${preview.message_count} 条消息` : ''}</span><button type="button" className="collection-primary" disabled={busy || loading || sessionLoading || blocked || !accountId || !selected.length || Object.values(messageSelection).some(items => !items.length) || !isTauri()} onClick={() => void buildPreview()}>{busy && !preview ? '正在读取…' : '预览上传范围 ↑'}</button></footer>
  </section>;
}
