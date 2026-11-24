import { useEffect, useId, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { t } from '../../shared/i18n/locale';
import { WizardActionButton } from '../../shared/ui/WizardActionButton';
import type { AppSettings } from '../settings/settingsModel';

type VoiceStatus = { settings: { engine: string; model: string; language: string }; engines: { id: string; label: string; available: boolean }[]; selected: string | null };
const chevron = <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="m5 7 5 5 5-5"/></svg>;
export function AgentVoiceSettings({ settings, saving, onSave }: { settings: AppSettings; saving: boolean; onSave: (edit: (value: AppSettings) => AppSettings) => Promise<boolean> }) {
  const id = useId();
  const [open, setOpen] = useState(false);
  const [voice, setVoice] = useState<VoiceStatus | null>(null);
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  const [refresh, setRefresh] = useState(0);
  const [detecting, setDetecting] = useState(false);
  const [engineSaving, setEngineSaving] = useState(false);
  const active = useRef(true);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  useEffect(() => {
    if (!open) return;
    let current = true; setDetecting(true); setError('');
    void invoke<VoiceStatus>('wechat_voice_status').then(next => { if (current) setVoice(next); })
      .catch(reason => { if (current) setError(String(reason)); }).finally(() => { if (current) setDetecting(false); });
    return () => { current = false; };
  }, [open, refresh]);
  const engineLabel = voice?.engines.find(engine => engine.id === voice.selected)?.label ?? voice?.selected;
  return <section className="integration-voice" aria-labelledby={`${id}-title`}>
    <div className="integration-section-heading"><h3 id={`${id}-title`}>{t('语音转写')}<small>{t('可选')}</small></h3>
      <button type="button" className="integration-disclosure" aria-expanded={open} aria-controls={`${id}-body`} disabled={engineSaving}
        onClick={() => setOpen(value => !value)}>{t(open ? '收起' : '展开')}{chevron}</button></div>
    <p>{t('把微信语音转成文字')}</p>
    {open && <div id={`${id}-body`} className="integration-voice-body"><p className="integration-help">{t('使用本机转写引擎，文字按账号和消息缓存。语音文件是否上传由备份附件设置决定。')}</p>
      {([['auto_transcribe', '上传前自动转写新语音'], ['upload_transcripts', '上传转写文本']] as const).map(([key, label]) => <div className="integration-voice-row" key={key}><span>{t(label)}</span><button type="button" role="switch" className="integration-switch" aria-label={t(label)} aria-checked={settings.voice[key]} disabled={saving}
        onClick={() => void onSave(value => ({ ...value, voice: { ...value.voice, [key]: !value.voice[key] } }))}><span/></button></div>)}
      <div className="integration-voice-row"><div><strong>{t('转写引擎')}</strong><p className="integration-help" role="status">{detecting ? t('正在检测转写引擎…') : engineLabel || t('未检测到可用转写引擎')}</p></div>
        <button type="button" className="secondary" disabled={detecting || engineSaving} onClick={() => setRefresh(value => value + 1)}>{t('重新检测')}</button></div>
      {voice && <details className="integration-voice-advanced"><summary>{t('转写引擎高级配置')}{chevron}</summary>
        <fieldset disabled={detecting || engineSaving}><div className="integration-config-grid">
          <label>{t('转写引擎')}<select value={voice.settings.engine} onChange={event => setVoice({ ...voice, settings: { ...voice.settings, engine: event.target.value } })}><option value="">{t('自动选择')}</option>{voice.engines.map(engine => <option key={engine.id} value={engine.id} disabled={!engine.available}>{engine.label}</option>)}</select></label>
          <label>{t('模型 / 模型路径')}<input value={voice.settings.model} onChange={event => setVoice({ ...voice, settings: { ...voice.settings, model: event.target.value } })} placeholder={t('留空使用引擎默认配置')}/></label>
          <label>{t('语言')}<input value={voice.settings.language} onChange={event => setVoice({ ...voice, settings: { ...voice.settings, language: event.target.value } })} placeholder={t('自动识别')}/></label>
        </div></fieldset>
        <div className="integration-voice-footer"><WizardActionButton className="primary" disabled={detecting || saving || engineSaving} busyLabel={t('正在保存…')} onAction={async () => {
          setError(''); setMessage(''); setEngineSaving(true);
          try { const next = await invoke<VoiceStatus>('wechat_voice_settings_save', voice.settings); if (active.current) { setVoice(next); setMessage(t('转写引擎设置已保存。')); } }
          catch (reason) { if (active.current) setError(String(reason)); }
          finally { if (active.current) setEngineSaving(false); }
        }}>{t('保存转写设置')}</WizardActionButton></div>
      </details>}
      {message && <p className="integration-success" role="status">{message}</p>}{error && <p className="integration-error" role="alert">{error}</p>}
    </div>}
  </section>;
}
