import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { t } from '../../shared/i18n/locale';
import { WizardActionButton } from '../../shared/ui/WizardActionButton';
import { mergeSettings, requireSettingsRevision, settingsSaveError, type AppSettings } from '../settings/settingsModel';

/** Shared prompt preference only. Agent discovery and paths belong to AgentHub. */
export function AgentSummaryInstructions({ settings, onSaved }: { settings: AppSettings; onSaved: (settings: AppSettings) => void }) {
  const [text, setText] = useState(settings.agent.instructions);
  const [savedText, setSavedText] = useState(settings.agent.instructions);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  return <div className="analysis-summary-instructions"><label>{t('总结补充要求')} <small>{t('可选')}</small>
    <textarea rows={3} maxLength={2000} value={text} onChange={event => { setText(event.target.value); setMessage(''); }} placeholder={t('例如：重点关注交付结论、负责人和截止时间；忽略闲聊。')}/></label>
    <p className="help">{t('所有本地智能体共用，在内置总结要求后追加；仍以原文为依据，不编造信息。')}</p>
    <WizardActionButton className="secondary" disabled={text === savedText} busyLabel={t('正在保存…')} onAction={async () => {
      setError(''); setMessage(''); const submitted = text;
      try {
        const saved = mergeSettings(requireSettingsRevision(await invoke<AppSettings>('app_settings_save', { value: { ...settings, agent: { ...settings.agent, instructions: submitted } } })));
        if (!mounted.current) return;
        onSaved(saved); setSavedText(submitted); setMessage(t('总结补充要求已保存。'));
      } catch (reason) { if (mounted.current) setError(settingsSaveError(reason)); }
    }}>{t('保存补充要求')}</WizardActionButton>
    {message && <p role="status">{message}</p>}{error && <p role="alert" className="error">{error}</p>}
  </div>;
}
