import { t, tf } from './locale';
import { useState } from 'react';
type Session = { username: string; display: string; is_group: boolean };
export type HistoryRange = { since: number | null; until: number | null };
export type HistoryChoice = 'all' | '7' | '30' | '90' | 'custom';
export type HistoryScope = 'selected' | 'single';
const options: Array<[HistoryChoice, string]> = [['all', '全部历史'], ['7', '最近 7 天'], ['30', '最近 30 天'], ['90', '最近 90 天'], ['custom', '自定义']];
const emptyRange: HistoryRange = { since: null, until: null };

export function presetHistoryRange(choice: Exclude<HistoryChoice, 'custom'>, now = new Date()): HistoryRange {
  if (choice === 'all') return { ...emptyRange };
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  start.setDate(start.getDate() - (Number(choice) - 1));
  return { since: Math.floor(start.getTime() / 1000), until: null };
}

export function historyChoiceForRange(range: HistoryRange = emptyRange, now = new Date()): HistoryChoice {
  if (range.since == null && range.until == null) return 'all';
  if (range.until == null) {
    for (const choice of ['7', '30', '90'] as const) if (range.since === presetHistoryRange(choice, now).since) return choice;
  }
  return 'custom';
}

export function historyDateText(seconds: number | null | undefined, end = false): string {
  if (seconds == null) return '';
  const date = new Date(seconds * 1000);
  if (end) date.setDate(date.getDate() - 1);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
}

export function historyDateSeconds(text: string, end = false): number | null {
  if (!text) return null;
  const date = new Date(`${text}T00:00:00`);
  if (end) date.setDate(date.getDate() + 1);
  return Math.floor(date.getTime() / 1000);
}

type Props = {
  selected: Session[]; focused?: Session; synced: Set<string>; scope: HistoryScope;
  ranges: Record<string, HistoryRange>; choices: Record<string, HistoryChoice>; disabled: boolean;
  onScope: (scope: HistoryScope) => void; onFocus: (id: string) => void;
  onChange: (ids: string[], range: HistoryRange, choice: HistoryChoice) => void;
  dateDetails: Record<string, Array<[string, number]>>; dateLoading: string; onLoadDates: (id: string) => void;
};

function HistoryTargetList({ sessions, disabled, onChoose }: { sessions: Session[]; disabled: boolean; onChoose: (id: string) => void }) {
  const [page, setPage] = useState(0);
  const size = 12;
  const last = Math.max(0, Math.ceil(sessions.length / size) - 1);
  const current = Math.min(page, last);
  const start = current * size;
  return <div className="history-target-roster">
    <ul className="history-target-list" aria-label={t("已选会话名单")}>{sessions.slice(start, start + size).map(item => <li key={item.username}>
      <button type="button" disabled={disabled} onClick={() => onChoose(item.username)}><span>{item.display}</span><small>{item.is_group ? t("群聊") : t("私聊")}</small></button>
    </li>)}</ul>
    {last > 0 && <nav className="history-target-pager" aria-label={t("会话名单分页")}>
      <span>{tf("{0}–{1} / {2} 个会话", start + 1, Math.min(start + size, sessions.length), sessions.length)}</span>
      <button type="button" disabled={disabled || current === 0} aria-label={t("上一页会话名单")} onClick={() => setPage(current - 1)}><svg viewBox="0 0 20 20" aria-hidden="true"><path d="m12 5-5 5 5 5"/></svg></button>
      <button type="button" disabled={disabled || current === last} aria-label={t("下一页会话名单")} onClick={() => setPage(current + 1)}><svg viewBox="0 0 20 20" aria-hidden="true"><path d="m8 5 5 5-5 5"/></svg></button>
    </nav>}
  </div>;
}

