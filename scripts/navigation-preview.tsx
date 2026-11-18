import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { NavigationRail, type Section } from '../src/NavigationRail';
import '../src/style.css';
import '../src/prototype-shell.css';
import '../src/navigation-rail.css';
import './navigation-preview.css';

const labels: Record<Section, string> = { chat: '聊天', events: '事件', topics: '主题', reports: '报告', ask: '问答', tasks: '任务', settings: '设置' };

function NavigationPreview() {
  const [section, setSection] = useState<Section>('chat');
  const [account, setAccount] = useState(false);
  const [macOS, setMacOS] = useState(true);
  const [username, setUsername] = useState('Ray');
  const [showBrand, setShowBrand] = useState(true);
  return <main className={`prototype-shell${macOS ? ' prototype-shell-macos' : ''}`}>
    <div className="prototype-titlebar" aria-hidden="true" />
    <NavigationRail section={section} accountName={username} showBrand={showBrand} authenticated onNavigate={next => { setAccount(false); setSection(next); }} onAccount={() => { setAccount(true); setSection('settings'); }} />
    <section className="navigation-preview-content">
      <header><span>{labels[section]}{account ? ' / 账户' : ''}</span><div className="navigation-preview-options"><label><input type="checkbox" checked={showBrand} onChange={event => setShowBrand(event.target.checked)} />保留底部 logo</label><label><input type="checkbox" checked={macOS} onChange={event => setMacOS(event.target.checked)} />预留 macOS 窗口按钮空间</label></div></header>
      <div className="navigation-preview-body">
        <h1>{account ? '账户' : labels[section]}</h1>
        <p>左侧菜单预览</p>
        <p>点击左侧图标切换选中项，移动鼠标查看悬停效果。顶部头像可打开账户入口。</p>
        <label className="navigation-preview-username">预览用户名<input value={username} onChange={event => setUsername(event.target.value)} placeholder="输入用户名" /></label>
        <small>此页面仅预览菜单，不读取聊天记录。</small>
      </div>
    </section>
  </main>;
}

createRoot(document.getElementById('root')!).render(<NavigationPreview />);
