import { RailIcons } from './icons';
import brandMark from './assets/app-mark-transparent.png';
import { version } from '../src-tauri/tauri.conf.json';

const avatarColors = ['#7c4d80', '#3f6b8f', '#3f766b', '#8a5b42', '#5869a6', '#8a4764', '#72703e', '#486d92'];

function accountAvatar(name: string) {
  const normalized = name.trim().normalize('NFC');
  const initial = Array.from(normalized).find(character => /[\p{L}\p{N}]/u.test(character))?.toUpperCase();
  let hash = 0;
  for (const character of normalized.toLowerCase()) hash = (Math.imul(hash, 31) + character.codePointAt(0)!) >>> 0;
  return { initial, color: normalized ? avatarColors[hash % avatarColors.length] : '#52525b' };
}

export type Section = 'chat' | 'events' | 'topics' | 'reports' | 'ask' | 'tasks' | 'settings';

const primaryItems = [
  { id: 'chat', label: '聊天', icon: RailIcons.chat },
  { id: 'events', label: '事件', icon: RailIcons.events },
  { id: 'topics', label: '主题', icon: RailIcons.topics },
  { id: 'reports', label: '报告', icon: RailIcons.reports },
  { id: 'ask', label: '问答', icon: RailIcons.ask },
] as const;
const utilityItems = [
  { id: 'tasks', label: '任务', icon: RailIcons.tasks },
  { id: 'settings', label: '设置', icon: RailIcons.settings },
] as const;

export function NavigationRail({ section, onNavigate, onAccount, authenticated, accountName = '', showBrand = true }: {
  section: Section;
  onNavigate: (section: Section) => void;
  onAccount: () => void;
  authenticated: boolean;
  accountName?: string;
  showBrand?: boolean;
}) {
  const avatar = accountAvatar(accountName);
  const navButton = (item: (typeof primaryItems)[number] | (typeof utilityItems)[number]) => <button
    key={item.id} type="button" className={`prototype-nav-item${item.id === 'tasks' ? ' prototype-nav-item-tasks' : ''}${section === item.id ? ' active' : ''}`}
    title={item.label} aria-current={section === item.id ? 'page' : undefined} onClick={() => onNavigate(item.id)}
  ><span className="prototype-nav-icon">{item.icon}</span><small>{item.label}</small></button>;

  return <aside className="prototype-rail" aria-label="主导航">
    <button type="button" className="prototype-rail-mark" style={{ backgroundColor: avatar.color }} onClick={onAccount} disabled={!authenticated} aria-label="打开账户设置" title={accountName ? `${accountName} · 账户设置` : '账户设置'}>
      {avatar.initial ?? <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true"><circle cx="12" cy="8" r="3.5" /><path d="M5 21v-2a7 7 0 0 1 14 0v2" /></svg>}
    </button>
    <nav className="prototype-nav" aria-label="工作区">{primaryItems.map(navButton)}</nav>
    <div className="prototype-spacer" />
    <nav className="prototype-nav" aria-label="应用">{utilityItems.map(navButton)}</nav>
    {showBrand && <div className="prototype-rail-brand"><img src={brandMark} alt="AIMyChats" title="AIMyChats" /><small className="prototype-rail-version" aria-label={`版本 ${version}`}>v{version}</small></div>}
  </aside>;
}
