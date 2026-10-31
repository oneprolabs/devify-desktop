import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  HISTORY_KIND_LABEL,
  HISTORY_LIMIT,
  SummaryDetail,
  summarySourceTitle,
  type SummaryRecord,
} from './summary';

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
    () => records.filter(record => (kind === 'all' || record.kind === kind) && (!account || record.account_id === account)),
    [records, kind, account],
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
      <h2>总结记录</h2>
      <p className="help">
        本机保存的 AI 总结、本地整理和自动日报，最多保留最近 {HISTORY_LIMIT} 条。
        每条记录都标出整理时包含的聊天会话，点开可以查看当时的完整内容。
      </p>
    </header>
    <section className="panel">
      {records.length > 0 && (
        <div className="history-toolbar">
          <div className="tabs" role="tablist" aria-label="按整理类型筛选">
            {KIND_TABS.map(item => (
              <button
                key={item.id}
                type="button"
                role="tab"
                aria-selected={kind === item.id}
                className={kind === item.id ? 'on' : ''}
                onClick={() => setKind(item.id)}
              >
                {item.label} {counts[item.id]}
              </button>
            ))}
          </div>
          <div className="history-tools">
            {accounts.length > 1 && (
              <label className="history-account">
                微信账号
                <select value={account} onChange={event => setAccount(event.target.value)}>
                  <option value="">全部账号</option>
                  {accounts.map(item => <option key={item} value={item}>{item}</option>)}
                </select>
              </label>
            )}
            <button type="button" className="secondary" onClick={() => void load()} disabled={loading}>
              {loading ? '读取中…' : '刷新'}
            </button>
          </div>
        </div>
      )}

      {error && <div className="error">{error}</div>}

      {loading ? (
        <p className="settings-empty">正在读取本机总结记录…</p>
      ) : !records.length ? (
        <p className="settings-empty">
          还没有保存过总结记录。在聊天记录里点「整理工作内容」生成后，会自动保存到本机并出现在这里。
        </p>
      ) : !filtered.length ? (
        <p className="settings-empty">当前筛选下没有记录。</p>
      ) : (
        <div className="history-layout">
          <ul className="history-list" aria-label="总结记录列表">
            {filtered.map(record => (
              <li key={record.id}>
                <button
                  type="button"
                  className={`history-item ${active?.id === record.id ? 'on' : ''}`}
                  onClick={() => setActiveId(record.id)}
                >
                  <span className="history-item-top">
                    <span className={`history-kind ${record.kind}`}>
                      {HISTORY_KIND_LABEL[record.kind] ?? record.kind}
                    </span>
                    <span className="history-item-time">{record.created_at}</span>
                  </span>
                  <strong className="history-item-title">{summarySourceTitle(record)}</strong>
                  <span className="history-item-meta">
                    {record.range_label}
                    {record.messages ? ` · ${record.messages} 条消息` : ''}
                    {record.sessions.length > 1 ? ` · ${record.sessions.length} 个会话` : ''}
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
                      确定删除「{summarySourceTitle(active)}」这条记录？本机保存的内容会一并删除，微信聊天原文不受影响。
                    </p>
                    <div className="history-confirm-actions">
                      <button type="button" className="history-danger" onClick={() => void remove()} disabled={deleting}>
                        {deleting ? '正在删除…' : '确认删除'}
                      </button>
                      <button type="button" className="secondary" onClick={() => setConfirmDelete(false)} disabled={deleting}>
                        取消
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
