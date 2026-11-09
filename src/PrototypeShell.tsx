import { Fragment, useCallback, useEffect, useState, type MouseEvent, type ReactNode } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { listen } from '@tauri-apps/api/event';
import WeChatLocal from './WeChatLocal';
import { AgentHub } from './AgentHub';
import { About } from './About';
import { HistoryPage } from './History';
import { DevifySettings, WhitelistSettings } from './Settings';
import { BackupSchedule } from './BackupSchedule';
import { AnalysisSettings } from './AnalysisSettings';
import { LanguageSettings } from './LanguageSettings';
import { Icons, RailIcons } from './icons';
import { Connectors } from './Connectors';
import { CloudAnalysisPage } from './CloudAnalysis';
import { TopicsPage } from './Topics';
import { AskPage } from './Ask';
import { Tasks } from './Tasks';
import { ChatEntry } from './ChatLanding';
import { ChatWorkbench } from './ChatWorkbench';
import { LoginGate } from './LoginGate';
import { registerDevifyDevice } from './devifyDevice';
import { refreshEntitlements } from './entitlements';
import './prototype-shell.css';
import './reference-theme.css';
import appIcon from './assets/app-icon.png';

type Section = 'chat' | 'events' | 'topics' | 'reports' | 'ask' | 'tasks' | 'settings';
type SettingsPage = 'connectors' | 'imports' | 'backup' | 'analysis' | 'language' | 'account' | 'history' | 'agent' | 'about';

function handleWindowDrag(event: MouseEvent<HTMLElement>) {
  if (!isTauri() || event.button !== 0 || event.clientY - event.currentTarget.getBoundingClientRect().top >= 72) return;
  if ((event.target as Element).closest('button, input, a, select, textarea, [contenteditable], [role="tab"], [role="separator"], [role="dialog"], [role="menuitem"]')) return;
  if (event.currentTarget.querySelector('[aria-modal="true"]')) return;
  if (event.detail === 2) {
    event.preventDefault();
    void getCurrentWindow().toggleMaximize();
  } else if (event.detail === 1) {
    event.preventDefault();
    void getCurrentWindow().startDragging();
  }
}

const sections: { id: Section; label: string; icon: ReactNode }[] = [
  { id: 'chat', label: '聊天', icon: RailIcons.chat },
  { id: 'events', label: '事件', icon: RailIcons.events },
  { id: 'topics', label: '主题', icon: RailIcons.topics },
  { id: 'reports', label: '报告', icon: RailIcons.reports },
  { id: 'ask', label: '问答', icon: RailIcons.ask },
];
const settingsPages: { id: SettingsPage; label: string; group?: 'main' | 'local' }[] = [
  // The first six follow protype.html (连接器 / 导入管理 / 备份周期 / 智能分析 / 语言 / 账户);
  // the rest are pages this build has on top of the prototype.
  { id: 'connectors', label: '连接器', group: 'main' },
  { id: 'imports', label: '导入管理', group: 'main' },
  { id: 'backup', label: '备份周期', group: 'main' },
  { id: 'analysis', label: '智能分析', group: 'main' },
  { id: 'language', label: '语言', group: 'main' },
  { id: 'account', label: '账户', group: 'main' },
  // Local summaries are saved on this machine only; the prototype has no page for
  // them, but without this entry the records are unreachable from settings.
  { id: 'history', label: '总结记录', group: 'local' },
  { id: 'agent', label: 'Agent 集成', group: 'local' },
  { id: 'about', label: '关于', group: 'local' },
];
const wizardSettingsPages: Array<{ id: SettingsPage; label: string; group?: 'main' | 'local' }> = [
  { id: 'connectors', label: '连接器' },
  { id: 'imports', label: '导入管理' },
  { id: 'backup', label: '备份周期' },
  { id: 'analysis', label: '智能分析' },
  { id: 'language', label: '语言' },
  { id: 'account', label: '账户' },
];
const settingsIcons: Record<SettingsPage, ReactNode> = {
  connectors: Icons.key, imports: Icons.download, backup: Icons.clock,
  analysis: Icons.agent, language: Icons.list, account: Icons.allow,
  history: Icons.history, agent: Icons.agent, about: Icons.info,
};

