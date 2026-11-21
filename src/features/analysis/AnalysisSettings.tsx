import { t } from '../../shared/i18n/locale';
import { ReportSchedules } from './ReportSchedules';
import { AgentSettings } from '../agents/AgentSettings';
import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { type AppSettings, defaultSettings, mergeSettings } from '../settings/settingsModel';
import { analysisSettingsError } from './analysisSettingsText';
import { WizardActionButton } from '../../shared/ui/WizardActionButton';
import './analysis-settings.css';

type AnalysisPreferences = {
  event_granularity: 'fine' | 'balanced' | 'coarse';
  summary_length: 'short' | 'standard' | 'detailed';
  summary_style: 'concise' | 'narrative' | 'actions';
  conversation_types: Array<'direct' | 'group'>;
  lookback_days: 0 | 30 | 90;
  auto_analyze: boolean;
};

function validPreferences(value: AnalysisPreferences): boolean {
  return Boolean(value && Array.isArray(value.conversation_types) && value.conversation_types.length
    && value.conversation_types.every(kind => kind === 'direct' || kind === 'group')
    && [0, 30, 90].includes(value.lookback_days) && typeof value.auto_analyze === 'boolean'
    && ['fine', 'balanced', 'coarse'].includes(value.event_granularity)
    && ['short', 'standard', 'detailed'].includes(value.summary_length)
    && ['concise', 'narrative', 'actions'].includes(value.summary_style));
}

