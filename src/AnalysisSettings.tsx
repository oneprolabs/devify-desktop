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

function TaskItem({ task }: { task: Task }) {
  const [open, setOpen] = useState(false);
  return <div className="plan-item">
    <div className="plan-item-head">
      <button type="button" className="plan-item-title" aria-expanded={open} onClick={() => setOpen(!open)}>
        <strong>{task.name}<em>{task.desc}</em></strong>
        <small>{task.summary}<i>{open ? '收起' : '查看'} →</i></small>
      </button>
      {task.control
        ? <Switch checked={task.control.checked} onChange={task.control.onChange} label={task.control.label}/>
        : <span className="plan-badge">{task.badge}</span>}
    </div>
    {open && <div className="plan-item-rows">
      {task.rows.map(row => <div className="plan-row" key={row.label}>
        <div className="plan-row-label"><span>{row.label}</span>{row.desc ? <small>{row.desc}</small> : null}</div>
        <div className="plan-row-control">{row.control ?? <span className="plan-kv">{row.value}</span>}</div>
      </div>)}
    </div>}
  </div>;
}

export function AnalysisSettings() {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [error, setError] = useState('');
  const chain = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    void invoke<AppSettings>('app_settings_get')
      .then(stored => setSettings(mergeSettings(stored)))
      .catch(e => setError(String(e)));
  }, []);

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
  const aiLine = automation.enabled
    ? [
      '事件分析与报告由服务端生成',
      dailyOn ? `本地日报每天 ${automation.daily_time} 生成` : '本地日报已关闭',
    ].join(' · ')
    : '已关闭 · 不会在本机定时生成内容，服务端结果仍会同步到事件和报告';

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
      <p className="help">所有平台共用同一套设置。事件分析、聊天总结和报告只使用已上传的记录。</p>
    </header>

    <section className="plan-master">
      <div>
        <strong>智能分析</strong>
        <small>{aiLine}</small>
      </div>
      <Switch
        checked={automation.enabled}
        onChange={next => update(current => ({ ...current, automation: { ...current.automation, enabled: next } }))}
        label="智能分析"
      />
    </section>

    <div className={`plan-groups${automation.enabled ? '' : ' off'}`}>
      {tasks.map(group => <section className="plan-group" key={group.title}>
        <h3>{group.title}</h3>
        <div className="plan-items">{group.items.map(task => <TaskItem key={task.id} task={task}/>)}</div>
      </section>)}
    </div>

    {error && <div className="error">{error}</div>}
  </div>;
}
