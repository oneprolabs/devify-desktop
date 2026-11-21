import { t, tf, ctf } from '../../../shared/i18n/locale';
import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  HISTORY_KIND_LABEL,
  HISTORY_LIMIT,
  SummaryDetail,
  summarySourceTitle,
  type SummaryRecord,
} from '../../../shared/content/summary';

type KindFilter = 'all' | 'ai' | 'local' | 'daily';

const KIND_TABS: Array<{ id: KindFilter; label: string }> = [
  { id: 'all', label: '全部' },
  { id: 'ai', label: 'AI 总结' },
  { id: 'local', label: '本地整理' },
  { id: 'daily', label: '自动日报' },
];

export function HistoryPage() {
  const [records, setRecords] = useState<SummaryRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState('');
  const [kind, setKind] = useState<KindFilter>('all');
  const [account, setAccount] = useState('');
  const [query, setQuery] = useState('');
  const [activeId, setActiveId] = useState('');
  const [confirmDelete, setConfirmDelete] = useState(false);

  const load = async () => {
    setLoading(true);
    setError('');
    try {
      setRecords(await invoke<SummaryRecord[]>('summary_history_list'));
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const accounts = useMemo(
    () => Array.from(new Set(records.map(record => record.account_id).filter(Boolean))),
    [records],
  );

  const counts = useMemo(() => ({
    all: records.length,
    ai: records.filter(record => record.kind === 'ai').length,
    local: records.filter(record => record.kind === 'local').length,
    daily: records.filter(record => record.kind === 'daily').length,
  }), [records]);

  const filtered = useMemo(
    () => records.filter(record => (kind === 'all' || record.kind === kind) && (!account || record.account_id === account) && (!query.trim() || `${record.sessions.map(session => session.display).join(' ')} ${record.range_label ?? ''} ${record.output ?? ''} ${record.note ?? ''}`.toLowerCase().includes(query.trim().toLowerCase()))),
    [records, kind, account, query],
  );

  const active = filtered.find(record => record.id === activeId) ?? filtered[0] ?? null;

  useEffect(() => {
    setConfirmDelete(false);
  }, [active?.id]);

  const remove = async () => {
    if (!active) return;
    setDeleting(true);
    setError('');
    try {
      const next = await invoke<SummaryRecord[]>('summary_history_delete', { id: active.id });
      setRecords(next);
      setActiveId('');
      setConfirmDelete(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setDeleting(false);
    }
  };

  return <div className="settings-page history-page">
    <header className="page-head">
      <h2>{t("总结记录")}</h2>
      <p className="help">
        {t("查看保存在本机的聊天总结和整理结果，最多保留最近")} {HISTORY_LIMIT} {t("条。")}
      </p>
    </header>
    <section className="panel">
      {records.length > 0 && (
        <div className="history-toolbar">
          <div className="tabs" role="tablist" aria-label={t("按整理类型筛选")}>
            {KIND_TABS.map(item => (
              <button
                key={item.id}
                type="button"
                role="tab"
                aria-selected={kind === item.id}
                className={kind === item.id ? 'on' : ''}
                onClick={() => setKind(item.id)}
              >
                {t(item.label)} {counts[item.id]}
              </button>
            ))}
          </div>
          <div className="history-tools">
            <input type="search" className="history-search" aria-label={t("搜索总结记录")} placeholder={t("搜索总结记录")} value={query} onChange={event => setQuery(event.target.value)}/>
            {accounts.length > 1 && (
              <label className="history-account">
                {t("微信账号")}
                <select value={account} onChange={event => setAccount(event.target.value)}>
                  <option value="">{t("全部账号")}</option>
                  {accounts.map(item => <option key={item} value={item}>{item}</option>)}
                </select>
              </label>
            )}
            <button type="button" className="secondary" onClick={() => void load()} disabled={loading}>
              {loading ? t("读取中…") : t("刷新")}
            </button>
          </div>
        </div>
      )}

      {error && <div className="error">{t(error)}</div>}

      {loading ? (
        <p className="settings-empty">{t("正在读取本机总结记录…")}</p>
      ) : !records.length ? (
        <p className="settings-empty">
          {t("还没有保存过总结记录。在聊天记录里点「整理工作内容」生成后，会自动保存到本机并出现在这里。")}
        </p>
      ) : !filtered.length ? (
        <p className="settings-empty">{t("当前筛选下没有记录。")}</p>
      ) : (
        <div className="history-layout">
          <ul className="history-list" aria-label={t("总结记录列表")}>
            {filtered.map(record => (
              <li key={record.id}>
                <button
                  type="button"
                  className={`history-item ${active?.id === record.id ? 'on' : ''}`}
                  onClick={() => setActiveId(record.id)}
                >
                  <span className="history-item-top"><strong className="history-item-title">{summarySourceTitle(record)}</strong><span className="history-item-time">{record.created_at}</span></span>
                  <span className="history-item-meta">
                    <span className={`history-kind ${record.kind}`}>{HISTORY_KIND_LABEL[record.kind] ?? record.kind}</span>{' '}
                    {record.range_label}
                    {record.messages ? tf(" · {0} 条消息", record.messages) : ''}
                    {record.sessions.length > 1 ? tf(" · {0} 个会话", record.sessions.length) : ''}
                  </span>
                </button>
              </li>
            ))}
          </ul>

          <div className="history-detail-pane">
            {active && (
              <>
                {confirmDelete && (
                  <div className="history-confirm">
                    <p>
                      {t("确定删除「")}{summarySourceTitle(active)}{t("」这条记录？本机保存的内容会一并删除，微信聊天原文不受影响。")}
                    </p>
                    <div className="history-confirm-actions">
                      <button type="button" className="history-danger" onClick={() => void remove()} disabled={deleting}>
                        {deleting ? t("正在删除…") : t("确认删除")}
                      </button>
                      <button type="button" className="secondary" onClick={() => setConfirmDelete(false)} disabled={deleting}>
                        {t("取消")}
                      </button>
                    </div>
                  </div>
                )}
                <SummaryDetail
                  record={active}
                  onDelete={() => setConfirmDelete(true)}
                />
              </>
            )}
          </div>
        </div>
      )}
    </section>
  </div>;
}
