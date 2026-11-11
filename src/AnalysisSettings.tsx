import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { type AppSettings, defaultSettings, mergeSettings } from './Settings';
import { analysisSettingsError } from './analysisSettingsText';
import { createSettingsSaveQueue, settingsSaveFailure, settingsUserRevision, type SettingsSaveQueue } from './settingsSaveQueue';

const DAILY_STATUS_LABEL: Record<string, string> = {
  scheduled: '服务端已计划',
  disabled: '已关闭',
  sent: '已投递',
  no_messages: '当天无新消息',
  busy: '账号正忙，本次跳过',
  error: '未完成',
};

type Row = { label: string; desc?: string; control?: ReactNode; value?: string };
type AnalysisPreferences = { event_granularity: 'fine' | 'balanced' | 'coarse'; summary_length: 'short' | 'standard' | 'detailed'; summary_style: 'concise' | 'narrative' | 'actions'; conversation_types: Array<'direct' | 'group'>; lookback_days: 0 | 30 | 90; auto_analyze: boolean };

type Task = {
  id: string;
  name: string;
  desc: string;
  summary: string;
  control?: { checked: boolean; onChange: (next: boolean) => void; label: string };
  badge?: string;
  rows: Row[];
};

function Switch({ checked, onChange, label, disabled = false }: { checked: boolean; onChange: (next: boolean) => void; label: string; disabled?: boolean }) {
  return <button
    type="button"
    role="switch"
    aria-checked={checked}
    aria-label={label}
    disabled={disabled}
    className={`plan-switch${checked ? ' on' : ''}`}
    onClick={() => onChange(!checked)}
  ><span /></button>;
}

