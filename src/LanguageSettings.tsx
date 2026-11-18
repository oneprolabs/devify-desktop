import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { type AppSettings, defaultSettings, mergeSettings } from './Settings';
import { applyLanguage, t, tf, ctf } from './locale';
import { createSettingsSaveQueue, settingsSaveFailure, settingsUserRevision, type SettingsSaveQueue } from './settingsSaveQueue';

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

  if (!ready) return <div className="settings-page language-settings"><h2>{t("语言")}</h2><p>{loading ? t("正在加载设置…") : t("设置尚未加载，无法安全保存。")}</p>{error && <div className="error" role="alert">{t(error)}</div>}{!loading && <button type="button" onClick={() => void load()}>{t("重新加载")}</button>}</div>;

  return <div className="settings-page language-settings">
    <header className="page-head">
      <h2>{t("语言")}</h2>
      <p className="help">{t("界面语言和 AI 输出语言分开设置。聊天原文不会被翻译或改动。")}</p>
    </header>

    <fieldset disabled={blocked} style={{ border: 0, margin: 0, padding: 0, minWidth: 0 }}>
    <p className="language-explanation">{t("设置应用菜单、按钮和提示的显示语言。聊天原文不会被翻译。")}</p>
    <h3 className="language-heading">{t("界面语言")}</h3>
    <div className="language-cards" role="group" aria-label={t("界面语言")}>{UI_OPTIONS.map(option => <button type="button" key={t(option.label)} className={settings.language.ui === option.label ? 'selected' : ''} aria-pressed={settings.language.ui === option.label} disabled={option.disabled} onClick={() => setLanguage({ ui: option.label })}><span className="language-mark">{option.label === '中文' ? '简' : 'EN'}</span><span><strong>{option.label === '中文' ? t("简体中文") : option.label}</strong><small>{t(option.hint || "界面以简体中文显示")}</small></span><span className="language-check">{settings.language.ui === option.label ? '✓' : '○'}</span></button>)}</div>
    <h3 className="language-heading">{t("内容语言")}</h3>
    <div className="plan-items">

      <div className="plan-row">
        <div className="plan-row-label">
          <span>{t("总结与报告语言")}</span>
          <small>{t("事件、聊天总结、报告的输出语言。「按原文语言」会让中文聊天出中文、英文聊天出英文")}</small>
        </div>
        <div className="plan-row-control">
          <Pills label={t("总结与报告语言")} options={SUMMARY_OPTIONS} value={settings.language.summary} onChange={summary => setLanguage({ summary })}/>
        </div>
      </div>

      <div className="plan-row">
        <div className="plan-row-label">
          <span>{t("问答回答语言")}</span>
          <small>{t("问答的回复语言")}</small>
        </div>
        <div className="plan-row-control">
          <span className="plan-kv">{t("由服务端生成，桌面端不单独设置")}</span>
        </div>
      </div>
    </div>

    <p className="language-note">{t("本机生成的总结与日报会按「总结与报告语言」书写。聊天原文保持原样，不会翻译。")}</p>
    </fieldset>
    {error && <div className="error" role="alert">{t(error)}</div>}
    {blocked && <div><p>{t("当前修改未确认保存；重新加载会放弃这些修改。")}</p><button type="button" disabled={loading} onClick={() => void load()}>{loading ? t("正在加载…") : t("放弃未保存修改并重新加载")}</button></div>}
  </div>;
}
