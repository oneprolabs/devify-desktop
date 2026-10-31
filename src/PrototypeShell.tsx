import { useState, type ReactNode } from 'react';
import WeChatLocal from './WeChatLocal';
import { HistoryPage } from './History';
import { AgentHub } from './AgentHub';
import { About } from './About';
import { DevifySettings, ServerAnalysisSettings, StrategySettings, WhitelistSettings } from './Settings';
import { Icons } from './icons';
import { Connectors } from './Connectors';
import { CloudAnalysisPage } from './CloudAnalysis';
import { TopicsPage } from './Topics';
import { AskPage } from './Ask';
import { Tasks } from './Tasks';
import './prototype-shell.css';

type Section = 'chat' | 'events' | 'topics' | 'reports' | 'ask' | 'tasks' | 'settings';
type SettingsPage = 'connectors' | 'imports' | 'backup' | 'analysis' | 'language' | 'account' | 'agent' | 'about' | 'history' | 'automation';

const sections: { id: Section; label: string; icon: ReactNode }[] = [
  { id: 'chat', label: '聊天', icon: Icons.chat },
  { id: 'events', label: '事件', icon: Icons.allow },
  { id: 'topics', label: '主题', icon: Icons.clock },
  { id: 'reports', label: '报告', icon: Icons.history },
  { id: 'ask', label: '问答', icon: Icons.agent },
];
const settingsPages: { id: SettingsPage; label: string }[] = [
  { id: 'connectors', label: '连接器' },
  { id: 'imports', label: '导入管理' },
  { id: 'backup', label: '备份周期' },
  { id: 'analysis', label: '智能分析' },
  { id: 'automation', label: '本地自动化' },
  { id: 'history', label: '总结记录' },
  { id: 'account', label: '账户' },
  { id: 'agent', label: 'Agent 集成' },
  { id: 'about', label: '关于' },
];
const wizardSettingsPages: Array<{ id: SettingsPage; label: string }> = [
  { id: 'connectors', label: '连接器' },
  { id: 'imports', label: '导入管理' },
  { id: 'backup', label: '备份周期' },
  { id: 'analysis', label: '智能分析' },
  { id: 'language', label: '语言' },
  { id: 'account', label: '账户' },
];

export default function PrototypeShell() {
  const [section, setSection] = useState<Section>('chat');
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
  const navButton = (item: { id: Section; label: string; icon: ReactNode }) => <button
    key={item.id} type="button" className={section === item.id ? 'prototype-nav-item active' : 'prototype-nav-item'}
    aria-current={section === item.id ? 'page' : undefined} onClick={() => setSection(item.id)}
  ><span>{item.icon}</span><small>{item.label}</small></button>;

  return <main className="prototype-shell">
    <aside className="prototype-rail" aria-label="主导航">
      <div className="prototype-drag" data-tauri-drag-region />
      <div className="prototype-logo" aria-label="Aimychats">a</div>
      <nav className="prototype-nav">{sections.map(navButton)}</nav>
      <div className="prototype-spacer" />
      <nav className="prototype-nav">
        {navButton({ id: 'tasks', label: '任务', icon: Icons.list })}
        {navButton({ id: 'settings', label: '设置', icon: Icons.info })}
      </nav>
    </aside>
    <section className="prototype-content">
      {section === 'chat' && <WeChatLocal onOpenData={() => openSettings('connectors', true)} onOpenWhitelist={() => openSettings('imports')} />}
      {section === 'tasks' && <Tasks onNewUpload={() => openSettings('account')} />}
      {section === 'settings' && <div className="prototype-settings-layout">
        <header className="prototype-settings-breadcrumb" data-tauri-drag-region>设置 <span>/</span> 数据源与定时上传</header>
        <div className="prototype-settings">
        <aside className="prototype-settings-nav">
          <h1>设置</h1>
          {(wechatGuide ? wizardSettingsPages : settingsPages).map(item => <button key={item.id} type="button" className={settingsPage === item.id ? 'active' : ''} onClick={() => { setSettingsPage(item.id); setWechatGuide(false); }} aria-current={settingsPage === item.id ? 'page' : undefined}>{item.label}</button>)}
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
          {settingsPage === 'backup' && <StrategySettings title="备份周期" description="设置本机数据同步频率和日报策略。上传范围仍由导入管理控制，事件、主题和报告由 Devify 服务端生成。" />}
          {settingsPage === 'analysis' && <ServerAnalysisSettings />}
          {settingsPage === 'automation' && <StrategySettings />}
          {settingsPage === 'language' && <div className="prototype-settings-empty"><h2>语言</h2><p>界面语言和 AI 输出语言设置将在服务端同步完成后提供。</p></div>}
          {settingsPage === 'account' && <DevifySettings />}
          {settingsPage === 'agent' && <AgentHub />}
          {settingsPage === 'history' && <HistoryPage />}
          {settingsPage === 'about' && <About />}
        </div>
        </div>
      </div>}
      {section === 'events' && <CloudAnalysisPage kind="events" />}
      {section === 'reports' && <CloudAnalysisPage kind="reports" />}
      {section === 'topics' && <TopicsPage />}
      {section === 'ask' && <AskPage />}
    </section>
  </main>;
}