export function AnalysisSettings() {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [error, setError] = useState('');
  const [preferences, setPreferences] = useState<AnalysisPreferences | null>(null);
  const [preferencesError, setPreferencesError] = useState('');
  const [preferencesNotice, setPreferencesNotice] = useState('');
  const [preferencesSaving, setPreferencesSaving] = useState(false);
  const [ready, setReady] = useState(false);
  const [loading, setLoading] = useState(true);
  const [blocked, setBlocked] = useState(false);
  const [scheduleError, setScheduleError] = useState('');
  const queue = useRef<SettingsSaveQueue<AppSettings> | null>(null);
  const unsubscribe = useRef<(() => void) | null>(null);
  const active = useRef(false);
  const loadId = useRef(0);

  const load = useCallback(async () => {
    const request = ++loadId.current;
    setLoading(true);
    try {
      const stored = await invoke<AppSettings>('app_settings_get');
      settingsUserRevision(stored);
      if (!active.current || request !== loadId.current) return;
      const next = createSettingsSaveQueue({
        initial: mergeSettings(stored),
        save: value => invoke<AppSettings>('app_settings_save', { value }),
        afterSave: async () => { await invoke('devify_daily_schedule_sync'); },
        onAfterSaveError: reason => {
          if (active.current) setScheduleError(`设置已保存，但日报计划同步失败：${analysisSettingsError(reason)}`);
        },
      });
      unsubscribe.current?.();
      queue.current = next;
      unsubscribe.current = next.subscribe(state => {
        if (!active.current) return;
        setSettings(state.draft);
        setBlocked(state.blocked);
        if (state.blocked) setError(settingsSaveFailure(analysisSettingsError(state.error)));
      });
      setError('');
      setReady(true);
    } catch (e) {
      if (active.current && request === loadId.current) setError(`无法加载设置：${analysisSettingsError(e)}`);
    } finally {
      if (active.current && request === loadId.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    active.current = true;
    void load();
    return () => { active.current = false; loadId.current += 1; unsubscribe.current?.(); };
  }, [load]);

  useEffect(() => {
    let active = true;
    if (!ready || blocked) return;
    const spaceId = settings.devify.active_space_id;
    setPreferences(null);
    setPreferencesNotice('');
    if (!spaceId) { setPreferencesError('请先在账户中选择可用空间。'); return; }
    setPreferencesError('');
    void invoke<AnalysisPreferences>('devify_analysis_preferences_get', { spaceId }).then(value => {
      if (!active) return;
      if (!Array.isArray(value.conversation_types) || ![0, 30, 90].includes(value.lookback_days) || typeof value.auto_analyze !== 'boolean') {
        setPreferencesError('当前服务端不支持完整的分析范围设置，请先更新服务端。');
        return;
      }
      setPreferences(value);
    }).catch(reason => { if (active) setPreferencesError(analysisSettingsError(reason)); });
    return () => { active = false; };
  }, [ready, blocked, settings.devify.active_space_id]);

  useEffect(() => {
    if (!preferencesNotice) return;
    const timer = window.setTimeout(() => setPreferencesNotice(''), 4000);
    return () => window.clearTimeout(timer);
  }, [preferencesNotice]);

  const changePreferences = (next: AnalysisPreferences) => {
    setPreferencesNotice('');
    setPreferences(next);
  };

  const savePreferences = async () => {
    const spaceId = settings.devify.active_space_id;
    if (!spaceId || !preferences) return;
    setPreferencesSaving(true); setPreferencesError(''); setPreferencesNotice('');
    try {
      const saved = await invoke<AnalysisPreferences>('devify_analysis_preferences_save', { spaceId, preferences });
      setPreferences(saved);
      setPreferencesNotice('已保存');
    } catch (reason) { setPreferencesError(analysisSettingsError(reason)); }
    finally { setPreferencesSaving(false); }
  };

  /** Save each explicit edit against the most recently confirmed revision. */
  const update = useCallback((change: (current: AppSettings) => AppSettings) => {
    queue.current?.enqueue(change);
  }, []);

  const automation = settings.automation;
  const agent = settings.agent;

  const dailyOn = automation.enabled && automation.daily_summary_enabled;
  const agentLabel = agent.provider_id.trim()
    ? `本地 Agent · ${agent.provider_id}${agent.model.trim() ? ` · ${agent.model}` : ''}`
    : '本地规则整理（未配置 Agent）';
  const lastDaily = automation.last_daily_run
    ? `${automation.last_daily_run} · ${DAILY_STATUS_LABEL[automation.last_daily_status ?? ''] ?? '未知'}`
    : '还没有运行过';

  const tasks: Array<{ title: string; items: Task[] }> = [
    {
      title: '分析',
      items: [{
        id: 'events',
        name: '事件分析',
        desc: '整理事件、更新主题',
        summary: '服务端在备份解析完成后自动生成',
        badge: '服务端',
        rows: [
          { label: '触发方式', value: '备份解析完成后由服务端自动整理' },
          { label: '使用范围', value: '只使用已上传的记录，未上传的本机记录不参与' },
          { label: '查看位置', value: '事件、主题页面（客户端只同步结果）' },
        ],
      }],
    },
    {
      title: '总结',
      items: [{
        id: 'summary',
        name: '聊天总结',
        desc: '按会话生成摘要，单次最长一周',
        summary: agent.provider_id.trim() ? '手动发起 · 本地 Agent 生成' : '手动发起 · 规则整理',
        badge: '手动',
        rows: [
          { label: '触发方式', value: '在聊天页选择消息后点「整理这段聊天」' },
          { label: '生成方式', value: agentLabel },
          { label: '保存位置', value: '总结记录（只保存在本机）' },
          { label: '定时生成', value: '尚未实现，目前只能手动发起' },
        ],
      }],
    },
    {
      title: '报告',
      items: [
        {
          id: 'daily',
          name: '日报',
          desc: '每天由服务端汇总已上传记录',
          summary: dailyOn ? `每天 ${automation.daily_time} · 服务端生成` : '已关闭',
          control: {
            label: '每日日报',
            checked: dailyOn,
            onChange: next => update(current => ({
              ...current,
              automation: { ...current.automation, enabled: next ? true : current.automation.enabled, daily_summary_enabled: next },
            })),
          },
          rows: [
            { label: '触发方式', value: '每天定点（服务端任务，应用关闭也执行）' },
            {
              label: '时间',
              control: <input
                type="time"
                aria-label="日报时间"
                value={automation.daily_time}
                onChange={event => {
                  const value = event.target.value;
                  if (value) update(current => ({ ...current, automation: { ...current.automation, daily_time: value } }));
                }}
              />,
            },
            { label: '统计范围', value: '已上传的记录；未上传的本机记录不参与' },
            { label: '查看位置', value: '报告页面 · 日报' },
            { label: '最近执行', value: lastDaily },
          ],
        },
        {
          id: 'weekly',
          name: '周报',
          desc: '按自然周汇总事件与进展',
          summary: '由服务端生成',
          badge: '服务端',
          rows: [
            { label: '触发方式', value: '服务端按空间设置生成' },
            { label: '查看位置', value: '报告页面 · 周报' },
          ],
        },
        {
          id: 'monthly',
          name: '月报',
          desc: '按自然月汇总事件与进展',
          summary: '由服务端生成',
          badge: '服务端',
          rows: [
            { label: '触发方式', value: '服务端按空间设置生成' },
            { label: '查看位置', value: '报告页面 · 月报' },
          ],
        },
      ],
    },
  ];

  if (!ready) return <div className="settings-page analysis-settings"><h2>智能分析</h2><p>{loading ? '正在加载设置…' : '设置尚未加载，无法安全保存。'}</p>{error && <div className="error" role="alert">{error}</div>}{!loading && <button type="button" onClick={() => void load()}>重新加载</button>}</div>;

  return <div className="settings-page analysis-settings">
    <header className="page-head">
      <h2>智能分析</h2>
      <p className="help">设置当前空间的服务端分析偏好，并查看聊天总结和报告的处理方式。</p>
      <p className="help" role="status">分析范围和行为用于之后创建的任务；事件粒度和摘要要求由模型参考，结果可能有所不同。</p>
    </header>

    <fieldset disabled={blocked} style={{ border: 0, margin: 0, padding: 0, minWidth: 0 }}>
    <section className="analysis-service">
      <div className="analysis-service-icon" aria-hidden="true">✦</div>
      <div><strong>服务端分析</strong><p>事件分析和云端报告由服务端处理，模型和分析配置在服务端管理。</p><small>日报开关不会影响事件分析。</small></div>
    </section>

    <section className="analysis-group analysis-preferences">
      <h3>分析触发</h3>
      <div className="analysis-task">
        <div className="analysis-row"><span>触发方式</span><div>上传后自动分析</div></div>
        <p className="help">上传的聊天记录解析完成后，由服务端自动创建分析任务。</p>
      </div>
    </section>
    <section className="analysis-group analysis-preferences">
      <h3>分析范围</h3>
      <div className="analysis-task">
        <div className="analysis-row"><span>聊天记录</span><div className="analysis-options" role="group" aria-label="聊天记录类型">{([{ value: 'direct', label: '个人聊天' }, { value: 'group', label: '群聊' }] as const).map(option => <label key={option.value}><input type="checkbox" checked={preferences?.conversation_types?.includes(option.value) ?? false} disabled={!preferences || preferencesSaving || (preferences.conversation_types.length === 1 && preferences.conversation_types.includes(option.value))} onChange={() => preferences && changePreferences({ ...preferences, conversation_types: preferences.conversation_types.includes(option.value) ? preferences.conversation_types.filter(value => value !== option.value) : [...preferences.conversation_types, option.value] })}/>{option.label}</label>)}</div></div>
        <label className="analysis-row"><span>时间范围</span><select value={preferences?.lookback_days ?? ''} disabled={!preferences || preferencesSaving} onChange={event => preferences && changePreferences({ ...preferences, lookback_days: Number(event.target.value) as AnalysisPreferences['lookback_days'] })}><option value="" disabled>等待服务端配置</option><option value={0}>全部记录</option><option value={30}>最近 30 天</option><option value={90}>最近 90 天</option></select></label>
        <p className="help">范围在任务创建时固定；有限时间范围不包含没有时间的消息。调整后不会自动重新分析历史记录。</p>
      </div>
    </section>
    <section className="analysis-group analysis-preferences">
      <h3>分析行为</h3>
      <div className="analysis-task">
        <div className="analysis-row"><span>事件划分粒度</span><div className="analysis-options" role="radiogroup" aria-label="事件划分粒度">{([{ value: 'fine', label: '更细（更多事件）' }, { value: 'balanced', label: '平衡' }, { value: 'coarse', label: '更粗（更少事件）' }] as const).map(option => <label key={option.value}><input type="radio" name="event-granularity" value={option.value} checked={preferences?.event_granularity === option.value} disabled={!preferences || preferencesSaving} onChange={() => preferences && changePreferences({ ...preferences, event_granularity: option.value })}/>{option.label}</label>)}</div></div>
        <label className="analysis-row"><span>摘要风格</span><select value={preferences?.summary_style ?? ''} disabled={!preferences || preferencesSaving} onChange={event => preferences && changePreferences({ ...preferences, summary_style: event.target.value as AnalysisPreferences['summary_style'] })}><option value="" disabled>等待服务端配置</option><option value="concise">简洁概述</option><option value="narrative">背景与经过</option><option value="actions">行动与后续</option></select></label>
        <div className="analysis-row"><span>摘要详略</span><div className="analysis-options" role="radiogroup" aria-label="摘要详略">{([{ value: 'short', label: '简短' }, { value: 'standard', label: '标准' }, { value: 'detailed', label: '详细' }] as const).map(option => <label key={option.value}><input type="radio" name="summary-length" value={option.value} checked={preferences?.summary_length === option.value} disabled={!preferences || preferencesSaving} onChange={() => preferences && changePreferences({ ...preferences, summary_length: option.value })}/>{option.label}</label>)}</div></div>
        <p className="help">这些偏好用于当前空间之后创建的服务端分析任务；本机 Agent 总结不受影响。粒度和摘要要求由模型参考，结果可能有所不同。</p>
        <div className="analysis-save-row">
          <button type="button" className="primary" disabled={!preferences || preferencesSaving} onClick={() => void savePreferences()}>{preferencesSaving ? '正在保存…' : '保存分析设置'}</button>
          <span className="analysis-save-status" role="status" aria-live="polite">{preferencesNotice && <><svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><circle cx="8" cy="8" r="6" stroke="currentColor"/><path d="m5 8 2 2 4-4" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round"/></svg>{preferencesNotice}</>}</span>
        </div>
        <p className="analysis-save-hint">设置用于之后创建的分析任务，历史记录不会自动重新分析。</p>
        {preferencesError && <p className="error" role="alert">{preferencesError}</p>}
      </div>
    </section>

    <div className="analysis-groups">
      {tasks.map(group => <section className="analysis-group" key={group.title}>
        <h3>{group.title}</h3>
        {group.items.map(task => <div className="analysis-task" key={task.id}>
          <div className="analysis-task-head"><div><strong>{task.name}</strong><small>{task.desc} · {task.summary}</small></div>{task.control ? <Switch checked={task.control.checked} onChange={task.control.onChange} label={task.control.label}/> : <span className="analysis-badge">{task.badge}</span>}</div>
          <div className="analysis-rows">{task.rows.map(row => <div className="analysis-row" key={row.label}><div><span>{row.label}</span>{row.desc && <small>{row.desc}</small>}</div><div>{row.control ?? row.value}</div></div>)}</div>
        </div>)}
      </section>)}
    </div>

    </fieldset>
    {error && <div className="error" role="alert">{error}</div>}
    {scheduleError && <div className="error" role="alert">{scheduleError}</div>}
    {blocked && <div><p>当前修改未确认保存；重新加载会放弃这些修改。</p><button type="button" disabled={loading} onClick={() => void load()}>{loading ? '正在加载…' : '放弃未保存修改并重新加载'}</button></div>}
  </div>;
}