export default function PrototypeShell() {
  const [section, setSection] = useState<Section>('chat');
  const [eventFocusId, setEventFocusId] = useState<number | null>(null);
  const [chatReady, setChatReady] = useState(false);
  const [startTaskCreate, setStartTaskCreate] = useState(false);
  const [authState, setAuthState] = useState<'checking' | 'authed' | 'anonymous'>('checking');
  const [authNotice, setAuthNotice] = useState('');
  const [currentUser, setCurrentUser] = useState('');
  const [accountMenu, setAccountMenu] = useState(false);
  useEffect(() => {
    let active = true;
    setCurrentUser('');
    if (authState === 'authed') void invoke<{ display_name?: string; username?: string }>('devify_current_user').then(user => { if (active) setCurrentUser(user.display_name || user.username || ''); }).catch(() => { if (active) setCurrentUser(''); });
    return () => { active = false; };
  }, [authState]);
  const handleAuthenticated = useCallback(() => { setAuthState('authed'); setAuthNotice(''); setChatReady(false); setSection('chat'); setEventFocusId(null); setStartTaskCreate(false); void registerDevifyDevice(); }, []);
  useEffect(() => {
    let active = true;
    let cleanExpired: (() => void) | undefined;
    void invoke<{ authenticated: boolean }>('devify_auth_status')
      .then(status => {
        if (!active) return;
        setAuthState(status.authenticated ? 'authed' : 'anonymous');
        if (status.authenticated) {
          void registerDevifyDevice();
        }
      })
      .catch(() => { if (active) setAuthState('anonymous'); });
    // The backend drops stale credentials and emits this when the server rejects
    // the stored session (expired, revoked or deleted account), so the app falls
    // back to the login gate instead of showing every Devify page as broken.
    void listen<{ message?: string }>('devify-auth-expired', event => {
      setAuthNotice(event.payload?.message ?? '登录状态已失效，请重新登录。');
      setAuthState('anonymous');
    }).then(unlisten => { if (active) cleanExpired = unlisten; else unlisten(); }).catch(() => {});
    const onAuthChanged = (event: Event) => {
      const detail = (event as CustomEvent<{ authenticated?: boolean }>).detail;
      setAuthNotice('');
      setAuthState(detail?.authenticated === false ? 'anonymous' : 'authed');
      setChatReady(false);
      setSection('chat');
    };
    window.addEventListener('aimychats:auth-changed', onAuthChanged);
    return () => {
      active = false;
      cleanExpired?.();
      window.removeEventListener('aimychats:auth-changed', onAuthChanged);
    };
  }, []);
  const openNewUpload = () => { setStartTaskCreate(true); setSection('tasks'); };
  // Entitlements are per account and per server, so every successful sign-in refreshes them
  // once. A server without the endpoint answers 404 and the app keeps its legacy behaviour.
  useEffect(() => {
    if (authState !== 'authed') return;
    void refreshEntitlements();
  }, [authState]);
  const handleChatReady = useCallback(() => setChatReady(true), []);
  const [settingsPage, setSettingsPage] = useState<SettingsPage>('connectors');
  const [wechatGuide, setWechatGuide] = useState(false);
  const [wechatGuideEdit, setWechatGuideEdit] = useState(false);
  const [wizardStep, setWizardStep] = useState('wizard-method');
  const wizardOrder = ['wizard-method', 'wizard-permission', 'wizard-account', 'wizard-key', 'wizard-import', 'wx-connect-decrypt', 'wizard-done'];
  const openSettings = (page: SettingsPage, guide = false, edit = guide) => { setSettingsPage(page); setWechatGuide(guide); setWechatGuideEdit(edit); setWizardStep('wizard-method'); setSection('settings'); };
  const setWizard = (step: string) => {
    setWizardStep(step);
    const target = step === 'wizard-permission' || step === 'wizard-account' ? 'wx-connect-account' : step === 'wizard-key' || step === 'wizard-import' ? 'wx-connect-key' : step === 'wx-connect-decrypt' ? 'wx-connect-decrypt' : step;
    window.requestAnimationFrame(() => document.getElementById(target)?.scrollIntoView({ behavior: 'smooth', block: 'start' }));
  };
  const wizardPrev = () => { const index = wizardOrder.indexOf(wizardStep); if (index > 0) setWizard(wizardOrder[index - 1]); };
  const wizardNext = () => { const index = wizardOrder.indexOf(wizardStep); if (index >= 0 && index < wizardOrder.length - 1) setWizard(wizardOrder[index + 1]); };
  const openEvent = (eventId: number) => { setEventFocusId(eventId); setSection('events'); };
  const navButton = (item: { id: Section; label: string; icon: ReactNode }) => <button
    key={item.id} type="button" className={section === item.id ? 'prototype-nav-item active' : 'prototype-nav-item'}
    aria-current={section === item.id ? 'page' : undefined}
    onClick={() => { if (item.id !== 'events') setEventFocusId(null); if (item.id === 'chat') setChatReady(false); setStartTaskCreate(false); setSection(item.id); }}
  ><span>{item.icon}</span><small>{item.label}</small></button>;

  return <main className="prototype-shell" onMouseDown={handleWindowDrag}>
    <div className="prototype-titlebar" aria-hidden="true" />
    <aside className="prototype-rail" aria-label="主导航">
      {authState === 'authed' && <div className="prototype-account-wrap"><button type="button" className="prototype-account" onClick={() => setAccountMenu(open => !open)} aria-expanded={accountMenu}><span className="prototype-account-avatar">{(currentUser || '账').slice(0, 1)}</span><span className="prototype-account-text"><strong>{currentUser || '账号信息暂不可用'}</strong><small>AIMyChats 账号</small></span><span aria-hidden="true">⌄</span></button>{accountMenu && <div className="prototype-account-menu"><button type="button" onClick={() => { setAccountMenu(false); openSettings('account'); }}>账号设置</button><button type="button" onClick={() => { setAccountMenu(false); void invoke('devify_logout').then(() => { setCurrentUser(''); setAuthState('anonymous'); setChatReady(false); }); }}>退出登录</button></div>}</div>}
      <nav className="prototype-nav">{sections.map(navButton)}</nav>
      <div className="prototype-spacer" />
      <nav className="prototype-nav">
        {navButton({ id: 'tasks', label: '任务', icon: RailIcons.tasks })}
        {navButton({ id: 'settings', label: '设置', icon: RailIcons.settings })}
      </nav>
      <div className="prototype-rail-brand"><img src={appIcon} alt=""/><strong>AIMyChats</strong></div>
    </aside>
    <section className="prototype-content">
      {authState === 'checking' && <div className="auth-checking" role="status"><span className="auth-gate-pulse"/>正在检查登录状态…</div>}
      {authState === 'anonymous' && <LoginGate notice={authNotice} onAuthenticated={handleAuthenticated}/>}
      {authState === 'authed' && <>
      {section === 'chat' && <>
        {!chatReady && <ChatEntry
          onConnect={(connector) => connector && connector !== '微信' ? openSettings('connectors') : openSettings('connectors', true, false)}
          onImport={() => openSettings('imports')}
          onBackup={openNewUpload}
          onSchedule={() => openSettings('backup')}
          onReady={handleChatReady}
          onUnlock={() => openSettings('connectors', true, true)}
        />}
        {chatReady && <ChatWorkbench onOpenData={() => openSettings('connectors', true)} onOpenWhitelist={() => openSettings('imports')} onNewUpload={openNewUpload} onSchedule={() => openSettings('backup')} onOpenAccount={() => openSettings('account')} onOpenEvent={openEvent} />}
      </>}
      {section === 'tasks' && <Tasks startCreating={startTaskCreate} onNewUpload={() => openSettings('account')} />}
      {section === 'settings' && <div className="prototype-settings-layout">
        <header className="prototype-settings-breadcrumb">设置 <span>/</span> {(wechatGuide ? wizardSettingsPages : settingsPages).find(item => item.id === settingsPage)?.label ?? ''}</header>
        <div className="prototype-settings">
        <aside className="prototype-settings-nav">
          <h1>设置</h1>
          {(wechatGuide ? wizardSettingsPages : settingsPages).map((item, index, list) => {
            const divider = !wechatGuide && item.group === 'local' && list[index - 1]?.group === 'main';
            return <Fragment key={item.id}>
              {divider && <div className="prototype-settings-divider" aria-hidden="true"/>}
              <button type="button" className={settingsPage === item.id ? 'active' : ''} onClick={() => { setSettingsPage(item.id); setWechatGuide(false); }} aria-current={settingsPage === item.id ? 'page' : undefined}><span className="settings-nav-icon">{settingsIcons[item.id]}</span>{item.label}</button>
            </Fragment>;
          })}
        </aside>
        <div className="prototype-settings-content">
          {settingsPage === 'connectors' && (wechatGuide ? <div className="wechat-wizard">
            <button type="button" className="connector-back" onClick={() => setWechatGuide(false)}>← 返回连接器</button>
            <header className="wizard-connector-head"><span className="connector-mark green">微</span><div><strong>{wechatGuideEdit ? '重新配置' : '连接向导'}</strong><h2>微信</h2><p>读取本机数据库，只读，不改动微信</p></div></header>
            <nav className="wizard-stepper" aria-label="微信连接向导">{[
              ['wizard-method','接入方式'], ['wizard-permission','系统授权'], ['wizard-account','账号扫描'], ['wizard-key','获取密钥'], ['wizard-import','导入校验'], ['wx-connect-decrypt','解密记录'], ['wizard-done','完成'],
            ].map(([id,label], index) => { const current = wizardOrder.indexOf(wizardStep); const complete = index < current; return <button key={id} type="button" className={`${wizardStep === id ? 'active ' : ''}${complete ? 'complete' : ''}`} aria-current={wizardStep === id ? 'step' : undefined} onClick={() => setWizard(id)} disabled={index > current}><span>{complete ? Icons.check : index + 1}</span>{label}</button>; })}</nav>
            {wizardStep === 'wizard-method' && (
            <section id="wizard-method" className="wizard-method"><div><h2>选择接入方式</h2><p>微信的数据库是加密的。两种方式都只读，不会改动微信，也不需要你的微信密码。</p></div><div className="wizard-method-options"><button type="button" className="wizard-method-option selected" onClick={() => setWizard('wizard-permission')}><strong>自动读取本机数据库 <em>推荐</em></strong><span>持续读取本机数据库；只需完成系统授权，再按原有引导获取并校验密钥。整个流程不需要修改系统完整性保护。</span></button><button type="button" className="wizard-method-option" onClick={() => setWizard('wizard-key')}><strong>导入可信密钥文件 <em>无需改系统设置</em></strong><span>不动系统设置、不捕获微信进程；选择可信密钥文件后逐库校验，再解密聊天记录。</span></button></div><div className="wizard-method-footer"><button type="button" className="wizard-next" onClick={() => setWizard('wizard-permission')}>下一步</button></div></section>
            )}
            <div id="wizard-permission" className="wizard-anchor" /><div id="wizard-account" className="wizard-anchor" /><div id="wizard-key" className="wizard-anchor" /><div id="wizard-import" className="wizard-anchor" /><div id="wizard-done" className="wizard-anchor" />
            {wizardStep === 'wizard-done' ? <section className="wizard-complete"><div className="wizard-complete-mark">✓</div><h2>连接向导完成</h2><p>账号、密钥和解密记录完成后，聊天内容会在「聊天」页面显示。</p><button type="button" className="wizard-next" onClick={() => setSection('chat')}>打开聊天</button></section> : <WeChatLocal mode="settings" wizard wizardStep={wizardStep} onWizardBack={wizardPrev} onWizardNext={wizardNext} onOpenChat={() => setSection('chat')} />}
          </div> : <Connectors onOpenWechat={(configured) => { setWechatGuide(true); setWechatGuideEdit(configured); setWizardStep('wizard-method'); }} />)}
          {settingsPage === 'imports' && <WhitelistSettings />}
          {settingsPage === 'backup' && <BackupSchedule onOpenConnectors={() => { setWechatGuide(false); setSettingsPage('connectors'); }} />}
          {settingsPage === 'analysis' && <AnalysisSettings />}
          {settingsPage === 'language' && <LanguageSettings />}
          {settingsPage === 'account' && <DevifySettings />}
          {settingsPage === 'history' && <HistoryPage />}
          {settingsPage === 'agent' && <AgentHub />}
          {settingsPage === 'about' && <About />}
        </div>
        </div>
      </div>}
      {section === 'events' && <CloudAnalysisPage kind="events" focusId={eventFocusId} onOpenEvent={openEvent} />}
      {section === 'reports' && <CloudAnalysisPage kind="reports" />}
      {section === 'topics' && <TopicsPage onOpenEvent={openEvent} />}
      {section === 'ask' && <AskPage onOpenEvent={openEvent} />}
      </>}
    </section>
  </main>;
}
