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

export function UpdatePanel() {
  const update = useUpdater();
  const [error, setError] = useState('');
  const busyUpdate = ['checking', 'downloading', 'installing'].includes(update.status);
  const updateStatus = update.status === 'checking' ? '正在检查更新…'
    : update.status === 'latest' ? '当前已是最新版本'
    : update.status === 'available' ? `发现新版本 ${update.version}`
    : update.status === 'downloading' ? `正在下载${update.progress == null ? '…' : ` ${update.progress}%`}`
    : update.status === 'downloaded' ? `版本 ${update.version} 已下载`
    : update.status === 'installed' ? '更新已安装，请重启应用'
    : update.status === 'installing' ? '正在安装更新…'
    : update.status === 'error' ? '检查更新失败'
    : '检查软件更新';

  return <>{error && <p role="alert">{error}</p>}
<div className="about-update" aria-live="polite" aria-busy={busyUpdate}>
        <div className="about-update-status">
          {busyUpdate
            ? <span className="update-spinner" aria-hidden="true"/>
            : update.status !== 'idle' && <span className={`update-dot${update.status === 'latest' ? ' is-ok' : update.status === 'available' ? ' is-new' : update.status === 'error' ? ' is-error' : ''}`} aria-hidden="true"/>}
          <div className="about-update-status-text">
            <strong>{updateStatus}</strong>
            {update.error
              ? <span className="about-update-sub is-error" role="alert">{update.error}</span>
              : update.notes && <span className="about-update-sub">{update.notes}</span>}
          </div>
        </div>
        <div className="about-update-actions">
          {['idle', 'latest', 'error'].includes(update.status) && <button type="button" onClick={() => void checkForUpdates()}>检查更新</button>}
          {update.status === 'available' && <button type="button" className="update-primary" onClick={() => void downloadUpdate()}>下载更新</button>}
          {['downloaded', 'installed'].includes(update.status) && !update.blockers?.length && <button type="button" className="update-primary" onClick={() => void installUpdate()}>{update.status === 'installed' ? '重新启动' : '安装并重启'}</button>}
          <button type="button" className="update-link" onClick={() => void invoke('devify_auth_open', { url: RELEASE_URL }).catch(value => setError(`打开下载页面失败：${String(value)}`))}>手动下载</button>
        </div>
        {update.blockers?.length ? <div className="about-update-blocked" role="alert">
          <span>正在上传（{update.blockers.join('、')}）。现在安装会中断这次上传，重启后可在「任务」页重试。</span>
          <button type="button" className="update-primary" onClick={() => void installUpdate(true)}>仍然安装</button>
          <button type="button" onClick={dismissInstallBlockers}>稍后</button>
        </div> : null}
        {update.status === 'downloading' && update.progress != null && <div className="about-update-progress" role="progressbar" aria-valuenow={update.progress} aria-valuemin={0} aria-valuemax={100}><span style={{ transform: `scaleX(${update.progress / 100})` }}/></div>}
      </div></>;
}

export function About() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [error, setError] = useState('');

  useEffect(() => {
    invoke<AppInfo>('app_info')
      .then(setInfo)
      .catch(value => setError(`读取版本信息失败：${String(value)}`));
  }, []);


  return <div className="settings-page about-page">
    <header className="page-head"><h2>关于</h2><p className="help">了解 AIMyChats 应用和版本信息。</p></header>
    <div className="about-hero">
      <div className="about-lockup">
        <img className="about-mark" src={appIcon} alt="" width={44} height={44} />
        <h2>{info?.name ?? 'AIMyChats'}</h2>
      </div>
      <p className="about-tagline">查看和备份微信聊天记录，整理事件与总结。</p>
      {error && <div className="error" role="alert">{error}</div>}
    </div>

    <div className="about-details">
    <section className="panel">
      <h2>版本信息</h2>
      <UpdatePanel />
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
