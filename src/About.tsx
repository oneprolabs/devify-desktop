import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import appIcon from './assets/app-icon.png';

type AppInfo = {
  name: string;
  version: string;
  stage: string;
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
  ['本地整理与总结', '规则抽取待办、决定和卡点，或交给本机 CLI Agent 生成 Markdown 总结。'],
  ['每日自动日报', '按白名单和时间策略生成日报，经 SMTP 投递到你自己的 Devify 收件邮箱。'],
  ['Agent 集成', '一边配置本地总结 Agent，一边把只读 CLI 与 Skills 安装到你正在使用的 Agent。'],
];

function platformLabel(platform: string, arch: string): string {
  const name = PLATFORM_LABEL[platform] ?? platform;
  const chip = ARCH_LABEL[arch] ?? arch;
  return `${name} · ${chip}`;
}

export function About() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [error, setError] = useState('');

  useEffect(() => {
    invoke<AppInfo>('app_info')
      .then(setInfo)
      .catch(value => setError(`读取版本信息失败：${String(value)}`));
  }, []);

  const version = info ? `${info.stage} ${info.version}` : '正在读取版本…';

  return <div className="settings-page about-page">
    <header className="page-head about-hero">
      <div className="about-lockup">
        <img className="about-mark" src={appIcon} alt="" width={44} height={44} />
        <h2>{info?.name ?? 'AImyChats'}</h2>
      </div>
      <p className="about-tagline">
        本地优先的微信工作台：只读查看白名单聊天，整理工作内容，并把日报交付到自己的 Devify 收件箱。
      </p>
      <span className="about-version">{version}</span>
    </header>

    <section className="panel">
      <h2>版本信息</h2>
      <dl className="about-meta">
        <div>
          <dt>版本</dt>
          <dd>{info ? `${info.stage} ${info.version}` : '—'}</dd>
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
      {error && <div className="error">{error}</div>}
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
      <h2>Alpha 阶段说明</h2>
      <ul className="about-checklist">
        <li>功能、配置字段和本地缓存格式仍可能调整；升级前建议保留自己的密钥与备份。</li>
        <li>所有聊天解析、搜索和总结都在本机完成；只有你主动生成或投递时，内容才会交给所选 Agent 或 SMTP 服务。</li>
        <li>应用不会修改微信数据库、不会 hook 微信、不会代替你登录，也不提供写消息或群操作能力。</li>
        <li>遇到问题时，「本地数据」页可以查看密钥状态并清理解密缓存；清理后需重新解密才能查看聊天。</li>
      </ul>
    </section>
  </div>;
}
