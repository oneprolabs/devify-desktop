import { t, tf } from './locale';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { detectTitleFor, useConnectorDetect } from './connectorDetect';
import { WhitelistSettings, type AppSettings, requireSettingsRevision } from './features/settings/Settings';
import { Icons } from './icons';
import { BackupSchedule } from './BackupSchedule';
import { WizardActionButton } from './WizardActionButton';
import whatsappIcon from './assets/connectors/whatsapp.svg';

type WhatsAppStatus = {
  installed: boolean;
  version: string | null;
  account: string | null;
  name: string | null;
  sessions: number;
  error: string | null;
};
const steps = ['开始前', '系统检查', '选择会话', '备份周期', '完成'];

export function WhatsAppWizard({ onBack, onOpenChat, resume }: { onBack: () => void; onOpenChat: () => void; resume?: { accountId: string; step: 'sessions' | 'backup' } }) {
  const [step, setStep] = useState(0);
  const [status, setStatus] = useState<WhatsAppStatus | null>(null);
  const [checking, setChecking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [readError, setReadError] = useState('');
  const [selected, setSelected] = useState(0);
  const actionLock = useRef(false);
  const request = useRef(0);
  const install = useConnectorDetect('whatsapp', { refreshOnFocus: false });
  const [resuming, setResuming] = useState(Boolean(resume));
  const resumeAttempted = useRef(false);
  useEffect(() => {
    if (!resume || install.checking || resumeAttempted.current) return;
    let active = true;
    void (async () => {
      try {
        const [fresh, settings] = await Promise.all([invoke<WhatsAppStatus>('whatsapp_status'), invoke<AppSettings>('app_settings_get')]);
        if (!active) return;
        if (!install.usable || !fresh.installed || fresh.error || fresh.account !== resume.accountId) {
          setError(t('WhatsApp 的安装、权限或登录账号已变化，请重新检查后继续。'));
          setStep(1); return;
        }
        await invoke('chat_account_activate', { accountId: resume.accountId });
        if (!active) return;
        const count = settings.chat.allowed_usernames[resume.accountId]?.length ?? 0;
        setSelected(count); setStatus(fresh);
        setStep(resume.step === 'backup' && count > 0 ? 3 : 2);
      } catch (reason) { if (active) { setError(tf('未能恢复配置：{0}，请重新检查后继续。', String(reason))); setStep(1); } }
      finally { if (active) { resumeAttempted.current = true; setResuming(false); } }
    })();
    return () => { active = false; };
  }, [resume, install.checking, install.usable]);
  // Check once when entering this step. Returning from System Settings never
  // refreshes the page or advances the flow; retry is an explicit action.
  useEffect(() => {
    if (step !== 1) return;
    let active = true;
    const id = ++request.current;
    setChecking(true); setReadError(''); setStatus(null);
    void install.recheck();
    void invoke<WhatsAppStatus>('whatsapp_status')
      .then(value => { if (active && request.current === id) setStatus(value); })
      .catch(reason => { if (active && request.current === id) setReadError(String(reason)); })
      .finally(() => { if (active && request.current === id) setChecking(false); });
    return () => { active = false; request.current += 1; };
  }, [step, install.recheck]);

  const recheck = async () => {
    if (actionLock.current || checking) return;
    const id = ++request.current;
    setChecking(true); setReadError(''); setError('');
    try {
      const [, value] = await Promise.all([install.recheck(), invoke<WhatsAppStatus>('whatsapp_status')]);
      if (request.current === id) setStatus(value);
    } catch (reason) { if (request.current === id) { setStatus(null); setReadError(String(reason)); } }
    finally { if (request.current === id) setChecking(false); }
  };
  const ready = !checking && !install.checking && install.usable && Boolean(status?.installed && status.account && !status.error) && !readError;
  const proceed = async (chatOnly = false) => {
    if (!ready || !status?.account || actionLock.current) return;
    actionLock.current = true; setBusy(true); setError('');
    try {
      // Revalidate before enabling/activating; no keys or WeChat actions are used.
      const fresh = await invoke<WhatsAppStatus>('whatsapp_status');
      if (fresh.error || !fresh.installed || fresh.account !== status.account) {
        setStatus(fresh);
        throw new Error(fresh.error || 'WhatsApp 账号已发生变化，请重新检查后继续。');
      }
      const settings = requireSettingsRevision(await invoke<AppSettings>('app_settings_get'));
      if (settings.connectors.whatsapp_enabled === false) {
        requireSettingsRevision(await invoke<AppSettings>('app_settings_save', { value: { ...settings, connectors: { ...settings.connectors, whatsapp_enabled: true } } }));
      }
      await invoke('chat_account_activate', { accountId: fresh.account });
      if (chatOnly) { await invoke('connector_setup_state', { accountId: fresh.account, completed: true, skipBackup: true }); onOpenChat(); } else { await invoke('connector_setup_state', { accountId: fresh.account, completed: false }); setStep(2); }
    } catch (reason) { setError(`未能完成连接：${String(reason)} 请处理后重试。`); }
    finally { actionLock.current = false; setBusy(false); }
  };
  const needsPermission = /permission denied|operation not permitted|os error (1|13)\b|权限|拒绝/i.test(status?.error || readError);
  const rows = [
    { title: 'WhatsApp 安装', passed: install.usable && Boolean(status?.installed), detail: status?.version ? `WhatsApp ${status.version}` : '', problem: install.detect ? `${detectTitleFor('WhatsApp', install.detect.state)}：${install.detect.message}` : '请安装 WhatsApp 桌面版。' },
    { title: '登录账号', passed: Boolean(status?.account), detail: status?.account ? `${status.name || 'WhatsApp'} · ${status.account}` : '', waiting: needsPermission && !status?.account, problem: '请打开 WhatsApp 桌面版并完成登录，再点「重新检查」。' },
    { title: '本地记录读取', passed: Boolean(status && !status.error && !readError), detail: status && !status.error && !readError ? `已读取 ${status.sessions} 个会话 · 只读访问` : '', problem: needsPermission ? '请在「完全磁盘访问权限」中打开 AIMyChats，然后按系统提示「退出并重新打开」。若没有提示，请按 ⌘Q 退出后重新打开。' : /不支持|缺少.*字段/.test(status?.error || '') ? '当前 WhatsApp 数据版本暂不支持读取，请保留数据并通过「问题反馈」联系我们。' : '本地记录暂时无法读取。请确认 WhatsApp 已登录并加载聊天记录，再点「重新检查」。' },
  ];
  if (resuming) return <div className="wechat-wizard whatsapp-wizard"><p role="status">{t('正在恢复上次的配置…')}</p></div>;
  return <div className="wechat-wizard whatsapp-wizard">
    <button type="button" className="connector-back" disabled={busy} onClick={onBack}>{t("← 返回连接器")}</button>
    <header className="wizard-connector-head"><span className="wizard-connector-mark" aria-hidden="true"><img src={whatsappIcon} alt="" /></span><div><strong>{t("连接向导")}</strong><h2>WhatsApp</h2><p>{t("读取桌面版本地数据，只读")}</p></div></header>
    <nav className="wizard-stepper" aria-label={t("WhatsApp 连接向导")}>{steps.map((label, index) => <button key={label} type="button" className={`${step === index ? 'active' : ''}${index < step ? ' complete' : ''}`} aria-current={step === index ? 'step' : undefined} disabled={busy || index > step || step === 4} onClick={() => { setError(''); setStep(index); }}><span>{index < step ? Icons.check : index + 1}</span>{t(label)}</button>)}</nav>
    {step === 0 && <section className="wizard-method"><div><h2>{t("开始前，请先确认")}</h2><p>{t("打开 WhatsApp 桌面版并完成登录，接下来检查本机读取情况。")}</p></div><ul className="whatsapp-intro"><li>{t("只读取本机聊天记录，不发送消息。")}</li><li>{t("选择会话与备份周期后，自动在后台备份。")}</li></ul><div className="wizard-method-footer"><button type="button" className="wizard-next" onClick={() => setStep(1)}>{t("下一步：系统检查")}</button></div></section>}
    {step === 1 && <section className="wx-setup" aria-label={t("WhatsApp 系统检查")}><header><h2>{t("系统检查")}</h2><p>{t("检查通过后，选择要备份的会话。")}</p></header><ol className="wx-setup-list">{rows.map(row => <li key={t(row.title)} className={`wx-setup-item ${row.waiting ? 'waiting' : row.passed && !checking && !install.checking ? 'done' : 'current'}`}><div className="wx-setup-row"><span className="wx-setup-indicator" aria-hidden="true">{row.passed && !checking && !install.checking ? Icons.check : Icons.clock}</span><span className="wx-setup-row-copy"><strong>{t(row.title)}</strong>{row.detail && <small>{t(row.detail)}</small>}</span><span className="wx-setup-status">{(checking || install.checking) && <svg className="wizard-button-spinner" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M8 2a6 6 0 0 1 6 6" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" /></svg>}{!checking && !install.checking && row.passed && <span className="wizard-status-check" aria-hidden="true">{Icons.check}</span>}{!checking && !install.checking && row.passed ? t("已完成") : t("未完成")}</span></div>{!checking && !install.checking && !row.passed && !row.waiting && <div className="wx-setup-body"><p>{t(row.problem)}</p>{row.title === "WhatsApp 安装" && install.detect?.state === 'not_installed' && <a href="https://www.whatsapp.com/download" target="_blank" rel="noreferrer">{t("下载 WhatsApp ↗")}</a>}{row.title === "本地记录读取" && needsPermission && <button type="button" className="wxv-btn" onClick={() => void invoke('perm_open_settings').catch(reason => setError(String(reason)))}>{t("打开系统设置")}</button>}</div>}</li>)}</ol>
      {(readError || status?.error) && <details className="wx-setup-details"><summary>{t("查看错误详情")}</summary><p>{readError || status?.error}</p></details>}
      {error && <p className="wx-setup-error" role="alert">{error}</p>}
      <footer><WizardActionButton className="wizard-skip" busyLabel={t("正在连接…")} disabled={!ready || busy} onAction={() => proceed(true)}>{t("暂不备份，进入聊天")}</WizardActionButton><button type="button" className="wxv-btn" disabled={busy} onClick={() => setStep(0)}>{t("上一步")}</button><WizardActionButton className="wxv-btn" loading={checking || install.checking} busyLabel={t("正在检查…")} disabled={busy} onAction={recheck}>{t("重新检查")}</WizardActionButton><WizardActionButton className="wizard-next" busyLabel={t("正在连接…")} disabled={!ready || busy} onAction={() => proceed()}>{t("下一步：选择会话")}</WizardActionButton></footer>
    </section>}
    {step === 2 && <section className="wizard-settings-step"><WhitelistSettings wizard wizardSource="whatsapp" onBack={() => setStep(1)} onSaved={async count => {
      if (count <= 0 || actionLock.current) return;
      actionLock.current = true; setBusy(true); setError('');
      try { const settings = await invoke<AppSettings>('app_settings_get'); await invoke('connector_setup_state', { accountId: settings.active_account_id, completed: false, wizardStep: 'backup' }); setSelected(count); setStep(3); }
      catch (reason) { setError(tf('保存向导进度失败：{0}，请重试。', String(reason))); }
      finally { actionLock.current = false; setBusy(false); }
    }} /></section>}
    {step > 1 && error && <p className="wx-setup-error" role="alert">{error}</p>}
    {step === 3 && <section className="wizard-settings-step"><BackupSchedule wizard onBack={() => setStep(2)} onContinue={async () => { const settings = await invoke<AppSettings>('app_settings_get'); await invoke('connector_setup_state', { accountId: settings.active_account_id, completed: true }); setStep(4); }} /></section>}
    {step === 4 && <section className="wizard-complete"><div className="wizard-complete-mark">{Icons.check}</div><h2>{t("WhatsApp 已连接")}</h2><p>{tf("已保存 {0} 个会话的备份范围。可在「任务」中查看备份状态。", selected)}</p><button type="button" className="wizard-next" onClick={onOpenChat}>{t("进入聊天")}</button></section>}
  </div>;
}
