import { t, tf, ctf } from '../../shared/i18n/locale';
import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import appIcon from '../../assets/app-icon.png';
import appMark from '../../assets/app-mark-transparent.png';
import { APP_IDENTIFIER } from '../../shared/api/buildFlavor';
import { checkForUpdates, dismissInstallBlockers, downloadUpdate, installUpdate, RELEASE_URL, useUpdater } from '../updates/updater';

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
  ['待办与总结', '查看云端整理的待办，以及本地保存的聊天总结。'],
  ['Agent 集成', '配置本地 Agent 并管理相关集成。'],
];

function platformLabel(platform: string, arch: string): string {
  const name = PLATFORM_LABEL[platform] ?? platform;
  const chip = ARCH_LABEL[arch] ?? arch;
  return `${name} · ${chip}`;
}

const RELEASE_NOTES: Record<string, string> = {
  "WeChat setup stays on the current step when you return to the app. If WeChat’s signature cannot be verified, you can still import a trusted key file and finish decrypting your chats. Feedback and About are available throughout setup.": "切回客户端时，微信连接向导会停留在当前步骤。即使无法验证微信签名，也可以导入可信密钥文件并完成解密。配置期间可随时打开「问题反馈」和「关于」。",
  "Feedback reports keep the host details when a server cannot accept diagnostics, and the form now says so when the log is not uploaded. Includes the English interface from 0.1.12.": "反馈在服务端收不到诊断信息时，仍会保留本机上的详情；日志没有上传时，表单会说明这一点。这一版也包含 0.1.12 的英文界面。",
  "Setup steps no longer flash a blank panel when you continue. Signing out now cancels a running backup right away instead of blocking the logout, and the next sign-in resumes it. Preparing a large chat history is faster because existing cloud message keys are read in parallel.": "继续设置时不再闪过空白页。退出登录会马上取消正在进行的备份，不再挡住退出；下次登录会接着备份。准备大量聊天记录时会并行读取已有的云端消息标识，因此更快。",
  "Update notes follow the interface language, and About describes the version you are about to install. English mode covers the Ask time range, report filters, and built-in topic titles. The update button sits on the side rail, and video messages use the same bubble as images.": "更新说明会跟着界面语言切换，关于页会写即将安装的这一版包含什么。英文界面补上了问答的时间范围、报告筛选和内置主题标题。更新按钮改到侧栏，视频消息和图片用同一种气泡。",
};

