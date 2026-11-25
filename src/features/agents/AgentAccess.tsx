import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { t, tf } from '../../shared/i18n/locale';
import { errorText } from '../../shared/i18n/errorText';
import { WizardActionButton } from '../../shared/ui/WizardActionButton';
import { ACCESS_AGENTS, type AccessStatus } from './agentCatalog';
import { AgentUsage } from './AgentUsage';
import { copyInstallationPrompt } from './agentInstallationPrompt';

export function AgentAccess({ status, disabled, error, onConfigure, onRefresh }: {
  status: AccessStatus | null; disabled: boolean; error: string;
  onConfigure: (id: string) => void; onRefresh: () => Promise<void>;
}) {
  const [installing, setInstalling] = useState<string | null>(null);
  const [copying, setCopying] = useState(false);
  const [message, setMessage] = useState('');
  const [copyMessage, setCopyMessage] = useState('');
  const [failure, setFailure] = useState('');
  const [copyFailure, setCopyFailure] = useState('');
  const lock = useRef(false);
  const active = useRef(true);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  const install = async (id: string) => {
    if (lock.current) return;
    lock.current = true; setInstalling(id); setMessage(''); setFailure('');
    try {
      const result = await invoke<{ legacy_preserved?: string[] }>('agent_access_install', { tool: id });
      if (!active.current) return;
      await onRefresh();
      if (!active.current) return;
      setMessage(tf('{0} 的 Skills 已安装。请在该智能体中新建会话后使用；连接是否成功仍需实际验证。', ACCESS_AGENTS.find(item => item.id === id)!.name));
      if (result.legacy_preserved?.length) setFailure(tf('新版 Skills 已安装，旧 Skills 含有自定义内容或无法迁移，已保留：{0}。请确认后自行停用旧版本，避免重复显示。', result.legacy_preserved.join('、')));
    } catch (reason) { if (active.current) setFailure(String(reason)); }
    finally { lock.current = false; if (active.current) setInstalling(null); }
  };
  const copy = async () => {
    if (lock.current) return;
    lock.current = true; setCopying(true); setCopyMessage(''); setCopyFailure('');
    try {
      await copyInstallationPrompt();
      if (active.current) setCopyMessage(t('已复制安装提示词。粘贴到你的智能体，按引导完成安装。'));
    } catch (reason) { if (active.current) setCopyFailure(tf('未能复制安装提示词，请重试：{0}', errorText(reason))); }
    finally { lock.current = false; if (active.current) setCopying(false); }
  };
  const unavailable = disabled || Boolean(installing) || copying || !status?.running || !status?.wxq || Boolean(error);
  return <section className="integration-skills" aria-labelledby="integration-skills-title">
    <div className="integration-section-heading"><h3 id="integration-skills-title">{t('使用智能体访问 AIMyChats')}</h3></div>
    <p>{t('通过一个 Skill 读取获准的聊天、待办、已生成的日报／周报／月报和主题报告。')}</p>
    <p className="integration-help">{t('仅读取已有结果，不会生成报告、完成待办或修改订阅。云端结果属于远程读取。')}</p>
    {error && <p className="integration-error" role="alert">{errorText(error)} <button className="integration-text" type="button" disabled={disabled} onClick={() => void onRefresh()}>{t('重试')}</button></p>}
    {status && !status.running && <p className="integration-error" role="alert">{t('本机接入暂不可用，请确认已登录并重新打开智能体集成。')}</p>}
    {status && !status.wxq && <p className="integration-error" role="alert">{t('缺少本机读取工具，请重新安装完整版 AIMyChats。')}</p>}
    <div className="integration-directory">{ACCESS_AGENTS.map(tool => {
      const target = status?.targets.find(item => item.id === tool.id);
      const complete = Boolean(status?.skills.length && target && status.skills.every(skill => target.installed.includes(skill.name)));
      const found = Boolean(target?.available);
      return <article key={tool.id} className="integration-tool" aria-label={tool.name}>
        <div className="integration-tool-head"><img src={tool.icon} alt=""/><div><strong>{tool.name}</strong><small className={found ? 'found' : ''}>
          {found && <svg className="integration-check" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="m4 10 4 4 8-8"/></svg>}
          {t(!target ? '检测中…' : found ? target.desktop_app && !target.command ? '已安装客户端' : '已安装 CLI' : '未检测到安装')}
        </small></div></div>
        <p className="integration-skill-state">{t(complete ? 'Skills 已安装 · 连接待验证' : 'Skills 尚未安装')}</p>
        <div className="integration-tool-actions"><WizardActionButton className={complete ? 'secondary' : 'primary'} disabled={unavailable || !found} loading={installing === tool.id}
          busyLabel={t('安装中…')} onAction={() => install(tool.id)}>{t(complete ? '更新 Skills' : '安装 Skills')}</WizardActionButton>
          <details className="integration-location"><summary>{t('安装位置')}</summary><div><p>{target?.skills_dir || t('尚未读取到安装目录')}</p>
            <button type="button" className="integration-text" onClick={() => onConfigure(tool.id)} disabled={disabled || Boolean(installing) || copying}>{t('修改安装目录')}</button></div></details></div>
      </article>;
    })}</div>
    {message && <p className="integration-success" role="status">{message}</p>}{failure && <p className="integration-error" role="alert">{errorText(failure)}</p>}
    <div className="integration-generic">
      <p>{t('使用其他智能体？复制通用安装提示词，交给它自行识别目录并安装。')}</p>
      <div className="integration-actions"><button type="button" className="secondary" disabled={unavailable} aria-busy={copying} onClick={() => void copy()}>{t(copying ? '正在准备…' : '复制通用安装提示词')}</button><span>{t('粘贴到你的智能体，按引导完成安装。')}</span></div>
      {copyMessage && <p className="integration-success" role="status">{copyMessage}</p>}{copyFailure && <p className="integration-error" role="alert">{copyFailure}</p>}
      <p className="integration-help">{t('Skills 连接本机 AIMyChats，使用时请保持应用打开并登录。')}</p>
      <p className="integration-help">{t('安装成功表示文件已就位；实际访问仍由当前智能体的本机权限决定。')}</p>
    </div>
    <AgentUsage skillName={status?.skills[0]?.name}/>
  </section>;
}
