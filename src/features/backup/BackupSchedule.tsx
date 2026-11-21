import { t, tf, ctf } from '../../shared/i18n/locale';
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { type AppSettings, defaultSettings, mergeSettings } from '../settings/Settings';
import wechatIcon from '../../assets/connectors/wechat.svg';
import whatsappIcon from '../../assets/connectors/whatsapp.svg';
import { createSettingsSaveQueue, settingsUserRevision, type SettingsSaveQueue } from '../../shared/state/settingsSaveQueue';
import { WizardActionButton } from '../../shared/ui/WizardActionButton';
import { useTaskRuntime } from '../tasks/taskRuntime';
import { BackupRuntimeSwitch } from './BackupRuntimeSwitch';
import './backup-schedule.css';

type ChatAccount = { id: string; source: string };

const WEEKDAYS = ['周一', '周二', '周三', '周四', '周五', '周六', '周日'];
/** The design's stepper allows 1–999 of the selected unit. */
const MAX_STEP = 999;
const MODES: Array<{ id: AppSettings['backup']['mode']; label: string }> = [
  { id: 'interval', label: '按间隔' },
  { id: 'times', label: '每天定点' },
  { id: 'manual', label: '仅手动' },
];
const QUICK_INTERVALS = [
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

function Switch({ checked, onChange, label, disabled = false, busy = false }: { checked: boolean; onChange: (next: boolean) => void; label: string; disabled?: boolean; busy?: boolean }) {
  return <button
    type="button"
    role="switch"
    aria-checked={checked}
    aria-label={t(label)}
    aria-busy={busy}
    disabled={disabled || busy}
    className={`plan-switch${checked ? ' on' : ''}`}
    onClick={() => onChange(!checked)}
  ><span /></button>;
}

function Row({ label, desc, children }: { label: string; desc?: string; children: ReactNode }) {
  return <div className="plan-row">
    <div className="plan-row-label"><span>{t(label)}</span>{desc ? <small>{desc}</small> : null}</div>
    <div className="plan-row-control">{children}</div>
  </div>;
}

function Pills<T extends string | number>({ options, value, onChange, label }: {
  options: Array<{ id: T; label: string }>;
  value: T;
  onChange: (next: T) => void;
  label: string;
}) {
  return <div className="plan-pills" role="group" aria-label={t(label)}>
    {options.map(option => <button
      key={option.id}
      type="button"
      aria-pressed={option.id === value}
      className={option.id === value ? 'on' : ''}
      onClick={() => onChange(option.id)}
    >{t(option.label)}</button>)}
  </div>;
}

export function BackupSchedule({ onOpenConnectors, onContinue, wizard = false, onBack }: { onOpenConnectors?: () => void; onContinue?: () => void | Promise<void>; wizard?: boolean; onBack?: () => void }) {
  const runtime = useTaskRuntime();
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [connected, setConnected] = useState(false);
  const [whatsappConnected, setWhatsappConnected] = useState(false);
  const [wechatConnected, setWechatConnected] = useState(false);
  const [customInterval, setCustomInterval] = useState(false);
  const [editingSharedPlan, setEditingSharedPlan] = useState(false);
  const [unitPref, setUnitPref] = useState<'auto' | '分钟' | '小时'>('auto');
  const [newTime, setNewTime] = useState('12:00');
  const [error, setError] = useState('');
  const [ready, setReady] = useState(false);
  const [loading, setLoading] = useState(true);
  const [blocked, setBlocked] = useState(false);
  const [savePending, setSavePending] = useState(false);
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
        invoke<ChatAccount[]>('chat_accounts').catch(() => []),
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
        setSavePending(state.saving || state.pending > 0);
        if (state.error) setError(tf("保存失败，修改尚未确认：{0}", String(state.error)));
      });
      setConnected((scan ?? []).some(account => !wizard || account.id === stored.active_account_id));
      setWhatsappConnected((scan ?? []).some(account => account.source === 'whatsapp'));
      setWechatConnected((scan ?? []).some(account => account.source !== 'whatsapp'));
      setError('');
      setReady(true);
    } catch (e) {
      if (active.current && request === loadId.current) setError(tf("无法加载设置：{0}", String(e)));
    } finally {
      if (active.current && request === loadId.current) setLoading(false);
    }
  }, [wizard]);

  useEffect(() => {
    active.current = true;
    void load();
    return () => { active.current = false; loadId.current += 1; unsubscribe.current?.(); };
  }, [load]);

  const plan = settings.backup;
  const intervalSeconds = settings.chat.sync_interval_seconds;
  const showCustomInterval = customInterval || !QUICK_INTERVALS.some(item => item.seconds === intervalSeconds);

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

  const windowHint = plan.window_start > plan.window_end
    ? tf("跨午夜：{0} → 次日 {1}", plan.window_start, plan.window_end)
    : `${plan.window_start} – ${plan.window_end}`;
  const windowRule = plan.window_mode === 'deny'
    ? t("，该时段暂停上传，结束后自动补传")
    : t("，其余时间攒着，到点集中上传");
  const scheduleSummary = plan.mode === 'manual' ? t("仅手动") : plan.mode === 'times'
    ? tf("每天 {0}", plan.times.join('、'))
    : intervalSeconds % 3600 === 0 ? tf("每 {0} 小时", intervalSeconds / 3600) : tf("每 {0} 分钟", intervalSeconds / 60);

  if (!ready) return <div className="settings-page backup-page backup-v7"><h2>{t("备份周期")}</h2><p>{loading ? t("正在加载设置…") : t("设置尚未加载，无法安全保存。")}</p>{error && <div className="error" role="alert">{error}</div>}{!loading && <button type="button" onClick={() => void load()}>{t("重新加载")}</button>}</div>;

  return <div className={`settings-page backup-page backup-v7${wizard ? ' wizard-backup-page' : ''}`}>
    <header className="page-head">
      <h2>{wizard ? t("确认备份周期") : t("备份周期")}</h2>
      <p className="help">{wizard ? t("所有平台共用一套备份设置，默认沿用当前设置，无需重复修改。") : t("选择聊天记录自动备份的时间与范围，只上传已导入会话的新增消息。")}</p>
    </header>

    <fieldset disabled={blocked} style={{ border: 0, margin: 0, padding: 0, minWidth: 0 }}>
    {wizard && !connected && <div role="alert"><p>{t("未能确认当前账号的连接状态，请重新检查或返回连接步骤。")}</p><button type="button" onClick={() => void load()}>{t("重新检查")}</button><button type="button" onClick={onOpenConnectors}>{t("返回连接步骤")}</button></div>}
    {wizard && connected && <div className="wizard-shared-plan">
      <div className="wizard-shared-plan-head"><strong>{t("当前备份设置")}</strong><button type="button" className="wxv-btn" aria-expanded={editingSharedPlan} aria-controls="wizard-shared-plan-editor" disabled={savePending} onClick={() => setEditingSharedPlan(value => !value)}>{editingSharedPlan ? t("收起设置") : t("修改统一设置")}</button></div>
      {!editingSharedPlan && <dl className="wizard-shared-plan-summary">
        <div><dt>{t("备份周期")}</dt><dd>{scheduleSummary}</dd></div>
        <div><dt>{t("上传内容")}</dt><dd>{t(ATTACHMENTS.find(item => item.id === plan.attachments)?.label ?? '仅文本')}</dd></div>
        {plan.wifi_only && <div><dt>{t("网络条件")}</dt><dd>{t("仅在 Wi-Fi 下上传")}</dd></div>}
        {plan.window_enabled && <div><dt>{t("限制上传时间")}</dt><dd>{windowHint}{windowRule}</dd></div>}
      </dl>}
      {(!settings.sync.enabled || runtime.paused) && <p className="help">{!settings.sync.enabled ? t("自动备份已关闭。") : t("后台备份已暂停，重启后仍保持暂停。")}</p>}
      {editingSharedPlan && <p className="wizard-shared-plan-notice">{t("以下修改会立即保存，并应用于所有已连接的平台。")}</p>}
    </div>}
    {(!wizard || editingSharedPlan) && <div id={wizard ? 'wizard-shared-plan-editor' : undefined}>
    <div className="backup-list">
      {connected && <section className="plan-plat">
        <div className="plan-rows plan-master">
        {wizard ? <Row label={t("自动备份")} desc={t("关闭后，所有平台暂停自动生成备份任务")}><Switch checked={settings.sync.enabled} onChange={next => update(current => ({ ...current, sync: { ...current.sync, enabled: next } }))} label={t("所有平台自动备份")} /></Row> :
          <Row label={t("后台备份")} desc={runtime.paused ? t("后台备份已暂停，重启后仍保持暂停。") : t("已连接的平台按统一周期自动备份。") }>
            <BackupRuntimeSwitch onError={setError}/>
          </Row>}
        </div>
        <fieldset disabled={wizard && !settings.sync.enabled} className="plan-rows" style={{ border: 0, margin: 0, minWidth: 0 }}>
          <Row label={t("备份方式")}>
            <Pills label={t("备份方式")} value={plan.mode} options={MODES} onChange={mode => setPlan({ mode })}/>
          </Row>
          {plan.mode === 'interval' && <>
            <Row label={t("快速选择")}>
              <Pills<string | number> label={t("常用间隔")} value={showCustomInterval ? 'custom' : intervalSeconds} options={[...QUICK_INTERVALS.map(item => ({ id: item.seconds as string | number, label: item.label })), { id: 'custom', label: t('自定义') }]} onChange={value => { setCustomInterval(value === 'custom'); if (typeof value === 'number') setInterval(value); }}/>
            </Row>
            {showCustomInterval && <Row label={t("自定义间隔")}>
              <div className="plan-step">
                <button type="button" aria-label={t("减少")} onClick={() => step(-1)} disabled={stepValue <= 1}>−</button>
                <b>{stepValue}</b>
                <button type="button" aria-label={t("增加")} onClick={() => step(1)}>＋</button>
                <button type="button" className="plan-unit" onClick={switchUnit} title={t("切换分钟 / 小时")}>{unit}</button>
              </div>
            </Row>}
          </>}
          {plan.mode === 'times' && <Row label={t("备份时间")} desc={t("可添加多个，精确到分钟")}>
            <div className="plan-times">
              {plan.times.map(time => <span key={time} className="plan-chip">{time}
                <button
                  type="button"
                  aria-label={tf("删除 {0}", time)}
                  disabled={plan.times.length <= 1}
                  onClick={() => setPlan({ times: plan.times.filter(value => value !== time) })}
                >×</button>
              </span>)}
              <input
                type="time"
                aria-label={t("备份时间")}
                value={newTime}
                onChange={event => setNewTime(event.target.value)}
              />
              <button
                type="button"
                className="plan-add"
                onClick={() => {
                  if (newTime && !plan.times.includes(newTime)) setPlan({ times: [...plan.times, newTime].sort() });
                }}
              >{t("添加")}</button>
            </div>
          </Row>}
          <Row label={t("限制上传时间")} desc={t("只在允许的时间段上传，或避开某个时间段")}>
            <Switch checked={plan.window_enabled} onChange={window_enabled => setPlan({ window_enabled })} label={t("限制上传时间")}/>
          </Row>
          {plan.window_enabled && <>
            <Row label={t("限制方式")}>
              <Pills
                label={t("限制方式")}
                value={plan.window_mode}
                options={[{ id: 'deny' as const, label: t("此时段内不上传") }, { id: 'allow' as const, label: t("只在此时段上传") }]}
                onChange={window_mode => setPlan({ window_mode })}
              />
            </Row>
            <Row label={t("时间段")} desc={`${windowHint}${windowRule}`}>
              <div className="plan-window">
                <input type="time" aria-label={t("开始时间")} value={plan.window_start} onChange={event => { if (event.target.value) setPlan({ window_start: event.target.value }); }}/>
                {t("至")}
                <input type="time" aria-label={t("结束时间")} value={plan.window_end} onChange={event => { if (event.target.value) setPlan({ window_end: event.target.value }); }}/>
              </div>
            </Row>
            <Row label={t("生效日")}>
              <div className="plan-pills" role="group" aria-label={t("生效日")}>
                {WEEKDAYS.map((label, day) => <button
                  key={t(label)}
                  type="button"
                  aria-pressed={plan.days.includes(day)}
                  className={plan.days.includes(day) ? 'on' : ''}
                  onClick={() => setPlan({
                    days: plan.days.includes(day)
                      ? (plan.days.length > 1 ? plan.days.filter(value => value !== day) : plan.days)
                      : [...plan.days, day].sort((a, b) => a - b),
                  })}
                >{t(label)}</button>)}
              </div>
            </Row>
          </>}
          <Row label={t("仅在 Wi-Fi 下上传")}>
            <Switch checked={plan.wifi_only} onChange={wifi_only => setPlan({ wifi_only })} label={t("仅在 Wi-Fi 下上传")}/>
          </Row>
          <Row label={t("附件")}><Pills label={t("上传内容")} value={plan.attachments} options={ATTACHMENTS} onChange={attachments => setPlan({ attachments })}/></Row>
        </fieldset>
      </section>}

      {!wizard && <>
      {(!wechatConnected || !whatsappConnected) && <div className="plan-off-head">{t("未连接")}</div>}
      {!wechatConnected && <div className="plan-off">
        <span className="plan-mark dim"><img className="backup-source-logo" src={wechatIcon} alt="" /></span>
        <div><strong>{t("微信")}</strong><small>{t("连接后按统一周期自动备份")}</small></div>
        <button type="button" onClick={onOpenConnectors}>{t("打开连接向导")}</button>
      </div>}
      {!whatsappConnected && <div className="plan-off">
        <span className="plan-mark dim"><img className="backup-source-logo" src={whatsappIcon} alt="" /></span>
        <div><strong>WhatsApp</strong><small>{t("连接后按统一周期自动备份")}</small></div>
        <button type="button" onClick={onOpenConnectors}>{t("打开连接向导")}</button>
      </div>}
      </>}
    </div>
    </div>}

    </fieldset>
    {onContinue && <div className="page-actions">{wizard && onBack && <button type="button" className="wxv-btn" disabled={savePending} onClick={onBack}>{t("上一步")}</button>}<WizardActionButton className="primary" loading={savePending} busyLabel={t("正在保存…")} disabled={!connected || (!wizard && (!settings.sync.enabled || plan.mode === 'manual')) || blocked || savePending} onAction={onContinue}>{wizard ? editingSharedPlan ? t("下一步") : t("沿用当前设置并继续") : t("保存并继续")}</WizardActionButton>{!wizard && (!settings.sync.enabled || plan.mode === 'manual') && <span>{t("请开启自动备份并选择备份周期。")}</span>}</div>}
    {error && <div className="error" role="alert">{error}</div>}
    {blocked && <div><p>{t("当前修改未确认保存；重新加载会放弃这些修改。")}</p><button type="button" disabled={loading} onClick={() => void load()}>{loading ? t("正在加载…") : t("放弃未保存修改并重新加载")}</button></div>}
  </div>;
}