function updateNoteLines(notes?: string): string[] {
  if (!notes?.trim()) return [];
  const plain = notes.replace(/\s+/g, ' ').trim();
  const known = RELEASE_NOTES[plain];
  if (known) return [t(known)];
  if (/whatsapp/i.test(plain) && /whitelist/i.test(plain) && /group header|groups sessions by source/i.test(plain)) {
    return [
      t("新增 WhatsApp 只读来源，可浏览、搜索并上传本机文字会话，白名单规则与微信相同。"),
      t("会话列表按来源分组；只有一个来源时不显示分组标题。"),
    ];
  }
  return notes.split(/\n+/).map(line => line.replace(/^[-*]\s+/, '').replace(/[`#]/g, '').trim()).filter(Boolean);
}

export function UpdatePanel({ onClose, compact = false }: { onClose?: () => void; compact?: boolean } = {}) {
  const update = useUpdater();
  const [error, setError] = useState('');
  const busy = ['checking', 'downloading', 'installing'].includes(update.status);
  const notes = updateNoteLines(update.notes);
  const status = update.status === 'disabled' ? t('开发版不检查或安装正式更新。')
    : update.status === 'checking' ? t("正在检查更新…")
    : update.status === 'latest' ? t("当前已是最新版本")
    : update.status === 'available' ? tf("可以更新到 {0}", update.version)
    : update.status === 'downloading' ? t("正在下载更新…")
    : update.status === 'downloaded' ? tf("AIMyChats {0} 已下载", update.version)
    : update.status === 'installed' ? t("更新已安装，请重启应用")
    : update.status === 'installing' ? t("正在安装更新…")
    : update.status === 'error' ? t("检查更新失败")
    : t("检查软件更新");

  return <div className={`update-panel${compact ? ' update-panel-compact' : ''}`}>
    <div className="update-panel-identity" role="status">
      {!compact && <img src={appIcon} alt="" width={46} height={46} />}
      <div>
        <strong>{status}</strong>
        {!compact && update.currentVersion && <span>{t("当前版本")} {update.currentVersion}</span>}
      </div>
      <div className="about-update-actions">
        {update.status === 'available' && onClose && <button type="button" onClick={onClose}>{t("稍后")}</button>}
        {['idle', 'latest', 'error'].includes(update.status) && <button type="button" className="update-primary" onClick={() => void checkForUpdates()}>{t("检查更新")}</button>}
        {update.status === 'available' && <button type="button" className="update-primary" onClick={() => void downloadUpdate()}>{update.error ? t("重新下载") : t("下载更新")}</button>}
        {['downloaded', 'installed'].includes(update.status) && !update.blockers?.length && <button type="button" className="update-primary" onClick={() => void installUpdate()}>{update.status === 'installed' ? t("重新启动") : t("安装并重启")}</button>}
        {busy && <span className="update-spinner" aria-hidden="true"/>}
      </div>
    </div>
    {notes.length > 0 && <section className="update-release-notes" aria-label={t("这次更新")}>
      <h3>{t("这次更新")}</h3>
      <ul className="update-note-list">{notes.map(line => <li key={line}>{line}</li>)}</ul>
    </section>}
    {update.status === 'available' && <p className="update-next-step">{t("下载完成后，会再请你确认安装并重启。")}</p>}
    {update.status === 'downloading' && <div className="update-download">
      <progress value={update.progress} max={100} aria-label={t("下载进度")} />
      {update.progress != null && <span>{update.progress}%</span>}
    </div>}
    {(update.error || error) && <p className="about-update-sub is-error" role="alert">{update.error || error}</p>}
    {update.blockers?.length ? <div className="about-update-blocked" role="alert">
      <span>{t("有上传正在进行。现在安装会中断上传，重启后可在「任务」页重试。")}</span>
      <button type="button" onClick={dismissInstallBlockers}>{t("稍后")}</button>
      <button type="button" className="update-primary" onClick={() => void installUpdate(true)}>{t("仍然安装")}</button>
    </div> : null}
    {update.status !== 'disabled' && <div className="update-panel-footer">
      <button type="button" className="update-link" onClick={() => {
        setError('');
        const releaseUrl = update.version && /^\d+\.\d+\.\d+(?:-[\w.-]+)?$/.test(update.version)
          ? RELEASE_URL.replace(/alpha$/, `v${update.version}`)
          : RELEASE_URL;
        void invoke('devify_auth_open', { url: releaseUrl }).catch(value => setError(tf("打开下载页面失败：{0}", String(value))));
      }}>{t("手动下载")}</button>
    </div>}
  </div>;
}

export function About() {
  const update = useUpdater();
  const [updateLater, setUpdateLater] = useState(false);
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [error, setError] = useState('');
  const hideOffer = updateLater && update.status === 'available';
  const releaseNotes = updateNoteLines(update.notes);
  const showingRelease = releaseNotes.length > 0 && ['available', 'downloading', 'downloaded', 'installing', 'installed'].includes(update.status);

  useEffect(() => {
    invoke<AppInfo>('app_info')
      .then(setInfo)
      .catch(value => setError(tf("读取版本信息失败：{0}", String(value))));
  }, []);


  return <div className="settings-page settings-refreshed about-page">
    <header className="page-head"><h2>{t("关于")}</h2><p className="help">{t("了解 AIMyChats 应用和版本信息。")}</p></header>
    {error && <div className="error" role="alert">{t(error)}</div>}

    <div className="about-details">
    <section className="panel about-product-panel">
      <div className="about-product-identity"><img src={appMark} alt="" width={52} height={52}/><div><strong>{info?.name || 'AIMyChats'}</strong><p>{info ? tf('版本 {0}', info.version) : t('正在读取版本…')}{info && ` · ${platformLabel(info.platform, info.arch)}`}</p></div></div>
      {!hideOffer && <UpdatePanel compact onClose={() => setUpdateLater(true)} />}
      <details className="about-app-details"><summary>{t('应用信息')}</summary><dl className="about-meta"><div><dt>{t('应用标识')}</dt><dd><code>{APP_IDENTIFIER}</code></dd></div></dl></details>
    </section>

    <section className="panel">
      <h2>{showingRelease ? tf("{0} 包含什么", update.version) : t("这一版包含什么")}</h2>
      {showingRelease ? <ul className="about-checklist">
        {releaseNotes.map(line => <li key={line}>{line}</li>)}
      </ul> : <ul className="about-highlights">
        {HIGHLIGHTS.map(([title, text]) => <li key={title}>
          <strong>{t(title)}</strong>
          <span>{t(text)}</span>
        </li>)}
      </ul>}
    </section>

    <details className="panel about-usage-notes">
      <summary>{t("版本说明")}</summary>
      <ul className="about-checklist">
        <li>{t("功能、配置字段和本地缓存格式仍可能调整；升级前建议保留自己的密钥与备份。")}</li>
        <li>{t("聊天记录可以按设置上传到所连接的服务，待办等结果由服务端处理。")}</li>
        <li>{t("聊天记录以只读方式访问；应用不提供发送消息或群操作能力。")}</li>
        <li>{t("可在连接器中检查微信连接状态与配置。")}</li>
      </ul>
    </details>
    </div>
  </div>;
}