export function ImportHistoryRange(p: Props) {
  const sessions = p.scope === 'selected' ? p.selected : p.focused && p.selected.some(item => item.username === p.focused?.username) ? [p.focused] : [];
  const targets = sessions.filter(item => !p.synced.has(item.username));
  const ids = targets.map(item => item.username);
  const first = p.ranges[ids[0]] ?? emptyRange;
  const commonRange = ids.length > 0 && ids.every(id => {
    const range = p.ranges[id] ?? emptyRange;
    return range.since === first.since && range.until === first.until;
  });
  const firstChoice = p.choices[ids[0]] ?? historyChoiceForRange(first);
  const choice = commonRange && ids.every(id => (p.choices[id] ?? historyChoiceForRange(p.ranges[id])) === firstChoice) ? firstChoice : 'mixed';
  const choose = (next: HistoryChoice) => p.onChange(ids, next === 'custom' ? commonRange ? { ...first } : { ...emptyRange } : presetHistoryRange(next), next);
  const setDate = (key: 'since' | 'until', text: string) => p.onChange(ids, { ...(commonRange ? first : emptyRange), [key]: historyDateSeconds(text, key === 'until') }, 'custom');
  const dateSession = targets.length === 1 ? targets[0].username : '';
  const dates = p.dateDetails[dateSession];
  return <section className="imports-detail import-history-range" aria-label={t("首次备份时间范围")}>
    <header>
      <div><h3>{t("首次备份的聊天时间范围")}</h3><p>{t("仅影响首次备份，后续继续同步新增消息。")}</p></div>
      <div className="wizard-scope-pills" role="group" aria-label={t("时间范围设置对象")}>
        <button type="button" disabled={p.disabled} aria-pressed={p.scope === 'selected'} onClick={() => p.onScope('selected')}>{t("已选")} {p.selected.length} {t("个会话")}</button>
        <button type="button" disabled={p.disabled || !p.selected.length} aria-pressed={p.scope === 'single'} onClick={() => { if (!p.selected.some(item => item.username === p.focused?.username)) p.onFocus(p.selected[0].username); p.onScope('single'); }}>{t("单个会话")}</button>
      </div>
    </header>
    {p.scope === 'selected' ? <div className="history-targets">
      {p.selected.length > 0 ? <details><summary><strong>{t("统一设置已选的")} {p.selected.length} {t("个会话")}</strong><span className="history-target-action">{t("查看会话名单")}<svg viewBox="0 0 20 20" aria-hidden="true"><path d="m7 4 6 6-6 6"/></svg></span></summary>
        <HistoryTargetList sessions={p.selected} disabled={p.disabled} onChoose={id => { p.onFocus(id); p.onScope('single'); }}/>
      </details> : <strong>{t("统一设置已选的")} 0 {t("个会话")}</strong>}
    </div> : <label className="history-session-select">{t("当前会话")}<select aria-label={t("选择要单独设置的会话")} value={p.selected.some(item => item.username === p.focused?.username) ? p.focused?.username : ''} disabled={p.disabled} onChange={event => p.onFocus(event.target.value)}>
      <option value="" disabled>{t("请选择已勾选的会话")}</option>{p.selected.map(item => <option key={item.username} value={item.username}>{item.display}</option>)}
    </select></label>}
    {sessions.length > targets.length && <p className="help history-progress-note"><svg viewBox="0 0 20 20" fill="none" aria-hidden="true"><circle cx="10" cy="10" r="7.25"/><path d="M10 9v5m0-8v.5"/></svg><span>{sessions.length - targets.length} {t("个会话已有备份进度，将继续同步新增消息，不重新上传历史记录。")}</span></p>}
    {!targets.length ? <p className="help">{!p.selected.length ? t("请先勾选要备份的会话。") : p.scope === 'single' && !sessions.length ? t("请先选择要设置的会话。") : t("这些会话已经开始备份，无需设置首次备份范围。")}</p> : <>
      <fieldset className="history-range-fields" disabled={p.disabled}>
        <div className="wizard-scope-pills history-range-options" role="group" aria-label={t("首次备份时间选项")}>{options.map(([value, label]) => <button key={value} type="button" aria-pressed={choice === value} onClick={() => choose(value)}>{t(label)}</button>)}</div>
        {choice === 'mixed' ? <p className="help">{t("当前会话使用不同的时间范围，选择一个选项可统一设置。")}</p> : choice === 'all' ? <p className="help">{t("备份这些会话在本机保存的全部历史记录。")}</p> : choice !== 'custom' ? <p className="help">{tf("从 {0} 起备份，时间范围按选择当天计算。", historyDateText(first.since))}</p> : <>
          <p className="help">{t("可只填写开始日期或结束日期。")}</p>
          <div className="imports-dates"><label>{t("开始日期")}<input type="date" value={historyDateText(first.since)} onChange={event => setDate('since', event.target.value)} /></label><span>{t("至")}</span><label>{t("结束日期")}<input type="date" value={historyDateText(first.until, true)} onChange={event => setDate('until', event.target.value)} /></label></div>
          {dateSession && <div className="imports-available"><strong>{t("有记录的日期")}</strong><button type="button" className="plan-button" disabled={!!p.dateLoading} onClick={() => p.onLoadDates(dateSession)}>{p.dateLoading === dateSession ? t("正在读取…") : t("查看日期")}</button>{dates && (dates.length ? <div className="imports-date-list" aria-label={t("选择有记录的开始日期")}>{dates.map(([day]) => <button type="button" key={day} aria-pressed={historyDateText(first.since) === day} onClick={() => setDate('since', day)}>{day}</button>)}</div> : <p>{t("没有可用记录")}</p>)}</div>}
        </>}
      </fieldset>
    </>}
  </section>;
}
