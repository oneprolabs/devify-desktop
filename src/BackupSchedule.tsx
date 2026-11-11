import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { type AppSettings, defaultSettings, mergeSettings } from './Settings';
import wechatIcon from './assets/connectors/wechat.svg';
import { ConnectorIcon } from './Connectors';
import { createSettingsSaveQueue, settingsUserRevision, type SettingsSaveQueue } from './settingsSaveQueue';

type ScanResult = { accounts: Array<{ id: string }> };

const WEEKDAYS = ['周一', '周二', '周三', '周四', '周五', '周六', '周日'];
/** The design's stepper allows 1–999 of the selected unit. */
const MAX_STEP = 999;
const MODES: Array<{ id: AppSettings['backup']['mode']; label: string }> = [
  { id: 'interval', label: '按间隔' },
  { id: 'times', label: '每天定点' },
  { id: 'manual', label: '仅手动' },
];
const QUICK_INTERVALS = [
  { label: '1 分钟', seconds: 60 },
  { label: '5 分钟', seconds: 300 },
  { label: '15 分钟', seconds: 900 },
  { label: '30 分钟', seconds: 1800 },
  { label: '1 小时', seconds: 3600 },
  { label: '6 小时', seconds: 21600 },
];
const ATTACHMENTS: Array<{ id: AppSettings['backup']['attachments']; label: string }> = [
  { id: 'text', label: '仅文本' },
  { id: 'images', label: '文本 + 图片' },
  { id: 'all', label: '全部（≤ 25 MB）' },
];

function minutesOf(value: string): number | null {
  const match = /^(\d{2}):(\d{2})$/.exec(value);
  if (!match) return null;
  const hour = Number(match[1]);
  const minute = Number(match[2]);
  return hour < 24 && minute < 60 ? hour * 60 + minute : null;
}

function clockOf(date: Date): number {
  return date.getHours() * 60 + date.getMinutes();
}

/** Same rules as `src-tauri/src/backup.rs`, for the "下次备份" sentence only. */
function nextRunLabel(plan: AppSettings['backup'], intervalSeconds: number, lastRun: Date | null): string {
  const now = new Date();
  const blocked = plan.window_enabled ? ' · 上传时段外会顺延' : '';
  if (plan.mode === 'manual') return '仅在手动触发时';
  const scheduled = plan.mode === 'times' ? plan.times.map(minutesOf).filter((value): value is number => value !== null).sort((a, b) => a - b) : [];
  if (plan.mode === 'times') {
    if (!scheduled.length) return '尚未设置时间';
    const at = (minutes: number, dayOffset: number) => new Date(now.getFullYear(), now.getMonth(), now.getDate() + dayOffset, Math.floor(minutes / 60), minutes % 60);
    let next = scheduled.map(minutes => at(minutes, 0)).find(date => date.getTime() > now.getTime());
    if (!next) next = at(scheduled[0], 1);
    const today = next.toDateString() === now.toDateString();
    return `${today ? '今天' : '明天'} ${String(next.getHours()).padStart(2, '0')}:${String(next.getMinutes()).padStart(2, '0')}${blocked}`;
  }
  if (!lastRun) return '启动后立即';
  const due = new Date(lastRun.getTime() + intervalSeconds * 1000);
  const seconds = Math.round((due.getTime() - now.getTime()) / 1000);
  if (seconds <= 30) return '马上' + blocked;
  if (seconds < 90) return '不到 1 分钟内' + blocked;
  const minutes = Math.round(seconds / 60);
  return `约 ${minutes < 60 ? `${minutes} 分钟` : `${(minutes / 60).toFixed(1)} 小时`}后${blocked}`;
}

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

function Row({ label, desc, children }: { label: string; desc?: string; children: ReactNode }) {
  return <div className="plan-row">
    <div className="plan-row-label"><span>{label}</span>{desc ? <small>{desc}</small> : null}</div>
    <div className="plan-row-control">{children}</div>
  </div>;
}

function Pills<T extends string | number>({ options, value, onChange, label }: {
  options: Array<{ id: T; label: string }>;
  value: T;
  onChange: (next: T) => void;
  label: string;
}) {
  return <div className="plan-pills" role="group" aria-label={label}>
    {options.map(option => <button
      key={option.id}
      type="button"
      aria-pressed={option.id === value}
      className={option.id === value ? 'on' : ''}
      onClick={() => onChange(option.id)}
    >{option.label}</button>)}
  </div>;
}

