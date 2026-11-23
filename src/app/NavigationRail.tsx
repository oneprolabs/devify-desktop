import { t, tf, ctf } from '../shared/i18n/locale';
import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useTaskRuntime } from '../features/tasks/taskRuntime';
import { RailIcons } from '../shared/ui/icons';
import brandMark from '../assets/app-mark-transparent.png';

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
  { id: 'events', label: '待办', icon: RailIcons.events },
  { id: 'topics', label: '主题', icon: RailIcons.topics },
  { id: 'reports', label: '报告', icon: RailIcons.reports },
  { id: 'ask', label: '问答', icon: RailIcons.ask },
] as const;
const utilityItems = [
  { id: 'tasks', label: '任务', icon: RailIcons.tasks },
  { id: 'settings', label: '设置', icon: RailIcons.settings },
] as const;

export function NavigationRail({ section, onNavigate, onAccount, authenticated, accountName = '', showBrand = true, updateVersion, onUpdate }: {
  section: Section;
  onNavigate: (section: Section) => void;
  onAccount: () => void;
  authenticated: boolean;
  accountName?: string;
  showBrand?: boolean;
  updateVersion?: string;
  onUpdate?: () => void;
}) {
  const tasks = useTaskRuntime();
  const [currentVersion, setCurrentVersion] = useState('');
  useEffect(() => {
    if (!showBrand) return;
    let active = true;
    void invoke<{version:string}>('app_info').then(info => {
      if (active) setCurrentVersion(info.version.trim());
    }).catch(() => { if (active) setCurrentVersion(''); });
    return () => { active = false; };
  }, [showBrand]);
  const avatar = accountAvatar(accountName);
  const navButton = (item: (typeof primaryItems)[number] | (typeof utilityItems)[number]) => <button
    key={item.id} type="button" className={`prototype-nav-item${item.id === 'tasks' ? ` prototype-nav-item-tasks${tasks.running ? ' task-running' : ''}` : ''}${section === item.id ? ' active' : ''}`}
    title={t(item.label)} aria-current={section === item.id ? 'page' : undefined} onClick={() => onNavigate(item.id)}
  ><span className="prototype-nav-icon">{item.icon}{item.id === 'tasks' && tasks.running && <svg className="task-runtime-ring" width="44" height="44" viewBox="0 0 44 44" fill="none" aria-hidden="true" focusable="false">
    <circle cx="22" cy="22" r="20" stroke="currentColor" strokeOpacity=".22" strokeWidth="2" />
    <path className="task-runtime-ring-arc" d="M22 2a20 20 0 0 1 20 20" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
  </svg>}</span><small>{t(item.label)}</small></button>;

  return <aside className="prototype-rail" aria-label={t("主导航")}>
    <button type="button" className="prototype-rail-mark" style={{ backgroundColor: avatar.color }} onClick={onAccount} disabled={!authenticated} aria-label={t("打开账户设置")} title={accountName ? tf("{0} · 账户设置", accountName) : t("账户设置")}>
      {avatar.initial ?? <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true"><circle cx="12" cy="8" r="3.5" /><path d="M5 21v-2a7 7 0 0 1 14 0v2" /></svg>}
    </button>
    <nav className="prototype-nav" aria-label={t("工作区")}>{primaryItems.map(navButton)}</nav>
    <div className="prototype-spacer" />
    <nav className="prototype-nav" aria-label={t("应用")}>
      {updateVersion && <button type="button" className="prototype-rail-update" onClick={onUpdate} title={tf("发现新版本 {0}，查看更新", updateVersion)} aria-label={tf("发现新版本 {0}，查看更新", updateVersion)}>
        <span className="prototype-rail-update-label"><svg viewBox="0 0 20 20" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M10 14V4m-4 4 4-4 4 4M4 15v2h12v-2"/></svg>{t('更新')}</span>
        <span className="prototype-rail-update-version">v{updateVersion.replace(/^v/, '')}</span>
      </button>}
      {utilityItems.map(navButton)}
    </nav>
    {showBrand && <div className="prototype-rail-brand"><img src={brandMark} alt="AIMyChats" title="AIMyChats" /><span className="prototype-rail-version" title={currentVersion ? `${t('当前版本')} ${currentVersion}` : t('版本读取失败')}>{currentVersion ? `v${currentVersion.replace(/^v/, '')}` : '—'}</span></div>}
  </aside>;
}
