import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { t, tf } from '../../shared/i18n/locale';
import { WizardActionButton } from '../../shared/ui/WizardActionButton';
import { AGENTS, type AccessStatus, type AgentInfo } from './agentCatalog';

const EXAMPLES = [
  '通过 AIMyChats 读取「产品讨论群」昨天的聊天，整理结论、事情经过和待办。',
  '通过 AIMyChats 梳理「项目进度群」最近 7 天的进展，保留原文依据。',
];
export function AgentAccess({ status, agents, defaultId, disabled, error, onConfigure, onRefresh }: {
  status: AccessStatus | null; agents: AgentInfo[]; defaultId: string | null; disabled: boolean; error: string;
  onConfigure: (id: string) => void; onRefresh: () => Promise<void>;
}) {
  const [installing, setInstalling] = useState<string | null>(null);
  const [message, setMessage] = useState('');
  const [failure, setFailure] = useState('');
  const lock = useRef(false);
  const active = useRef(true);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  const install = async (id: string) => {
    if (lock.current) return;
    lock.current = true; setInstalling(id); setMessage(''); setFailure('');
    try {
      const result = await invoke<{ legacy_preserved?: string[] }>('agent_access_install', { tool: id });
      if (!active.current) return;
      await onRefresh(); if (!active.current) return; setMessage(tf('{0} 的 Skills 已安装。请重新打开该智能体的会话后使用。', AGENTS.find(item => item.id === id)!.name));
      if (result.legacy_preserved?.length) setFailure(tf('新版 Skills 已安装，旧 Skills 含有自定义内容或无法迁移，已保留：{0}。请确认后自行停用旧版本，避免重复显示。', result.legacy_preserved.join('、')));
    } catch (reason) { if (active.current) setFailure(String(reason)); }
    finally { lock.current = false; if (active.current) setInstalling(null); }
  };
  const copy = async (text: string) => {
    try { await navigator.clipboard.writeText(t(text)); setMessage(t('已复制，可粘贴到你的智能体对话中。')); }
    catch { setFailure(t('复制失败，请选中文字手动复制。')); }
  };
  const installed = (id: string) => agents.some(item => item.id === id && item.available) || Boolean(status?.targets.find(item => item.id === id)?.available);
  return <section className="integration-skills" aria-labelledby="integration-skills-title">
    <div className="integration-section-heading"><h3 id="integration-skills-title"><span>3</span>{t('让智能体从本地读取聊天记录')}</h3><small>{t('按需接入')}</small></div>
    <p>{t('为智能体安装 Skills，直接通过本机 AIMyChats 读取聊天记录，继续总结、分析或处理。')}</p>
    <p className="integration-help">{t('本机接入，优先本地读取。无需开启上方的总结开关，后续处理由你的智能体完成。')}</p>
    {error && <p className="integration-error" role="alert">{error} <button className="integration-text" type="button" onClick={() => void onRefresh()}>{t('重试')}</button></p>}
    {status && !status.running && <p className="integration-error" role="alert">{t('本机接入暂不可用，请确认已登录并重新打开智能体集成。')}</p>}
    {status && !status.wxq && <p className="integration-error" role="alert">{t('缺少本机读取工具，请重新安装完整版 AIMyChats。')}</p>}
    <div className="integration-directory-heading"><h4>{t('选择智能体，安装 Skills')}</h4><small>{tf('已安装 {0} / {1}', AGENTS.filter(item => installed(item.id)).length, AGENTS.length)}</small></div>
    <div className="integration-directory">{AGENTS.map(tool => {
      const target = status?.targets.find(item => item.id === tool.id);
      const complete = Boolean(status?.skills.length && target && status.skills.every(skill => target.installed.includes(skill.name)));
      const found = installed(tool.id);
      return <article key={tool.id} className={`integration-tool${defaultId === tool.id ? ' is-default' : ''}`}>
        <div className="integration-tool-head"><img src={tool.icon} alt=""/><div><strong>{tool.name}</strong><small className={found ? 'found' : ''}>
          {found && <svg className="integration-check" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="m4 10 4 4 8-8"/></svg>}{t(found ? '已安装' : '未安装')}
          {!tool.callable && <span className="integration-pending">{t('调用待适配')}</span>}{defaultId === tool.id && <span className="integration-default">{t('默认')}</span>}
        </small></div></div>
        <div className="integration-tool-actions"><WizardActionButton className={complete ? 'secondary' : 'primary'} disabled={disabled || Boolean(installing) || !target?.available || !status?.running || !status?.wxq || Boolean(error)} loading={installing === tool.id}
          busyLabel={t('安装中…')} onAction={() => install(tool.id)}>{t(complete ? '更新 Skills' : '安装 Skills')}</WizardActionButton>
          <button type="button" className="integration-text" onClick={() => onConfigure(tool.id)} disabled={disabled || Boolean(installing)}>{t('配置')}</button></div>
        {complete && <span className="integration-installed">{t('Skills 已安装')}</span>}
      </article>;
    })}</div>
    {message && <p className="integration-success" role="status">{message}</p>}{failure && <p className="integration-error" role="alert">{failure}</p>}
    <div className="integration-howto"><div className="integration-directory-heading"><h4>{t('怎么用')}</h4><small>{t('安装 Skills 后，在你的智能体里直接说')}</small></div>
      <ol><li>{t('为已安装的智能体安装 Skills')}</li><li>{t('打开它的对话窗口')}</li><li>{t('说出会话名称和时间范围')}</li></ol>
      <div className="integration-examples">{EXAMPLES.map(example => <button type="button" key={example} onClick={() => void copy(example)}>{t(example)}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" aria-label={t('复制')}><rect x="8" y="8" width="12" height="13" rx="2"/><path d="M16 8V3H3v13h5"/></svg></button>)}</div>
      <p className="integration-help">{t('保持 AIMyChats 打开并登录。Skills 仅在本机使用，只读取允许访问的会话。')}</p>
    </div>
  </section>;
}