function formatInterval(seconds: number): string {
  if (seconds % 3600 === 0) return `每 ${seconds / 3600} 小时`;
  return `每 ${Math.round(seconds / 60)} 分钟`;
}

export function BackupSchedule({ onOpenConnectors }: { onOpenConnectors?: () => void }) {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [connected, setConnected] = useState(false);
  // The plan opens expanded, matching the settings screen the design was drawn for; the
  // header folds it down to the summary line once it is configured.
  const [open, setOpen] = useState(true);
  const [unitPref, setUnitPref] = useState<'auto' | '分钟' | '小时'>('auto');
  const [newTime, setNewTime] = useState('12:00');
  const [error, setError] = useState('');
  const [ready, setReady] = useState(false);
  const [loading, setLoading] = useState(true);
  const [blocked, setBlocked] = useState(false);
  const queue = useRef<SettingsSaveQueue<AppSettings> | null>(null);
  const unsubscribe = useRef<(() => void) | null>(null);
  const active = useRef(false);
  const loadId = useRef(0);

  const load = useCallback(async () => {
    const request = ++loadId.current;
    setLoading(true);
    try {
      const [stored, scan] = await Promise.all([
        invoke<AppSettings>('app_settings_get'),
        invoke<ScanResult>('wechat_scan').catch(() => ({ accounts: [] })),
      ]);
      settingsUserRevision(stored);
      if (!active.current || request !== loadId.current) return;
      const next = createSettingsSaveQueue({
        initial: mergeSettings(stored),
        save: value => invoke<AppSettings>('app_settings_save', { value }),
      });
      unsubscribe.current?.();
      queue.current = next;
      unsubscribe.current = next.subscribe(state => {
        if (!active.current) return;
        setSettings(state.draft);
        setBlocked(state.blocked);
        if (state.error) setError(`保存失败，修改尚未确认：${String(state.error)}`);
      });
      setConnected((scan.accounts ?? []).length > 0);
      setError('');
      setReady(true);
    } catch (e) {
      if (active.current && request === loadId.current) setError(`无法加载设置：${String(e)}`);
    } finally {
      if (active.current && request === loadId.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    active.current = true;
    void load();
    return () => { active.current = false; loadId.current += 1; unsubscribe.current?.(); };
  }, [load]);

  const plan = settings.backup;
  const intervalSeconds = settings.chat.sync_interval_seconds;
  // The scheduler runs in the backend while this page stays open, so the run
  // status is polled instead of being frozen at mount time.
  const [runStatus, setRunStatus] = useState<AppSettings['sync'] | null>(null);
  useEffect(() => {
    let active = true;
    const load = async () => {
      try {
        const stored = await invoke<AppSettings>('app_settings_get');
        if (!active) return;
        setRunStatus(mergeSettings(stored).sync);
      } catch {
        /* a failed poll keeps the last known status */
      }
    };
    void load();
    // WebKit throttles timers while the window is in the background, so a fresh
    // focus also refreshes the run status immediately.
    const onFocus = () => void load();
    window.addEventListener('focus', onFocus);
    const timer = window.setInterval(load, 5000);
    return () => {
      active = false;
      window.clearInterval(timer);
      window.removeEventListener('focus', onFocus);
    };
  }, []);
  const syncState = runStatus ?? settings.sync;
  const lastRun = syncState.last_run_at ? new Date(syncState.last_run_at) : null;

  /** Every control saves its explicit change against the last confirmed revision. */
  const update = useCallback((change: (current: AppSettings) => AppSettings) => {
    queue.current?.enqueue(change);
  }, []);

  const setPlan = (patch: Partial<AppSettings['backup']>) => update(current => ({ ...current, backup: { ...current.backup, ...patch } }));
  const setInterval = (seconds: number) => update(current => ({ ...current, chat: { ...current.chat, sync_interval_seconds: Math.min(MAX_STEP * 3600, Math.max(60, seconds)) } }));

  // The stepper keeps the unit the user picked; "auto" follows what the stored value divides by.
  const derivedUnit: '分钟' | '小时' = intervalSeconds % 3600 === 0 && intervalSeconds >= 3600 ? '小时' : '分钟';
  const unit = unitPref === 'auto' ? derivedUnit : unitPref;
  const perUnit = unit === '小时' ? 3600 : 60;
  const stepValue = Math.min(MAX_STEP, Math.max(1, Math.round(intervalSeconds / perUnit)));
  const step = (delta: number) => {
    const next = Math.min(MAX_STEP, Math.max(1, stepValue + delta));
    setInterval(next * perUnit);
  };
  const switchUnit = () => {
    const next: '分钟' | '小时' = unit === '小时' ? '分钟' : '小时';
    const per = next === '小时' ? 3600 : 60;
    const value = Math.min(MAX_STEP, Math.max(1, Math.round(intervalSeconds / per)));
    setUnitPref(next);
    setInterval(value * per);
  };

  const summary = useMemo(() => {
    if (!settings.sync.enabled) return '已关闭';
    const base = plan.mode === 'interval'
      ? formatInterval(intervalSeconds)
      : plan.mode === 'times'
        ? `每天 ${plan.times.join('、') || '未设置'}`
        : '仅手动';
    const window = plan.window_enabled
      ? ` · ${plan.window_mode === 'deny' ? '避开' : '仅'} ${plan.window_start}–${plan.window_end}`
      : '';
    const network = plan.wifi_only ? ' · 仅 Wi-Fi' : '';
    return `${base}${window}${network}`;
  }, [settings.sync.enabled, plan, intervalSeconds]);

  const windowHint = plan.window_start > plan.window_end
    ? `跨午夜：${plan.window_start} → 次日 ${plan.window_end}`
    : `${plan.window_start} – ${plan.window_end}`;
  const windowRule = plan.window_mode === 'deny'
    ? '，该时段暂停上传，结束后自动补传'
    : '，其余时间攒着，到点集中上传';

  if (!ready) return <div className="settings-page backup-page"><h2>备份周期</h2><p>{loading ? '正在加载设置…' : '设置尚未加载，无法安全保存。'}</p>{error && <div className="error" role="alert">{error}</div>}{!loading && <button type="button" onClick={() => void load()}>重新加载</button>}</div>;

  return <div className="settings-page backup-page">
    <header className="page-head">
      <h2>备份周期</h2>
      <p className="help">选择聊天记录自动备份的时间与范围，只上传已导入会话的新增消息。</p>
    </header>

    <fieldset disabled={blocked} style={{ border: 0, margin: 0, padding: 0, minWidth: 0 }}>
    <div className="backup-list">
      {connected && <section className={`plan-plat${open ? ' open' : ''}`}>
        <div className="backup-section backup-master"><h3>自动备份</h3><div className="plan-plat-head">
          <span className="plan-mark"><img className="backup-wechat-logo" src={wechatIcon} alt="" /></span>
          <button type="button" className="plan-plat-title" aria-expanded={open} onClick={() => setOpen(!open)}>
            <strong>微信自动备份</strong>
            <small><span>{summary}</span><i>{open ? '收起' : '编辑'}</i></small>
          </button>
          <Switch checked={settings.sync.enabled} onChange={next => update(current => ({ ...current, sync: { ...current.sync, enabled: next } }))} label="微信自动备份"/>
        </div></div>
        {open && <div className={`plan-rows${settings.sync.enabled ? '' : ' off'}`}>
          <div className="backup-section"><h3>备份频率</h3>
          <Row label="备份方式">
            <Pills label="备份方式" value={plan.mode} options={MODES} onChange={mode => setPlan({ mode })}/>
          </Row>
          {plan.mode === 'interval' && <>
            <Row label="快速选择" desc="最短 1 分钟，间隔越短越接近实时">
              <Pills label="常用间隔" value={intervalSeconds} options={QUICK_INTERVALS.map(item => ({ id: item.seconds, label: item.label }))} onChange={setInterval}/>
            </Row>
            <Row label="自定义间隔">
              <div className="plan-step">
                <button type="button" aria-label="减少" onClick={() => step(-1)} disabled={stepValue <= 1}>−</button>
                <b>{stepValue}</b>
                <button type="button" aria-label="增加" onClick={() => step(1)}>＋</button>
                <button type="button" className="plan-unit" onClick={switchUnit} title="切换分钟 / 小时">{unit}</button>
              </div>
            </Row>
          </>}
          {plan.mode === 'times' && <Row label="备份时间" desc="可添加多个，精确到分钟">
            <div className="plan-times">
              {plan.times.map(time => <span key={time} className="plan-chip">{time}
                <button
                  type="button"
                  aria-label={`删除 ${time}`}
                  disabled={plan.times.length <= 1}
                  onClick={() => setPlan({ times: plan.times.filter(value => value !== time) })}
                >×</button>
              </span>)}
              <input
                type="time"
                aria-label="备份时间"
                value={newTime}
                onChange={event => setNewTime(event.target.value)}
              />
              <button
                type="button"
                className="plan-add"
                onClick={() => {
                  if (newTime && !plan.times.includes(newTime)) setPlan({ times: [...plan.times, newTime].sort() });
                }}
              >添加</button>
            </div>
          </Row>}
          </div><div className="backup-section"><h3>上传内容</h3>
          <Row label="上传内容" desc="选择备份时包含的消息和附件">
            <Pills label="上传内容" value={plan.attachments} options={ATTACHMENTS} onChange={attachments => setPlan({ attachments })}/>
          </Row>
          </div><div className="backup-section"><h3>网络与时间限制</h3>
          <Row label="限制上传时间" desc="只在允许的时间段上传，或避开某个时间段">
            <Switch checked={plan.window_enabled} onChange={window_enabled => setPlan({ window_enabled })} label="限制上传时间"/>
          </Row>
          {plan.window_enabled && <>
            <Row label="限制方式">
              <Pills
                label="限制方式"
                value={plan.window_mode}
                options={[{ id: 'deny' as const, label: '此时段内不上传' }, { id: 'allow' as const, label: '只在此时段上传' }]}
                onChange={window_mode => setPlan({ window_mode })}
              />
            </Row>
            <Row label="时间段" desc={`${windowHint}${windowRule}`}>
              <div className="plan-window">
                <input type="time" aria-label="开始时间" value={plan.window_start} onChange={event => { if (event.target.value) setPlan({ window_start: event.target.value }); }}/>
                至
                <input type="time" aria-label="结束时间" value={plan.window_end} onChange={event => { if (event.target.value) setPlan({ window_end: event.target.value }); }}/>
              </div>
            </Row>
            <Row label="生效日">
              <div className="plan-pills" role="group" aria-label="生效日">
                {WEEKDAYS.map((label, day) => <button
                  key={label}
                  type="button"
                  aria-pressed={plan.days.includes(day)}
                  className={plan.days.includes(day) ? 'on' : ''}
                  onClick={() => setPlan({
                    days: plan.days.includes(day)
                      ? (plan.days.length > 1 ? plan.days.filter(value => value !== day) : plan.days)
                      : [...plan.days, day].sort((a, b) => a - b),
                  })}
                >{label}</button>)}
              </div>
            </Row>
          </>}
          <Row label="仅在 Wi-Fi 下上传">
            <Switch checked={plan.wifi_only} onChange={wifi_only => setPlan({ wifi_only })} label="仅在 Wi-Fi 下上传"/>
          </Row>
          {settings.sync.enabled && plan.mode === 'interval' && <Row label="下次备份">
            <span className="plan-kv">{nextRunLabel(plan, intervalSeconds, lastRun)}</span>
          </Row>}
          </div>
        </div>}
        {open && syncState.last_error && <div className="backup-error">{syncState.last_error}</div>}
      </section>}

      <div className="plan-off-head">未连接</div>
      {!connected && <div className="plan-off">
        <span className="plan-mark dim"><img className="backup-wechat-logo" src={wechatIcon} alt="" /></span>
        <div><strong>微信</strong><small>连接后可单独设置备份周期</small></div>
        <button type="button" onClick={onOpenConnectors}>打开连接向导</button>
      </div>}
      {[{ name: 'WhatsApp', hint: '尚未接入' }, { name: '邮件', hint: '尚未接入' }].map(item => <div className="plan-off" key={item.name}>
        <ConnectorIcon name={item.name} />
        <div><strong>{item.name}</strong><small>连接后可单独设置备份周期</small></div>
        <button type="button" className="ghost" onClick={onOpenConnectors}>{item.hint}</button>
      </div>)}
    </div>

    </fieldset>
    {error && <div className="error" role="alert">{error}</div>}
    {blocked && <div><p>当前修改未确认保存；重新加载会放弃这些修改。</p><button type="button" disabled={loading} onClick={() => void load()}>{loading ? '正在加载…' : '放弃未保存修改并重新加载'}</button></div>}
  </div>;
}
