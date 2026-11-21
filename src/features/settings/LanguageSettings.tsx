import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { type AppSettings, defaultSettings, mergeSettings } from './settingsModel';
import { applyLanguage, t, tf, ctf } from '../../shared/i18n/locale';
import { createSettingsSaveQueue, settingsSaveFailure, settingsUserRevision, type SettingsSaveQueue } from '../../shared/state/settingsSaveQueue';

type Option = { label: string; disabled?: boolean; hint?: string };

const UI_OPTIONS: Option[] = [
  { label: '中文', hint: '界面以简体中文显示' },
  { label: 'English', hint: '界面以英文显示' },
];
const SUMMARY_OPTIONS: Option[] = [
  { label: '跟随界面' },
  { label: '中文' },
  { label: 'English' },
  { label: '按原文语言' },
];

function Pills({ options, value, onChange, label }: { options: Option[]; value: string; onChange: (next: string) => void; label: string }) {
  return <div className="plan-pills" role="group" aria-label={t(label)}>
    {options.map(option => <button
      key={t(option.label)}
      type="button"
      aria-pressed={option.label === value}
      className={option.label === value ? 'on' : ''}
      disabled={option.disabled}
      title={option.hint ? t(option.hint) : undefined}
      onClick={() => onChange(option.label)}
    >{t(option.label)}</button>)}
  </div>;
}

export function LanguageSettings() {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
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
      const stored = await invoke<AppSettings>('app_settings_get');
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
        if (state.blocked) setError(settingsSaveFailure(state.error));
      });
      setError('');
      setReady(true);
      applyLanguage(mergeSettings(stored).language);
    } catch (e) {
      if (active.current && request === loadId.current) setError(tf("无法加载设置：{0}", String(e)));
    } finally {
      if (active.current && request === loadId.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    active.current = true;
    void load();
    return () => { active.current = false; loadId.current += 1; unsubscribe.current?.(); };
  }, [load]);

  const update = useCallback((change: (current: AppSettings) => AppSettings) => {
    queue.current?.enqueue(change);
  }, []);

  const setLanguage = (patch: Partial<AppSettings['language']>) => update(current => {
    const language = { ...current.language, ...patch };
    applyLanguage(language);
    return { ...current, language };
  });

  if (!ready) return <div className="settings-page settings-refreshed language-settings"><h2>{t("语言")}</h2><p>{loading ? t("正在加载设置…") : t("设置尚未加载，无法安全保存。")}</p>{error && <div className="error" role="alert">{t(error)}</div>}{!loading && <button type="button" onClick={() => void load()}>{t("重新加载")}</button>}</div>;

  return <div className="settings-page settings-refreshed language-settings">
    <header className="page-head">
      <h2>{t("语言")}</h2>
      <p className="help">{t("设置界面和本机总结的语言。聊天原文保持不变。")}</p>
    </header>

    <fieldset disabled={blocked} className="settings-fields">
    <section className="settings-group" aria-label={t('界面语言')}>
      <div className="settings-row">
        <div className="settings-row-label"><span>{t('界面语言')}</span><small>{t('菜单、按钮和提示的显示语言。')}</small></div>
        <Pills label="界面语言" options={UI_OPTIONS} value={settings.language.ui} onChange={ui => setLanguage({ ui })}/>
      </div>
    </section>
    <section className="settings-group" aria-label={t('内容语言')}>
      <div className="settings-row">
        <div className="settings-row-label"><span>{t('本机总结语言')}</span><small>{t('用于本机生成的聊天总结，可跟随界面或原文语言。')}</small></div>
        <Pills label="本机总结语言" options={SUMMARY_OPTIONS} value={settings.language.summary} onChange={summary => setLanguage({ summary })}/>
      </div>
      <div className="settings-row">
        <div className="settings-row-label"><span>{t('服务端内容语言')}</span><small>{t('事件、报告与问答由服务端生成，使用服务端的语言设置。')}</small></div>
        <span className="settings-value">{t('由服务端管理')}</span>
      </div>
    </section>
    <p className="settings-footnote">{t('更改后自动保存。')}</p>
    </fieldset>
    {error && <div className="error" role="alert">{t(error)}</div>}
    {blocked && <div><p>{t("当前修改未确认保存；重新加载会放弃这些修改。")}</p><button type="button" disabled={loading} onClick={() => void load()}>{loading ? t("正在加载…") : t("放弃未保存修改并重新加载")}</button></div>}
  </div>;
}
