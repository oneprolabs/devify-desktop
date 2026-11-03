import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { type AppSettings, defaultSettings, mergeSettings } from './Settings';

type SyncSummary = { status: string; uploaded: number; skipped: number; message_count: number; error: string | null };
type ScanResult = { accounts: Array<{ id: string }> };

const WEEKDAYS = ['周一', '周二', '周三', '周四', '周五', '周六', '周日'];
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
const STATUS_LABEL: Record<string, string> = {
  success: '已完成',
  failed: '未完成',
  idle: '未运行',
  syncing: '同步中',
};

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
  const [open, setOpen] = useState(false);
  const [unitPref, setUnitPref] = useState<'auto' | '分钟' | '小时'>('auto');
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const chain = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    void (async () => {
      try {
        const [stored, scan] = await Promise.all([
          invoke<AppSettings>('app_settings_get'),
          invoke<ScanResult>('wechat_scan').catch(() => ({ accounts: [] })),
        ]);
        setSettings(mergeSettings(stored));
        setConnected((scan.accounts ?? []).length > 0);
      } catch (e) {
        setError(String(e));
      }
    })();
  }, []);

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

  /** Every control saves straight away, so the page never needs a separate "保存" step. */
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

  const setPlan = (patch: Partial<AppSettings['backup']>) => update(current => ({ ...current, backup: { ...current.backup, ...patch } }));
  const setInterval = (seconds: number) => update(current => ({ ...current, chat: { ...current.chat, sync_interval_seconds: Math.min(604800, Math.max(60, seconds)) } }));

  const syncNow = async () => {
    setBusy(true);
    setError('');
    setNotice('');
    try {
      const result = await invoke<SyncSummary>('devify_sync_now');
      setNotice(result.error
        ? `备份未完成：${result.error}`
        : result.message_count
          ? `已上传 ${result.message_count} 条消息，等待服务端解析。`
          : '白名单会话没有需要上传的新消息。');
      setSettings(mergeSettings(await invoke<AppSettings>('app_settings_get')));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // The stepper keeps the unit the user picked; "auto" follows what the stored value divides by.
  const derivedUnit: '分钟' | '小时' = intervalSeconds % 3600 === 0 && intervalSeconds >= 3600 ? '小时' : '分钟';
  const unit = unitPref === 'auto' ? derivedUnit : unitPref;
  const perUnit = unit === '小时' ? 3600 : 60;
  const stepValue = Math.max(1, Math.round(intervalSeconds / perUnit));
  const step = (delta: number) => setInterval(Math.max(1, stepValue + delta) * perUnit);
  const switchUnit = () => {
    const next: '分钟' | '小时' = unit === '小时' ? '分钟' : '小时';
    setUnitPref(next);
    setInterval(Math.max(1, Math.round(intervalSeconds / (next === '小时' ? 3600 : 60))) * (next === '小时' ? 3600 : 60));
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
    return `${base}${window}`;
  }, [settings.sync.enabled, plan, intervalSeconds]);

  const windowHint = plan.window_start > plan.window_end
    ? `跨午夜：${plan.window_start} → 次日 ${plan.window_end}`
    : `${plan.window_start} – ${plan.window_end}`;
  const windowRule = plan.window_mode === 'deny'
    ? '，该时段暂停上传，结束后自动补传'
    : '，其余时间攒着，到点集中上传';

  return <div className="settings-page backup-page">
    <header className="page-head">
      <h2>备份周期</h2>
      <p className="help">每个平台单独设置，也可以单独开关。只上传已导入会话的新增消息，端到端加密，本机原始数据不会被修改。</p>
    </header>

    <div className="backup-list">
      {connected && <section className={`plan-plat${open ? ' open' : ''}`}>
        <div className="plan-plat-head">
          <span className="plan-mark">微</span>
          <button type="button" className="plan-plat-title" aria-expanded={open} onClick={() => setOpen(!open)}>
            <strong>微信</strong>
            <small>{summary}<i>{open ? '收起' : '编辑'} →</i></small>
          </button>
          <Switch checked={settings.sync.enabled} onChange={next => update(current => ({ ...current, sync: { ...current.sync, enabled: next } }))} label="微信自动备份"/>
        </div>
        {open && <div className={`plan-rows${settings.sync.enabled ? '' : ' off'}`}>
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
                aria-label="添加备份时间"
                onChange={event => {
                  const value = event.target.value;
                  if (value && !plan.times.includes(value)) setPlan({ times: [...plan.times, value].sort() });
                  event.target.value = '';
                }}
              />
            </div>
          </Row>}
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
          {settings.sync.enabled && plan.mode !== 'manual' && <Row label="下次备份"><span className="plan-kv">{nextRunLabel(plan, intervalSeconds, lastRun)}</span></Row>}
          <Row label="最近备份"><span className="plan-kv">{lastRun
            ? `${lastRun.toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })} · ${STATUS_LABEL[syncState.last_status ?? ''] ?? '未知'} · ${syncState.last_message_count ? `${syncState.last_message_count} 条新消息` : '没有新消息'}`
            : '还没有运行过'}</span></Row>
        </div>}
        {open && syncState.last_error && <div className="backup-error">{syncState.last_error}</div>}
        {open && <div className="plan-plat-actions">
          <button type="button" className="secondary" disabled={busy} onClick={() => void syncNow()}>{busy ? '正在备份…' : '立即备份一次'}</button>
        </div>}
      </section>}

      <div className="plan-off-head">未连接</div>
      {!connected && <div className="plan-off">
        <span className="plan-mark dim">微</span>
        <div><strong>微信</strong><small>连接后可单独设置备份周期</small></div>
        <button type="button" onClick={onOpenConnectors}>打开连接向导</button>
      </div>}
      {[{ mark: 'W', name: 'WhatsApp', hint: '尚未接入' }, { mark: '@', name: '邮件', hint: '尚未接入' }].map(item => <div className="plan-off" key={item.name}>
        <span className="plan-mark dim alt">{item.mark}</span>
        <div><strong>{item.name}</strong><small>连接后可单独设置备份周期</small></div>
        <button type="button" className="ghost" onClick={onOpenConnectors}>{item.hint}</button>
      </div>)}
    </div>

    {notice && <div className="notice">{notice}</div>}
    {error && <div className="error">{error}</div>}
  </div>;
}
