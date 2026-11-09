import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { type AppSettings, defaultSettings, mergeSettings } from './Settings';

const DAILY_STATUS_LABEL: Record<string, string> = {
  sent: '已投递',
  no_messages: '当天无新消息',
  busy: '账号正忙，本次跳过',
  error: '未完成',
};

type Row = { label: string; desc?: string; control?: ReactNode; value?: string };
type AnalysisPreferences = { event_granularity: 'fine' | 'balanced' | 'coarse'; summary_length: 'short' | 'standard' | 'detailed'; summary_style: 'concise' | 'narrative' | 'actions'; conversation_types: Array<'direct' | 'group'>; lookback_days: 0 | 30 | 90; auto_analyze: boolean };
type CloudConversation = { id: number; title?: string; source_conversation_id?: string };

type Task = {
  id: string;
  name: string;
  desc: string;
  summary: string;
  control?: { checked: boolean; onChange: (next: boolean) => void; label: string };
  badge?: string;
  rows: Row[];
};

function Switch({ checked, onChange, label }: { checked: boolean; onChange: (next: boolean) => void; label: string }) {
  return <button
    type="button"
    role="switch"
    aria-checked={checked}
    aria-label={label}
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
  const [savedPreferences, setSavedPreferences] = useState('');
  const [conversations, setConversations] = useState<CloudConversation[]>([]);
  const [manualSelection, setManualSelection] = useState('');
  const [manualLoading, setManualLoading] = useState(false);
  const [manualMessage, setManualMessage] = useState('');
  const chain = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    void invoke<AppSettings>('app_settings_get')
      .then(stored => setSettings(mergeSettings(stored)))
      .catch(e => setError(String(e)));
  }, []);

  useEffect(() => {
    let active = true;
    const spaceId = settings.devify.active_space_id;
    setPreferences(null);
    setSavedPreferences(''); setConversations([]); setManualSelection(''); setManualMessage('');
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
      setSavedPreferences(JSON.stringify(value));
    }).catch(reason => { if (active) setPreferencesError(String(reason)); });
    return () => { active = false; };
  }, [settings.devify.active_space_id]);

  const savePreferences = async () => {
    const spaceId = settings.devify.active_space_id;
    if (!spaceId || !preferences) return;
    setPreferencesSaving(true); setPreferencesError(''); setPreferencesNotice('');
    try {
      const saved = await invoke<AnalysisPreferences>('devify_analysis_preferences_save', { spaceId, preferences });
      setPreferences(saved); setSavedPreferences(JSON.stringify(saved));
      setPreferencesNotice('分析偏好已保存，将用于之后执行的分析任务。');
    } catch (reason) { setPreferencesError(String(reason)); }
    finally { setPreferencesSaving(false); }
  };

  const loadConversations = async () => {
    const spaceId = settings.devify.active_space_id;
    if (!spaceId) return;
    setManualLoading(true); setManualMessage('');
    try {
      const items: CloudConversation[] = [];
      for (let page = 1; page <= 100; page += 1) {
        const result = await invoke<{ list?: CloudConversation[]; pagination?: { next?: unknown } }>('devify_conversations', { spaceId, page, pageSize: 100 });
        if (!Array.isArray(result.list) || result.pagination?.next === undefined) throw new Error('无法完整读取云端会话，请重试。');
        items.push(...result.list);
        if (!result.pagination.next) break;
        if (page === 100) throw new Error('云端会话过多，请在服务端选择会话。');
      }
      setConversations(items); setManualSelection('');
      if (!items.length) setManualMessage('当前空间没有可分析的云端会话。');
    } catch (reason) { setConversations([]); setManualMessage(String(reason)); }
    finally { setManualLoading(false); }
  };

  const startManualAnalysis = async () => {
    const spaceId = settings.devify.active_space_id;
    const conversationId = Number(manualSelection);
    if (!spaceId || !conversationId) return;
    setManualLoading(true); setManualMessage('');
    try {
      const job = await invoke<{ state: string; id: number; reused?: boolean }>('devify_analysis_manual_start', { spaceId, conversationId });
      const status: Record<string, string> = { queued: '已排队', running: '正在处理', ready: '已完成', partial: '部分完成', failed: '已失败', cancelled: '已取消，不会自动重启' };
      const reuseNotice = job.reused ? (['queued', 'running'].includes(job.state) ? ' 当前会话已有任务，将沿用该任务创建时的范围；如需新的范围，请在任务结束后再次发起。' : ' 已复用已有任务，没有重新启动或重复创建。') : '';
      setManualMessage(`分析任务 #${job.id}：${status[job.state] ?? job.state}。${reuseNotice}`);
    } catch (reason) { setManualMessage(String(reason)); }
    finally { setManualLoading(false); }
  };

  /** Same rule as 备份周期: each control saves straight away, no separate "保存" step. */
  const update = useCallback((change: (current: AppSettings) => AppSettings) => {
    setSettings(current => {
      const next = change(current);
      chain.current = chain.current.then(async () => {
        try {
          await invoke<AppSettings>('app_settings_save', { value: next });
          setError('');
        } catch (e) {
          setError(`保存失败：${String(e)}`);
        }
      });
      return next;
    });
  }, []);

  const automation = settings.automation;
  const agent = settings.agent;
  const recipient = settings.devify.recipient.trim();

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
          { label: '触发方式', value: '备份解析完成后由 Devify 服务端自动整理' },
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
          desc: '每天把已上传的记录汇总后投递到 Devify',
          summary: dailyOn ? `每天 ${automation.daily_time} · 投递到 Devify` : '已关闭',
          control: {
            label: '每日日报',
            checked: dailyOn,
            onChange: next => update(current => ({
              ...current,
              automation: { ...current.automation, enabled: next ? true : current.automation.enabled, daily_summary_enabled: next },
            })),
          },
          rows: [
            { label: '触发方式', value: '每天定点（应用运行时检查）' },
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
            { label: '投递到', desc: '在「账户」页面配置 Devify 接收地址', value: recipient || '未配置接收地址' },
            { label: '统计范围', value: '当天已上传的记录；没有新消息时跳过' },
            { label: '生成方式', value: agent.use_for_daily && agent.provider_id.trim() ? agentLabel : '规则整理' },
            { label: '最近执行', value: lastDaily },
          ],
        },
        {
          id: 'weekly',
          name: '周报',
          desc: '按自然周汇总事件与进展',
          summary: '由 Devify 服务端生成',
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
          summary: '由 Devify 服务端生成',
          badge: '服务端',
          rows: [
            { label: '触发方式', value: '服务端按空间设置生成' },
            { label: '查看位置', value: '报告页面 · 月报' },
          ],
        },
      ],
    },
  ];

  return <div className="settings-page analysis-settings">
    <header className="page-head">
      <h2>智能分析</h2>
      <p className="help">设置当前空间的服务端分析偏好，并查看聊天总结和报告的处理方式。</p>
    </header>

    <section className="analysis-service">
      <div className="analysis-service-icon" aria-hidden="true">✦</div>
      <div><strong>服务端分析</strong><p>事件分析和云端报告由服务端处理，模型和分析配置在服务端管理。</p><small>本机日报开关不会影响服务端分析。</small></div>
    </section>

    <section className="analysis-group analysis-preferences">
      <h3>分析触发</h3>
      <div className="analysis-task">
        <div className="analysis-row"><span>触发方式</span><div className="analysis-options" role="radiogroup" aria-label="分析触发方式">{([{ value: true, label: '上传后自动分析' }, { value: false, label: '手动分析' }] as const).map(option => <label key={String(option.value)}><input type="radio" name="analysis-trigger" checked={preferences?.auto_analyze === option.value} disabled={!preferences || preferencesSaving} onChange={() => preferences && setPreferences({ ...preferences, auto_analyze: option.value })}/>{option.label}</label>)}</div></div>
        <p className="help">上传解析完成时按当前模式决定是否创建分析任务；已排队或正在执行的任务不受影响。切回自动模式不会主动补分析过去的记录，但解析任务重试时会按当时的模式处理。</p>
        {preferences && !preferences.auto_analyze && <div className="analysis-manual"><p>保存设置后，可选择一个云端会话手动分析。将处理当前范围内尚未完成分析的云端消息，可能包含历史记录；不会重启已取消的任务。</p><button type="button" className="secondary" disabled={manualLoading || JSON.stringify(preferences) !== savedPreferences} onClick={() => void loadConversations()}>读取云端会话</button>{conversations.length > 0 && <><select aria-label="选择手动分析会话" value={manualSelection} onChange={event => setManualSelection(event.target.value)}><option value="">选择一个会话</option>{conversations.map(item => <option key={item.id} value={item.id}>{item.title || item.source_conversation_id || `会话 ${item.id}`}</option>)}</select><button type="button" className="primary" disabled={!manualSelection || manualLoading || JSON.stringify(preferences) !== savedPreferences} onClick={() => void startManualAnalysis()}>{manualLoading ? '正在处理…' : '开始分析所选会话'}</button></>}{manualMessage && <p role="status">{manualMessage}</p>}</div>}
      </div>
    </section>
    <section className="analysis-group analysis-preferences">
      <h3>分析范围</h3>
      <div className="analysis-task">
        <div className="analysis-row"><span>聊天记录</span><div className="analysis-options" role="group" aria-label="聊天记录类型">{([{ value: 'direct', label: '个人聊天' }, { value: 'group', label: '群聊' }] as const).map(option => <label key={option.value}><input type="checkbox" checked={preferences?.conversation_types?.includes(option.value) ?? false} disabled={!preferences || preferencesSaving || (preferences.conversation_types.length === 1 && preferences.conversation_types.includes(option.value))} onChange={() => preferences && setPreferences({ ...preferences, conversation_types: preferences.conversation_types.includes(option.value) ? preferences.conversation_types.filter(value => value !== option.value) : [...preferences.conversation_types, option.value] })}/>{option.label}</label>)}</div></div>
        <label className="analysis-row"><span>时间范围</span><select value={preferences?.lookback_days ?? ''} disabled={!preferences || preferencesSaving} onChange={event => preferences && setPreferences({ ...preferences, lookback_days: Number(event.target.value) as AnalysisPreferences['lookback_days'] })}><option value="" disabled>等待服务端配置</option><option value={0}>全部记录</option><option value={30}>最近 30 天</option><option value={90}>最近 90 天</option></select></label>
        <p className="help">范围在任务创建时固定；限时范围不包含没有时间的消息。调整范围不会自动重新分析历史记录。</p>
      </div>
    </section>
    <section className="analysis-group analysis-preferences">
      <h3>分析行为</h3>
      <div className="analysis-task">
        <div className="analysis-row"><span>事件划分粒度</span><div className="analysis-options" role="radiogroup" aria-label="事件划分粒度">{([{ value: 'fine', label: '更细（更多事件）' }, { value: 'balanced', label: '平衡' }, { value: 'coarse', label: '更粗（更少事件）' }] as const).map(option => <label key={option.value}><input type="radio" name="event-granularity" value={option.value} checked={preferences?.event_granularity === option.value} disabled={!preferences || preferencesSaving} onChange={() => preferences && setPreferences({ ...preferences, event_granularity: option.value })}/>{option.label}</label>)}</div></div>
        <label className="analysis-row"><span>摘要风格</span><select value={preferences?.summary_style ?? ''} disabled={!preferences || preferencesSaving} onChange={event => preferences && setPreferences({ ...preferences, summary_style: event.target.value as AnalysisPreferences['summary_style'] })}><option value="" disabled>等待服务端配置</option><option value="concise">简洁概述</option><option value="narrative">背景与经过</option><option value="actions">行动与后续</option></select></label>
        <div className="analysis-row"><span>摘要详略</span><div className="analysis-options" role="radiogroup" aria-label="摘要详略">{([{ value: 'short', label: '简短' }, { value: 'standard', label: '标准' }, { value: 'detailed', label: '详细' }] as const).map(option => <label key={option.value}><input type="radio" name="summary-length" value={option.value} checked={preferences?.summary_length === option.value} disabled={!preferences || preferencesSaving} onChange={() => preferences && setPreferences({ ...preferences, summary_length: option.value })}/>{option.label}</label>)}</div></div>
        <p className="help">这些偏好仅用于当前空间的服务端事件提取和笔记摘要；模型结果可能有所不同。本机 Agent 总结不受影响，历史记录不会自动重算。</p>
        <button type="button" className="primary" disabled={!preferences || preferencesSaving} onClick={() => void savePreferences()}>{preferencesSaving ? '正在保存…' : '保存分析设置'}</button>
        {preferencesNotice && <p role="status">{preferencesNotice}</p>}{preferencesError && <p className="error" role="alert">{preferencesError}</p>}
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

    {error && <div className="error">{error}</div>}
  </div>;
}
