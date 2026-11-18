import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import appIcon from './assets/app-icon.png';
import { checkForUpdates, dismissInstallBlockers, downloadUpdate, installUpdate, RELEASE_URL, useUpdater } from './updater';

type AppInfo = {
  name: string;
  version: string;
  platform: string;
  arch: string;
};

const PLATFORM_LABEL: Record<string, string> = {
  macos: 'macOS',
  darwin: 'macOS',
  windows: 'Windows',
  linux: 'Linux',
};

const ARCH_LABEL: Record<string, string> = {
  aarch64: 'ARM64',
  arm64: 'ARM64',
  x86_64: 'x86_64',
  x86: 'x86',
};

const HIGHLIGHTS = [
  ['只读聊天记录', '定位本机微信数据、导入密钥、解密副本，按聊天白名单浏览与检索原文。'],
  ['聊天备份', '选择会话和首次上传范围，并按设置同步新增记录。'],
  ['事件与总结', '查看云端整理的事件，以及本地保存的聊天总结。'],
  ['Agent 集成', '配置本地 Agent 并管理相关集成。'],
];

function platformLabel(platform: string, arch: string): string {
  const name = PLATFORM_LABEL[platform] ?? platform;
  const chip = ARCH_LABEL[arch] ?? arch;
  return `${name} · ${chip}`;
}

function updateNoteLines(notes?: string): string[] {
  if (!notes?.trim()) return [];
  const plain = notes.replace(/\s+/g, ' ').trim();
  if (/whatsapp/i.test(plain) && /whitelist/i.test(plain) && /group header|groups sessions by source/i.test(plain)) {
    return [
      '新增 WhatsApp 只读来源，可浏览、搜索并上传本机文字会话，白名单规则与微信相同。',
      '会话列表按来源分组；只有一个来源时不显示分组标题。',
    ];
  }
  return notes.split(/\n+/).map(line => line.replace(/^[-*]\s+/, '').replace(/[`#]/g, '').trim()).filter(Boolean);
}

export function UpdatePanel({ onClose }: { onClose?: () => void } = {}) {
  const update = useUpdater();
  const [error, setError] = useState('');
  const busy = ['checking', 'downloading', 'installing'].includes(update.status);
  const notes = updateNoteLines(update.notes);
  const status = update.status === 'checking' ? '正在检查更新…'
    : update.status === 'latest' ? '当前已是最新版本'
    : update.status === 'available' ? `可以更新到 ${update.version}`
    : update.status === 'downloading' ? '正在下载更新…'
    : update.status === 'downloaded' ? `AIMyChats ${update.version} 已下载`
    : update.status === 'installed' ? '更新已安装，请重启应用'
    : update.status === 'installing' ? '正在安装更新…'
    : update.status === 'error' ? '检查更新失败'
    : '检查软件更新';

  return <div className="update-panel">
    <div className="update-panel-identity" role="status">
      <img src={appIcon} alt="" width={46} height={46} />
      <div>
        <strong>{status}</strong>
        {update.currentVersion && <span>当前版本 {update.currentVersion}</span>}
      </div>
      <div className="about-update-actions">
        {update.status === 'available' && onClose && <button type="button" onClick={onClose}>稍后</button>}
        {['idle', 'latest', 'error'].includes(update.status) && <button type="button" className="update-primary" onClick={() => void checkForUpdates()}>检查更新</button>}
        {update.status === 'available' && <button type="button" className="update-primary" onClick={() => void downloadUpdate()}>{update.error ? '重新下载' : '下载更新'}</button>}
        {['downloaded', 'installed'].includes(update.status) && !update.blockers?.length && <button type="button" className="update-primary" onClick={() => void installUpdate()}>{update.status === 'installed' ? '重新启动' : '安装并重启'}</button>}
        {busy && <span className="update-spinner" aria-hidden="true"/>}
      </div>
    </div>
    {notes.length > 0 && <section className="update-release-notes" aria-label="这次更新">
      <h3>这次更新</h3>
      <ul className="update-note-list">{notes.map(line => <li key={line}>{line}</li>)}</ul>
    </section>}
    {update.status === 'available' && <p className="update-next-step">下载完成后，会再请你确认安装并重启。</p>}
    {update.status === 'downloading' && <div className="update-download">
      <progress value={update.progress} max={100} aria-label="下载进度" />
      {update.progress != null && <span>{update.progress}%</span>}
    </div>}
    {(update.error || error) && <p className="about-update-sub is-error" role="alert">{update.error || error}</p>}
    {update.blockers?.length ? <div className="about-update-blocked" role="alert">
      <span>有上传正在进行。现在安装会中断上传，重启后可在「任务」页重试。</span>
      <button type="button" onClick={dismissInstallBlockers}>稍后</button>
      <button type="button" className="update-primary" onClick={() => void installUpdate(true)}>仍然安装</button>
    </div> : null}
    <div className="update-panel-footer">
      <button type="button" className="update-link" onClick={() => {
        setError('');
        const releaseUrl = update.version && /^\d+\.\d+\.\d+(?:-[\w.-]+)?$/.test(update.version)
          ? RELEASE_URL.replace(/alpha$/, `v${update.version}`)
          : RELEASE_URL;
        void invoke('devify_auth_open', { url: releaseUrl }).catch(value => setError(`打开下载页面失败：${String(value)}`));
      }}>手动下载</button>
    </div>
  </div>;
}

export function About() {
  const update = useUpdater();
  const [updateLater, setUpdateLater] = useState(false);
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [error, setError] = useState('');
  const hideOffer = updateLater && update.status === 'available';

  useEffect(() => {
    invoke<AppInfo>('app_info')
      .then(setInfo)
      .catch(value => setError(`读取版本信息失败：${String(value)}`));
  }, []);


  return <div className="settings-page about-page">
    <header className="page-head"><h2>关于</h2><p className="help">了解 AIMyChats 应用和版本信息。</p></header>
    {error && <div className="error" role="alert">{error}</div>}

    <div className="about-details">
    {!hideOffer && <section className="panel"><UpdatePanel onClose={() => setUpdateLater(true)} /></section>}
    <section className="panel">
      <h2>版本信息</h2>
      <dl className="about-meta">
        <div>
          <dt>版本</dt>
          <dd>{info ? info.version : '—'}</dd>
        </div>
        <div>
          <dt>运行平台</dt>
          <dd>{info ? platformLabel(info.platform, info.arch) : '—'}</dd>
        </div>
        <div>
          <dt>应用标识</dt>
          <dd><code>local.wechat.devifybridge</code></dd>
        </div>
      </dl>
    </section>

    <section className="panel">
      <h2>这一版包含什么</h2>
      <ul className="about-highlights">
        {HIGHLIGHTS.map(([title, text]) => <li key={title}>
          <strong>{title}</strong>
          <span>{text}</span>
        </li>)}
      </ul>
    </section>

    <section className="panel">
      <h2>版本说明</h2>
      <ul className="about-checklist">
        <li>功能、配置字段和本地缓存格式仍可能调整；升级前建议保留自己的密钥与备份。</li>
        <li>聊天记录可以按设置上传到所连接的服务，事件等结果由服务端处理。</li>
        <li>聊天记录以只读方式访问；应用不提供发送消息或群操作能力。</li>
        <li>可在连接器中检查微信连接状态与配置。</li>
      </ul>
    </section>
    </div>
  </div>;
}