export function AnalysisSettings({ onOpenReports, onOpenTopics, onOpenChats, canUseLocalSummary = false }: { onOpenReports: () => void; onOpenTopics: () => void; onOpenChats: () => void; canUseLocalSummary?: boolean }) {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [ready, setReady] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [preferences, setPreferences] = useState<AnalysisPreferences | null>(null);
  const [confirmed, setConfirmed] = useState<AnalysisPreferences | null>(null);
  const [preferencesError, setPreferencesError] = useState('');
  const [preferencesNotice, setPreferencesNotice] = useState('');
  const [preferencesSaving, setPreferencesSaving] = useState(false);
  const [preferencesRefresh, setPreferencesRefresh] = useState(0);
  const [advanced, setAdvanced] = useState(false);
  const [localSummaryOpen, setLocalSummaryOpen] = useState(false);
  const active = useRef(false);
  const loadId = useRef(0);
  const preferencesLock = useRef(false);
  const spaceId = settings.devify.active_space_id;

  const load = useCallback(async () => {
    const request = ++loadId.current;
    setLoading(true); setError('');
    try {
      const stored = await invoke<AppSettings>('app_settings_get');
      if (!active.current || request !== loadId.current) return;
      setSettings(mergeSettings(stored)); setReady(true);
    } catch (reason) {
      if (active.current && request === loadId.current) setError(analysisSettingsError(reason));
    } finally {
      if (active.current && request === loadId.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    active.current = true;
    void load();
    return () => { active.current = false; loadId.current += 1; };
  }, [load]);

  useEffect(() => {
    let current = true;
    setPreferences(null); setConfirmed(null); setPreferencesNotice(''); setPreferencesError('');
    if (!ready || !spaceId) return;
    void invoke<AnalysisPreferences>('devify_analysis_preferences_get', { spaceId }).then(value => {
      if (!current) return;
      if (!validPreferences(value)) throw new Error(t('当前服务端不支持完整的分析范围设置，请先更新服务端。'));
      setPreferences(value); setConfirmed(value);
    }).catch(reason => { if (current) setPreferencesError(analysisSettingsError(reason)); });
    return () => { current = false; };
  }, [ready, spaceId, preferencesRefresh]);

  const changePreferences = (next: AnalysisPreferences) => { setPreferencesNotice(''); setPreferences(next); };
  const savePreferences = async () => {
    if (!spaceId || !preferences || preferencesLock.current) return;
    preferencesLock.current = true;
    const request = loadId.current;
    setPreferencesSaving(true); setPreferencesError(''); setPreferencesNotice('');
    try {
      const saved = await invoke<AnalysisPreferences>('devify_analysis_preferences_save', { spaceId, preferences });
      if (!active.current || request !== loadId.current) return;
      if (!validPreferences(saved)) throw new Error(t('未能确认分析设置，请重试。'));
      setPreferences(saved); setConfirmed(saved); setPreferencesNotice(t('已保存'));
    } catch (reason) {
      if (active.current && request === loadId.current) setPreferencesError(analysisSettingsError(reason));
    } finally {
      preferencesLock.current = false;
      if (active.current && request === loadId.current) setPreferencesSaving(false);
    }
  };

  const dirty = Boolean(preferences && JSON.stringify(preferences) !== JSON.stringify(confirmed));
  const kinds = preferences?.conversation_types ?? [];
  const rangeSummary = kinds.length === 2 ? t('私聊和群聊') : kinds.includes('group') ? t('群聊') : t('私聊');
  const lengthSummary = preferences ? t({ short: '简短', standard: '标准', detailed: '详细' }[preferences.summary_length]) : '';

  if (!ready) return <div className="settings-page analysis-settings analysis-simple"><h2>{t('智能分析')}</h2><p>{loading ? t('正在加载设置…') : t('设置尚未加载，无法安全保存。')}</p>{error && <p role="alert" className="error">{error}</p>}{!loading && <button type="button" onClick={() => void load()}>{t('重新加载')}</button>}</div>;

  return <div className="settings-page analysis-settings analysis-simple">
    <header className="page-head"><h2>{t('智能分析')}</h2><p className="help">{t('按功能管理聊天总结、聊天事件分析和定期报告。')}</p></header>
    {!spaceId ? <p role="alert">{t('请先在账户中选择可用空间。')}</p> : <>
      <section className="analysis-main-settings analysis-summary-settings" aria-label={t('聊天总结设置')}>
        <div className="analysis-setting-row"><div><h3>{t('聊天总结')}</h3><p>{t('在聊天中选择时间或消息，按需生成独立总结。')}</p></div><button type="button" className="analysis-feature-link" onClick={onOpenChats}>{t('前往聊天')}</button></div>
        {canUseLocalSummary && <>
          <button type="button" className="analysis-advanced-trigger" aria-expanded={localSummaryOpen} aria-controls="analysis-local-summary" onClick={() => setLocalSummaryOpen(value => !value)}>
            <span><strong>{t('本地总结设置')}</strong><small>{t('本地工具、模型和补充要求')}</small></span>
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d={localSummaryOpen ? 'm4 10 4-4 4 4' : 'm4 6 4 4 4-4'}/></svg>
          </button>
          {localSummaryOpen && <div id="analysis-local-summary" className="analysis-local-summary"><AgentSettings summaryOnly /></div>}
        </>}
      </section>
      <section className="analysis-main-settings" aria-label={t('事件与笔记设置')}>
        <div className="analysis-setting-row">
          <div><h3>{t('聊天事件分析')}</h3><p>{t('自动将已备份的聊天整理为笔记和事件。')}</p></div>
        </div>
      <section className="analysis-advanced">
        <button type="button" className="analysis-advanced-trigger" aria-label={t('调整事件与笔记设置')} aria-expanded={advanced} aria-controls="analysis-advanced-options" disabled={!preferences} onClick={() => setAdvanced(value => !value)}>
          <span><strong>{t('调整事件与笔记设置')}</strong><small>{preferences ? `${rangeSummary} · ${lengthSummary}` : t('正在读取设置…')}{dirty && ` · ${t('有未保存的修改')}`}</small></span>
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d={advanced ? 'm4 10 4-4 4 4' : 'm4 6 4 4 4-4'}/></svg>
        </button>
        {advanced && preferences && <div id="analysis-advanced-options">
          <fieldset disabled={preferencesSaving} className="analysis-preference-fields">
            <p className="analysis-preference-help">{t('以下选项用于事件提取和笔记整理。聊天总结、报告和主题使用各自的生成规则。')}</p>
            <div className="analysis-preference-row"><span>{t('参与分析的聊天类型')}</span><div className="analysis-options" role="group" aria-label={t('聊天记录类型')}>{([{ value: 'direct', label: '私聊' }, { value: 'group', label: '群聊' }] as const).map(option => <label key={option.value}><input type="checkbox" checked={kinds.includes(option.value)} disabled={kinds.length === 1 && kinds.includes(option.value)} onChange={() => changePreferences({ ...preferences, conversation_types: kinds.includes(option.value) ? kinds.filter(value => value !== option.value) : [...kinds, option.value] })}/>{t(option.label)}</label>)}</div></div>
            <div className="analysis-preference-row"><span>{t('事件划分粒度')}</span><div className="analysis-options" role="radiogroup" aria-label={t('事件划分粒度')}>{([{ value: 'fine', label: '更细（更多事件）' }, { value: 'balanced', label: '平衡' }, { value: 'coarse', label: '更粗（更少事件）' }] as const).map(option => <label key={option.value}><input type="radio" name="event-granularity" checked={preferences.event_granularity === option.value} onChange={() => changePreferences({ ...preferences, event_granularity: option.value })}/>{t(option.label)}</label>)}</div></div>
            <label className="analysis-preference-row"><span>{t('整理风格')}</span><select value={preferences.summary_style} onChange={event => changePreferences({ ...preferences, summary_style: event.target.value as AnalysisPreferences['summary_style'] })}><option value="concise">{t('简洁概述')}</option><option value="narrative">{t('背景与经过')}</option><option value="actions">{t('行动与后续')}</option></select></label>
            <div className="analysis-preference-row"><span>{t('内容详略')}</span><div className="analysis-options" role="radiogroup" aria-label={t('内容详略')}>{([{ value: 'short', label: '简短' }, { value: 'standard', label: '标准' }, { value: 'detailed', label: '详细' }] as const).map(option => <label key={option.value}><input type="radio" name="summary-length" checked={preferences.summary_length === option.value} onChange={() => changePreferences({ ...preferences, summary_length: option.value })}/>{t(option.label)}</label>)}</div></div>
          </fieldset>
          <div className="analysis-preference-actions"><p>{t('用于之后新建的事件分析任务，已有任务和结果保持不变。')}</p><WizardActionButton className="primary" disabled={!dirty || preferencesSaving} loading={preferencesSaving} busyLabel={t('正在保存…')} onAction={savePreferences}>{t('保存事件与笔记设置')}</WizardActionButton></div>
        </div>}
      </section>
      <div className="analysis-preference-feedback" role="status" aria-live="polite">{preferencesNotice}</div>
      {preferencesError && <p className="error" role="alert">{preferencesError} {!preferences && <button type="button" onClick={() => setPreferencesRefresh(value => value + 1)}>{t('重试')}</button>}</p>}
      </section>
      <ReportSchedules key={spaceId} spaceId={spaceId} onOpenReports={onOpenReports} onOpenTopics={onOpenTopics}/>
      <section className="analysis-feature-entry" aria-label={t('主题订阅')}>
        <div className="analysis-section-head"><div><h3>{t('主题订阅')}</h3><p>{t('关注内容、生成周期和时间在每个主题中分别设置。')}</p></div><button type="button" onClick={onOpenTopics}>{t('管理主题')}</button></div>
      </section>
    </>}
  </div>;
}
