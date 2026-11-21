import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { t } from '../../shared/i18n/locale';
import { WizardActionButton } from '../../shared/ui/WizardActionButton';
import type { AppSettings } from '../settings/settingsModel';

type VoiceStatus = { settings: { engine: string; model: string; language: string }; engines: { id: string; label: string; available: boolean }[]; selected: string | null };
export function AgentVoiceSettings({ settings, saving, onSave }: { settings: AppSettings; saving: boolean; onSave: (edit: (value: AppSettings) => AppSettings) => Promise<boolean> }) {
  const [open, setOpen] = useState(false);
  const [voice, setVoice] = useState<VoiceStatus | null>(null);
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  const [refresh, setRefresh] = useState(0);
  const [detecting, setDetecting] = useState(false);
  useEffect(() => {
    if (!open) return;
    let active = true; setDetecting(true); setError('');
    void invoke<VoiceStatus>('wechat_voice_status').then(next => { if (active) setVoice(next); })
      .catch(reason => { if (active) setError(String(reason)); }).finally(() => { if (active) setDetecting(false); });
    return () => { active = false; };
  }, [open, refresh]);
  return <details className="integration-voice" open={open} onToggle={event => setOpen(event.currentTarget.open)}>
    <summary><span>{t('语音转写')} <small>{t('可选')}</small></span><span>{t('把微信语音转成文字')}</span></summary>
    {open && <div className="integration-voice-body"><p className="integration-help">{t('使用本机转写引擎，文字按账号和消息缓存。语音文件是否上传由备份附件设置决定。')}</p>
      {([['auto_transcribe', '上传前自动转写新语音'], ['upload_transcripts', '上传转写文本']] as const).map(([key, label]) => <div className="integration-voice-row" key={key}><span>{t(label)}</span><button type="button" role="switch" className="integration-switch" aria-label={t(label)} aria-checked={settings.voice[key]} disabled={saving}
        onClick={() => void onSave(value => ({ ...value, voice: { ...value.voice, [key]: !value.voice[key] } }))}><span/></button></div>)}
      <div className="integration-voice-row"><span>{detecting ? t('正在检测转写引擎…') : voice?.selected ? `${t('转写引擎')} · ${voice.selected}` : t('未检测到可用转写引擎')}</span><button type="button" className="secondary" disabled={detecting} onClick={() => setRefresh(value => value + 1)}>{t('重新检测')}</button></div>
      {voice && <details><summary>{t('高级配置')}</summary><div className="integration-config-grid">
        <label>{t('转写引擎')}<select value={voice.settings.engine} onChange={event => setVoice({ ...voice, settings: { ...voice.settings, engine: event.target.value } })}><option value="">{t('自动选择')}</option>{voice.engines.map(engine => <option key={engine.id} value={engine.id} disabled={!engine.available}>{engine.label}</option>)}</select></label>
        <label>{t('模型 / 模型路径')}<input value={voice.settings.model} onChange={event => setVoice({ ...voice, settings: { ...voice.settings, model: event.target.value } })}/></label>
        <label>{t('语言')}<input value={voice.settings.language} onChange={event => setVoice({ ...voice, settings: { ...voice.settings, language: event.target.value } })} placeholder={t('自动识别')}/></label>
      </div><WizardActionButton className="primary" busyLabel={t('正在保存…')} onAction={async () => {
        setError(''); setMessage(''); try { setVoice(await invoke<VoiceStatus>('wechat_voice_settings_save', voice.settings)); setMessage(t('转写引擎设置已保存。')); } catch (reason) { setError(String(reason)); }
      }}>{t('保存转写设置')}</WizardActionButton></details>}
      {message && <p className="integration-success" role="status">{message}</p>}{error && <p className="integration-error" role="alert">{error}</p>}
    </div>}
  </details>;
}
