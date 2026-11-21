import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { DailyTimePicker } from '../../shared/ui/DailyTimePicker';
import { analysisSettingsError } from './analysisSettingsText';
import { REPORT_PLANS, reportPlanSelection, type ReportDefinition } from './reportPlans';
import { t, tf } from '../../shared/i18n/locale';

function readPlan(value: unknown, definition: ReportDefinition) {
  try { return { ...reportPlanSelection(value, definition), error: '' }; }
  catch (reason) { return { plan: null, others: 0, error: analysisSettingsError(reason) }; }
}

export function ReportScheduleRow({ spaceId, plans, definition, onOpenTopics }: { spaceId: string; plans: unknown[]; definition: ReportDefinition; onOpenTopics: () => void }) {
  const [saved, setSaved] = useState(() => readPlan(plans, definition));
  const [draft, setDraft] = useState(saved.plan?.time_local?.slice(0, 5) || '08:30');
  const [action, setAction] = useState<'read' | 'save' | ''>('');
  const busy = Boolean(action);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const active = useRef(false);
  const lock = useRef(false);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  const plan = saved.plan;
  const enabled = Boolean(plan?.enabled && plan.time_local);
  const blocked = Boolean(error || saved.error);
  const apply = (value: unknown) => {
    const next = readPlan(value, definition);
    if (next.error) throw new Error(next.error);
    setSaved(next); setDraft(next.plan?.time_local?.slice(0, 5) || '08:30');
  };
  const reload = async () => {
    if (lock.current) return;
    lock.current = true; setAction('read'); setNotice('');
    try {
      const value = await invoke('devify_topics', { spaceId });
      if (active.current) { apply(value); setError(''); }
    } catch (reason) { if (active.current) setError(analysisSettingsError(reason)); }
    finally { lock.current = false; if (active.current) setAction(''); }
  };
  const change = async (patch: { enabled?: boolean; timeLocal?: string }) => {
    if (lock.current || blocked) return;
    lock.current = true; setAction('save'); setNotice('');
    try {
      // Re-read before creating: a prior uncertain response or another page may have created it.
      const current = plan ?? reportPlanSelection(await invoke('devify_topics', { spaceId }), definition).plan;
      if (!active.current) return;
      if (current) {
        await invoke('devify_report_plan_update', { spaceId, topicId: current.id, period: definition.period,
          enabled: patch.enabled ?? null, timeLocal: patch.timeLocal ?? (patch.enabled === true && !current.time_local ? draft : null) });
      } else if (patch.enabled === true) {
        await invoke('devify_topic_create', { spaceId, title: definition.title, instruction: t('汇总已备份内容中的进展、待办和风险。'),
          period: definition.period, timeLocal: draft, timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'Asia/Shanghai' });
      } else return;
      const value = await invoke('devify_topics', { spaceId });
      if (active.current) { apply(value); setError(''); setNotice(t('已保存')); }
    } catch (reason) { if (active.current) setError(analysisSettingsError(reason)); }
    finally { lock.current = false; if (active.current) setAction(''); }
  };
  return <div className="analysis-report-row" data-period={definition.period}>
    <div className="analysis-setting-row">
      <div><h3>{t(definition.label)}</h3><p>{t(definition.frequency)}</p></div>
      <button type="button" role="switch" aria-label={tf('自动{0}', t(definition.label))} aria-checked={enabled} aria-busy={busy} disabled={busy || blocked}
        className={`analysis-toggle${enabled ? ' on' : ''}`} onClick={() => void change({ enabled: !enabled })}><span /></button>
    </div>
    {enabled && <div className="analysis-setting-row analysis-time-row"><div><span>{t('生成时间')}</span><small>{tf('时区：{0}', plan?.timezone || Intl.DateTimeFormat().resolvedOptions().timeZone)}</small></div>
      <DailyTimePicker label={tf('{0}生成时间 {1}', t(definition.label), draft)} value={draft} onChange={setDraft}
        onConfirm={() => { if (draft !== plan?.time_local?.slice(0, 5)) void change({ timeLocal: draft }); }}
        onCancel={() => setDraft(plan?.time_local?.slice(0, 5) || '08:30')} disabled={busy || blocked}/>
    </div>}
    {(error || saved.error) && <p role="alert" className="analysis-report-error">{t('未能确认设置，当前显示上次读取的结果。')} {error || saved.error} <button type="button" disabled={busy} onClick={() => void reload()}>{t('重新读取')}</button> <button type="button" onClick={onOpenTopics}>{t('管理主题')}</button></p>}
    {plan?.last_error && <p className="analysis-report-note">{plan.last_error === 'No matching events or notes.' ? t('上次未生成报告：该时段暂无可用的笔记或事件。') : t('上次报告未能完成，请到报告页面查看。')}</p>}
    {saved.others > 0 && <p className="analysis-report-note">{tf('另有 {0} 个已开启的同周期报告，请在主题中核对。', saved.others)} <button type="button" onClick={onOpenTopics}>{t('管理主题')}</button></p>}
    <span role="status" aria-live="polite" className="analysis-report-saved">{busy ? action === 'read' ? t('正在读取…') : t('正在保存…') : notice}</span>
  </div>;
}

export function ReportSchedules({ spaceId, onOpenReports, onOpenTopics }: { spaceId: string; onOpenReports: () => void; onOpenTopics: () => void }) {
  const [plans, setPlans] = useState<unknown[] | null>(null);
  const [error, setError] = useState('');
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let current = true;
    setPlans(null); setError('');
    void invoke<unknown>('devify_topics', { spaceId }).then(value => {
      if (!Array.isArray(value)) throw new Error(t('未能读取报告计划，请重新读取。'));
      if (current) setPlans(value);
    }).catch(reason => { if (current) setError(analysisSettingsError(reason)); });
    return () => { current = false; };
  }, [spaceId, revision]);
  return <section className="analysis-report-settings" aria-label={t('定期报告')}>
    <div className="analysis-section-head"><div><h3>{t('定期报告')}</h3><p>{t('根据已生成的笔记与事件汇总。客户端关闭后仍会按计划生成。')}</p></div><button type="button" onClick={onOpenReports}>{t('查看报告')}</button></div>
    {!plans ? error ? <p role="alert" className="analysis-report-error">{error} <button type="button" onClick={() => setRevision(value => value + 1)}>{t('重新读取')}</button></p> : <p role="status" className="analysis-report-note">{t('正在读取报告计划…')}</p>
      : <div className="analysis-main-settings">{REPORT_PLANS.map(definition => <ReportScheduleRow key={definition.period} spaceId={spaceId} plans={plans} definition={definition} onOpenTopics={onOpenTopics}/>)}</div>}
    <p className="analysis-quiet-note">{t('按计划时间之后生成第一份报告；该时段暂无可用内容时跳过。')}</p>
  </section>;
}
