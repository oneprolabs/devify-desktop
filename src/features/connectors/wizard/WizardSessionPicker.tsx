import { t, tf } from '../../../shared/i18n/locale';
import { useEffect, useState } from 'react';
import wechatIcon from '../../../assets/connectors/wechat.svg';
import whatsappIcon from '../../../assets/connectors/whatsapp.svg';

type Session = { username: string; display: string; is_group: boolean; last_time: number | null };
type Props = {
  items: Session[]; total: number; groupCount: number; selected: Set<string>; selectedCount: number;
  kind: 'all' | 'group' | 'direct'; onKind: (kind: 'all' | 'group' | 'direct') => void;
  search: string; onSearch: (text: string) => void; loading: boolean; disabled: boolean; accountId: string;
  onToggle: (id: string) => void; onBulk: (mode: 'select' | 'clear' | 'invert', ids?: string[]) => void;
  focused: string; onFocus: (id: string) => void; activity: (time: number | null) => string;
  sourceName?: string;
};

/** V7's compact table; all selection and date persistence stays in WhitelistSettings. */
export function WizardSessionPicker(p: Props) {
  const [page, setPage] = useState(0);
  const [size, setSize] = useState(20);
  useEffect(() => { setPage(0); }, [p.search, p.kind, p.accountId, size]);
  const pages = Math.max(1, Math.ceil(p.items.length / size));
  const current = Math.min(page, pages - 1);
  const rows = p.items.slice(current * size, (current + 1) * size);
  const visibleSelected = p.items.filter(item => p.selected.has(item.username)).length;
  const pageSelected = rows.filter(item => p.selected.has(item.username)).length;
  const pageNumbers = [...new Set([0, current - 1, current, current + 1, pages - 1].filter(n => n >= 0 && n < pages))].sort((a, b) => a - b);
  return <fieldset className="wizard-session-picker" disabled={p.disabled}>
    <div className="wizard-session-tools">
      <div className="plan-pills" role="group" aria-label={t("按会话类型筛选")}>
        {([['all', t("全部"), p.total], ['direct', t("私聊"), p.total - p.groupCount], ['group', t("群聊"), p.groupCount]] as const).map(([kind, label, n]) => <button key={kind} type="button" aria-pressed={p.kind === kind} className={p.kind === kind ? 'on' : ''} onClick={() => p.onKind(kind)}>{t(label)} {n}</button>)}
      </div>
      <input type="search" value={p.search} onChange={event => p.onSearch(event.target.value)} placeholder={t("搜索会话名称")} aria-label={t("搜索会话")} />
    </div>
    <div className="wizard-session-selection" role="group" aria-label={t("批量选择当前筛选结果")}>
      <span>{t("共")}{p.items.length} {t("个会话 · 当前已选")}{visibleSelected} {t("个")}</span>
      <button type="button" disabled={!p.items.length || p.loading} onClick={() => p.onBulk('select')}>{t("全选")}</button>
      <button type="button" disabled={!visibleSelected || p.loading} onClick={() => p.onBulk('clear')}>{t("全不选")}</button>
      <button type="button" disabled={!p.items.length || p.loading} onClick={() => p.onBulk('invert')}>{t("反选")}</button>
    </div>
    <div className="wizard-session-table" role="table" aria-label={t("选择要备份的会话")} aria-busy={p.loading}>
      <div className="wizard-session-row wizard-session-table-head" role="row">
        <span role="columnheader"><input type="checkbox" aria-label={t("选择本页全部会话")} checked={rows.length > 0 && pageSelected === rows.length} ref={input => { if (input) input.indeterminate = pageSelected > 0 && pageSelected < rows.length; }} disabled={!rows.length || p.loading} onChange={() => p.onBulk(pageSelected === rows.length ? 'clear' : 'select', rows.map(item => item.username))} /></span>
        <span role="columnheader">{t("会话")}</span><span role="columnheader">{t("最近活跃")}</span>
      </div>
      {rows.map(item => <div className="wizard-session-row" role="row" key={item.username}>
        <span role="cell"><input type="checkbox" aria-label={`导入 ${item.display}`} checked={p.selected.has(item.username)} onChange={() => p.onToggle(item.username)} /></span>
        <div role="cell" className="wizard-session-name"><button type="button" title={t("设置此会话的首次备份时间范围")} onClick={() => { p.onFocus(item.username); }}><img className="wizard-session-badge" src={p.sourceName === 'WhatsApp' ? whatsappIcon : wechatIcon} alt="" /><strong>{item.display}</strong></button><small>{item.is_group ? t("群聊") : t("私聊")}{p.selected.has(item.username) ? t(" · 已选择") : ''}</small></div>
        <time role="cell">{p.activity(item.last_time)}</time>
      </div>)}
      {!p.accountId ? <p className="plan-empty">{t("未发现")}{p.sourceName ?? "微信"}{t("账号，请返回连接器完成连接。")}</p> : p.loading ? <p className="plan-empty" role="status">{t("正在读取会话列表…")}</p> : !rows.length && <p className="plan-empty">{t("没有符合条件的会话。")}</p>}
    </div>
    <div className="wizard-session-pagination">
      <span>{p.items.length ? `${current * size + 1}–${Math.min((current + 1) * size, p.items.length)}` : '0'} / {p.items.length} {t("个 · 已选")}{p.selectedCount}</span>
      <nav aria-label={t("会话分页")}><button type="button" aria-label={t("上一页")} disabled={current === 0} onClick={() => setPage(current - 1)}>‹</button>{pageNumbers.map((n, i) => <span key={n}>{i > 0 && n - pageNumbers[i - 1] > 1 && <span className="wizard-page-ellipsis">…</span>}<button type="button" aria-label={`第 ${n + 1} 页`} aria-current={current === n ? 'page' : undefined} onClick={() => setPage(n)}>{n + 1}</button></span>)}<button type="button" aria-label={t("下一页")} disabled={current >= pages - 1} onClick={() => setPage(current + 1)}>›</button></nav>
      <div className="wizard-session-size"><span>{t("每页")}</span>{[20, 50, 100].map(n => <button type="button" key={n} aria-pressed={size === n} onClick={() => setSize(n)}>{n}</button>)}</div>
    </div>
  </fieldset>;
}
