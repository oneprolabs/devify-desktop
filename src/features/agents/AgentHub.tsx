import { useRef, useState } from 'react';
import { t, tf } from '../../shared/i18n/locale';
import { WizardActionButton } from '../../shared/ui/WizardActionButton';
import { granted, useInternalAccount } from '../auth/entitlements';
import { AgentSelect } from './AgentSelect';
import { AgentAdvanced } from './AgentAdvanced';
import { AgentAccess } from './AgentAccess';
import { AgentVoiceSettings } from './AgentVoiceSettings';
import { useAgentIntegration } from './useAgentIntegration';
import { effectiveAgent, localSummaryEnabled, profileFor, selectAgent, type AgentProfile } from './agentCatalog';
import './agent-integration.css';

export function AgentHub({ onOpenAnalysis }: { onOpenAnalysis?: () => void }) {
  const { internal, view } = useInternalAccount();
  const canSummary = internal && granted(view, 'ai.local_agent');
  const canAccess = internal && granted(view, 'bridge.agent_cli');
  const model = useAgentIntegration(canAccess);
  const { settings, ready, saving, scanning, agents, access } = model;
  const [editing, setEditing] = useState<string | null>(null);
  const advanced = useRef<HTMLDivElement>(null);
  const automatic = effectiveAgent({ ...settings.agent, provider_id: '' }, agents);
  const effective = effectiveAgent(settings.agent, agents);
  const enabled = localSummaryEnabled(settings.agent);
  const selectedProfile = effective ? profileFor(settings.agent, effective) : null;
  const knownAgents = [...agents, ...(access?.targets ?? []).filter(target => !agents.some(item => item.id === target.id)).map(target => ({ ...target, version: null, note: '' }))];
  const locked = !ready || saving;
  const configure = (id: string) => { setEditing(id); window.requestAnimationFrame(() => advanced.current?.scrollIntoView({ block: 'nearest' })); };
  const saveProfile = async (id: string, profile: AgentProfile) => {
    const ok = await model.save(value => ({ ...value, agent: {
      ...value.agent, profiles: { ...value.agent.profiles, [id]: profile },
      ...(value.agent.provider_id === id ? { command: profile.command, model: profile.model, timeout_seconds: profile.timeout_seconds } : {}),
    } }));
    if (ok) await model.scan();
    return ok;
  };
  return <div className="settings-page agent-integration">
    <header className="page-head"><h2>{t('智能体集成')}</h2><p className="help">{t('从直接总结，到使用自己的智能体，按需选择。')}</p></header>
    <section className="integration-online" aria-labelledby="integration-online-title">
      <div className="integration-section-heading"><h3 id="integration-online-title"><span>1</span>{t('默认使用线上总结')}</h3><small className="integration-online-badge">{t('默认使用')}</small></div>
      <p>{t('在聊天中选择「生成总结」，使用 AIMyChats 提供的线上模型，无需安装智能体。')}</p>
    </section>
    {!ready && !model.error && <p role="status">{t('正在加载设置…')}</p>}
    {model.error && <p role="alert" className="integration-error">{model.error} <button type="button" className="integration-text" disabled={saving} onClick={() => void model.reload()}>{t('重新加载')}</button></p>}
    {ready && canSummary && <section className="integration-local" aria-labelledby="integration-local-title">
      <div className="integration-section-heading"><div><h3 id="integration-local-title"><span>2</span>{t('使用本地智能体总结')}</h3><p>{t('开启后，可在聊天中调用本机智能体，生成聊天记录总结。')}</p></div>
        <span className="integration-switch-state" role="status">{saving ? t('正在保存…') : enabled ? t(effective ? '已开启' : '等待可用工具') : t('未开启')}</span>
        <button type="button" role="switch" className="integration-switch" aria-label={t('启用本地智能体总结')} aria-checked={enabled} disabled={locked}
          onClick={() => void model.save(value => ({ ...value, agent: { ...value.agent, enabled: !enabled } }))}><span/></button>
      </div>
      <div className="integration-defaults">
        <AgentSelect value={settings.agent.provider_id} automatic={automatic} agents={knownAgents} disabled={locked || scanning || !agents.some(item => item.available)}
          onChange={id => void model.save(value => ({ ...value, agent: selectAgent(value.agent, id) }))}/>
        {effective && <label>{t('默认模型')}<select aria-label={t('默认模型')} value={selectedProfile?.model ? 'configured' : 'default'} disabled={locked}
          onChange={event => event.target.value === 'manual' ? configure(effective) : event.target.value === 'default' ? void saveProfile(effective, { ...selectedProfile!, model: '' }) : undefined}>
          <option value="default">{t('自动 · 沿用工具默认')}</option>{selectedProfile?.model && <option value="configured">{selectedProfile.model}</option>}<option value="manual">{t('手动指定…')}</option>
        </select></label>}
        <WizardActionButton className="secondary" disabled={scanning || locked} loading={scanning} busyLabel={t('扫描中…')} onAction={model.scan}>{t('扫描智能体')}</WizardActionButton>
        <button type="button" className="secondary" disabled={locked} onClick={() => configure(settings.agent.provider_id || effective || 'codex')}>{t('高级配置')}</button>
      </div>
      {model.scanError && <p className="integration-error" role="alert">{tf('扫描失败，已保留上次结果：{0}', model.scanError)}</p>}
      {!effective && !scanning && <div className="integration-empty"><strong>{t('未发现可用于总结的本地智能体')}</strong><p>{t('安装智能体后，点击「扫描智能体」。已安装但未识别？可在「高级配置」中指定路径。')}</p><p>{t('你仍可直接使用上方的线上总结。')}</p></div>}
      <p className="integration-help">{t('无需安装 Skills，沿用智能体已有的模型配置；也可以继续使用线上总结。')}</p>
      <p className="integration-help">{t('本地智能体也可能使用线上模型，聊天内容将按它自己的模型配置处理。')}</p>
    </section>}
    {ready && editing && <div ref={advanced}><AgentAdvanced key={editing} id={editing} agent={settings.agent} access={access} saving={saving} canSummarize={canSummary}
      onSelect={setEditing} onClose={() => setEditing(null)} onSave={saveProfile}/></div>}
    {ready && canAccess && <>
      {!canSummary && <div className="integration-actions"><WizardActionButton className="secondary" disabled={locked || scanning} loading={scanning} busyLabel={t('扫描中…')} onAction={model.scan}>{t('扫描智能体')}</WizardActionButton></div>}
      <AgentAccess status={access} agents={agents} defaultId={effective} disabled={locked || scanning} error={model.accessError} onConfigure={configure} onRefresh={model.refreshAccess}/>
    </>}
    {ready && !canSummary && !canAccess && <p>{t('本地智能体集成暂未向当前账户开放，线上总结仍可使用。')}</p>}
    {ready && canSummary && <div className="integration-analysis-link"><span>{t('总结补充要求统一在「智能分析」中设置。')}</span>{onOpenAnalysis && <button type="button" className="integration-text" onClick={onOpenAnalysis}>{t('前往智能分析')} →</button>}</div>}
    {model.notice && <p className="integration-success" role="status">{model.notice}</p>}
    {ready && canSummary && <AgentVoiceSettings settings={settings} saving={saving} onSave={model.save}/>}
  </div>;
}
