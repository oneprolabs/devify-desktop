import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { t, tf } from '../../shared/i18n/locale';
import { WizardActionButton } from '../../shared/ui/WizardActionButton';
import type { AppSettings } from '../settings/settingsModel';
import { AGENTS, agentById, profileFor, type AccessStatus, type AgentProfile } from './agentCatalog';

type Props = { id: string; agent: AppSettings['agent']; access: AccessStatus | null; saving: boolean; canSummarize: boolean;
  onSelect: (id: string) => void; onClose: () => void; onSave: (id: string, profile: AgentProfile) => Promise<boolean> };
export function AgentAdvanced({ id, agent, access, saving, canSummarize, onSelect, onClose, onSave }: Props) {
  const tool = agentById(id)!;
  const [draft, setDraft] = useState(() => profileFor(agent, id));
  const [models, setModels] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const active = useRef(true);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  const patch = (value: Partial<AgentProfile>) => { setDraft(current => ({ ...current, ...value })); setMessage(''); setError(''); };
  const target = access?.targets.find(item => item.id === id);
  const value = { ...agent, ...draft, enabled: true, provider_id: id };
  const run = async (action: () => Promise<string>) => {
    setBusy(true); setError(''); setMessage('');
    try { const text = await action(); if (active.current) setMessage(text); }
    catch (reason) { if (active.current) setError(String(reason)); }
    finally { if (active.current) setBusy(false); }
  };
  return <section className="integration-advanced" aria-labelledby="integration-config-title">
    <div className="integration-section-heading"><h3 id="integration-config-title"><img src={tool.icon} alt=""/>{tool.name} · {t('高级配置')}</h3>
      <button type="button" className="integration-text" onClick={onClose} disabled={busy || saving}>{t('收起配置')}</button></div>
    <fieldset disabled={busy || saving}><div className="integration-config-grid">
      <label>{t('智能体')}<select value={id} onChange={event => onSelect(event.target.value)}>{AGENTS.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</select></label>
      <label>{t('可执行文件路径')}<input value={draft.command} onChange={event => patch({ command: event.target.value })} disabled={!tool.callable}
        placeholder={t('留空自动识别')} spellCheck={false}/></label>
      <label>{t('默认模型')}<input value={draft.model} onChange={event => patch({ model: event.target.value })} disabled={!tool.callable}
        list="integration-models" placeholder={t('留空沿用智能体默认模型')} maxLength={128} spellCheck={false}/><datalist id="integration-models">{models.map(model => <option key={model} value={model}/>)}</datalist></label>
      <label>{t('Skills 安装目录')}<input value={draft.skills_dir} onChange={event => patch({ skills_dir: event.target.value })}
        placeholder={target?.skills_dir || t('留空使用智能体默认目录')} spellCheck={false}/></label>
      {tool.callable && <label>{t('执行超时（秒）')}<input type="number" min={30} max={900} value={draft.timeout_seconds} onChange={event => patch({ timeout_seconds: Number(event.target.value) })}/></label>}
    </div></fieldset>
    <p className="integration-help">{tool.callable ? t('路径可填写绝对路径或 ~/ 开头的路径。模型留空时沿用工具配置，执行超时为 30–900 秒。') : t('此智能体可接入 Skills；直接调用总结仍待适配。')}</p>
    <div className="integration-actions">
      <WizardActionButton className="primary" disabled={busy || saving} loading={saving} busyLabel={t('正在保存…')} onAction={async () => {
        if (!Number.isInteger(draft.timeout_seconds) || draft.timeout_seconds < 30 || draft.timeout_seconds > 900) { setError(t('执行超时请填写 30–900 秒。')); return; }
        setError(''); if (await onSave(id, draft)) setMessage(t('此智能体配置已保存。'));
      }}>{t('保存此工具设置')}</WizardActionButton>
      {tool.callable && canSummarize && <WizardActionButton className="secondary" disabled={busy || saving} busyLabel={t('测试中…')} onAction={() => run(async () => {
        const result = await invoke<{ output: string; elapsed_ms: number }>('agent_test', { value });
        return tf('测试成功，用时 {0} 秒：{1}', (result.elapsed_ms / 1000).toFixed(1), result.output.slice(0, 160));
      })}>{t('测试连接')}</WizardActionButton>}
      {id === 'opencode' && <WizardActionButton className="secondary" disabled={busy || saving} busyLabel={t('读取中…')} onAction={() => run(async () => {
        const list = await invoke<string[]>('agent_models', { value }); if (active.current) setModels(list);
        return list.length ? tf('已读取 {0} 个模型。', list.length) : t('未读取到模型，可手动填写或沿用默认配置。');
      })}>{t('刷新模型')}</WizardActionButton>}
    </div>
    {message && <p className="integration-success" role="status">{message}</p>}{error && <p className="integration-error" role="alert">{error}</p>}
  </section>;
}
