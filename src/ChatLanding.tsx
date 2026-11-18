import { useEffect, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { ConnectorIcon } from './Connectors';
import { currentAccount, detectTitleFor, useConnectorDetect, type ChatAccount } from './connectorDetect';

type HomeMode = 'new' | 'connected';
type KeyStatus = { keys?: Array<{ account: string }> };
type Settings = { connectors?: { wechat_enabled?: boolean; whatsapp_enabled?: boolean }; active_account_id?: string; chat?: { allowed_usernames?: Record<string, string[]>; sync_interval_seconds?: number }; devify?: { active_space_id?: string | null } };

type Step = { number: string; title: string; description: string; state: 'current' | 'next' | 'done' };
type Connector = { name: string; description: string };
const connectors: Connector[] = [
  { name: '微信', description: '读取本机微信聊天记录，也可以导入 JSON 文件' },
  { name: 'WhatsApp', description: '读取本机 WhatsApp 聊天记录，无需密钥' },
  { name: '邮件', description: '用只读授权连接 Gmail、Outlook 或 IMAP 邮箱' },
];

function steps(mode: HomeMode, source: string, sessions = 0, syncIntervalSeconds = 1800): Step[] {
  const current = mode === 'new' ? 0 : sessions > 0 ? 3 : 1;
  const minutes = Math.max(1, Math.round(syncIntervalSeconds / 60));
  const step = (index: number): Step['state'] => index < current ? 'done' : index === current ? 'current' : 'next';
  return [
    { number: '1', title: '连接来源', description: mode === 'new' ? '选一个聊天来源，打开连接向导，按提示授权。会逐步检查权限。' : `已连接：${source}`, state: step(0) },
    { number: '2', title: '选择要备份的会话', description: mode === 'new' ? '在导入管理里勾选要备份的私聊和群聊，未勾选的不会导入。' : sessions ? `已选 ${sessions.toLocaleString()} 个会话` : '还没有选择会话，先去勾选要备份的聊天。', state: step(1) },
    { number: '3', title: '设置备份周期', description: mode === 'new' ? '每个平台单独设置，默认每 30 分钟。' : `${source}：每 ${minutes} 分钟 · 仅 Wi-Fi`, state: step(2) },
    { number: '4', title: '第一次备份', description: mode === 'new' ? '把已选会话的记录加密上传，完成后就能看到事件、总结和报告。' : '完成后就能看到事件、总结和报告', state: step(3) },
  ];
}

export function ChatLanding({ mode, source = '微信', sessionCount, syncIntervalSeconds, onConnect, onConnectWhatsApp, onImport, onBackup, onSchedule }: { mode: HomeMode; source?: string; sessionCount: number; syncIntervalSeconds: number; onConnect: (connector?: string) => void; onConnectWhatsApp: () => void; onImport: () => void; onBackup: () => void; onSchedule: () => void }) {
  const isNew = mode === 'new';
  const wechat = useConnectorDetect('wechat');
  const whatsapp = useConnectorDetect('whatsapp');
  const wechatMissing = !wechat.checking && !wechat.usable && wechat.detect;
  const whatsappMissing = !whatsapp.checking && !whatsapp.usable && whatsapp.detect;
  const needsImport = !isNew && sessionCount === 0;
  const summary = needsImport
    ? '解密只是读出本机记录。备份哪些聊天，需要你在导入管理里勾选。'
    : `将备份${source}的 ${sessionCount.toLocaleString()} 个会话。`;
  return <div className="chat-landing">
    <div className="chat-landing-inner">
      <header className="chat-landing-head">
        <p className="chat-landing-eyebrow">{isNew ? '欢迎使用 AIMyChats' : '还差最后一步'}</p>
        <h1>{isNew ? '先连接一个聊天来源' : needsImport ? '已连接，还没有选择要备份的会话' : '已连接，还没有备份过任何内容'}</h1>
        <p>{isNew ? 'AIMyChats 在你的电脑上只读取聊天记录，加密备份后，再帮你整理成事件、总结和报告。数据留在本机，随时可以断开。' : needsImport ? '未勾选的聊天不会导入，也不会出现在备份里。' : '备份完成前，这里还没有可看的内容。现在做第一次备份，之后会按你设置的周期自动进行。'}</p>
      </header>

      {isNew ? <section className="chat-landing-connectors" aria-label="连接聊天来源">
        {connectors.map(connector => {
          const isWechat = connector.name === '微信';
          const isWhatsApp = connector.name === 'WhatsApp';
          const detect = isWhatsApp ? whatsapp : wechat;
          const canProceed = isWechat ? detect.canProceed : detect.usable;
          const missing = !detect.checking && !canProceed ? detect.detect : null;
          return <article className={`chat-landing-connector${missing ? ' blocked' : ''}`} key={connector.name}>
            <ConnectorIcon name={connector.name} />
            <div>
              <h2>{connector.name}</h2>
              {missing ? <p className="chat-landing-install"><span />{detectTitleFor(connector.name, missing.state)}：{missing.message}</p>
                : <p>{detect.checking ? `正在检测本机${connector.name}…` : connector.description}</p>}
              {missing && <p className="chat-landing-install-actions">
                <button type="button" onClick={() => void detect.recheck()}>重新检查</button>
                {missing.state === 'not_installed' && <a href={isWhatsApp ? 'https://www.whatsapp.com/download' : 'https://pc.weixin.qq.com/'} target="_blank" rel="noreferrer">前往下载{connector.name} ↗</a>}
              </p>}
            </div>
            {isWhatsApp
              ? <button type="button" className="chat-landing-primary" onClick={onConnectWhatsApp} disabled={!detect.usable && isTauri()}>{detect.usable ? '打开聊天' : '查看连接器'}</button>
              : <button type="button" className="chat-landing-primary" onClick={() => onConnect(connector.name)} disabled={isWechat && !detect.canProceed && isTauri()}>{isWechat ? '打开连接向导' : '查看连接器'}</button>}
          </article>;
        })}
      </section> : <section className="chat-landing-first-backup">
        <p>{summary}</p>
        <div className="chat-landing-actions">{needsImport
          ? <><button type="button" className="chat-landing-primary" onClick={onImport}>选择要备份的会话</button><button type="button" className="chat-landing-link" onClick={onSchedule}>设置备份周期</button></>
          : <><button type="button" className="chat-landing-primary" onClick={onBackup}>立即备份</button><button type="button" className="chat-landing-link" onClick={onImport}>调整导入范围</button><button type="button" className="chat-landing-link" onClick={onSchedule}>设置备份周期</button></>}</div>
      </section>}

      <section className="chat-landing-steps" aria-labelledby="chat-landing-steps-title">
        <h2 id="chat-landing-steps-title">开始前的四步</h2>
        <ol>{steps(mode, source, sessionCount, syncIntervalSeconds).map(step => <li key={step.number} className={step.state}><span>{step.state === 'done' ? '✓' : step.number}</span><div><strong>{step.title}</strong><p>{step.description}</p></div></li>)}</ol>
      </section>
      <p className="chat-landing-after">完成后，「聊天」这里会显示：按会话整理的聊天记录，可以搜索、查看原文。</p>
    </div>
  </div>;
}

export function ChatEntry({ onConnect, onConnectWhatsApp, onImport, onBackup, onSchedule, onReady, onUnlock }: { onConnect: (connector?: string) => void; onConnectWhatsApp: () => void; onImport: () => void; onBackup: () => void; onSchedule: () => void; onReady: (source: string) => void; onUnlock: () => void }) {
  const [mode, setMode] = useState<HomeMode | null>(isTauri() ? null : 'new');
  const [source, setSource] = useState('微信');
  const [syncIntervalSeconds, setSyncIntervalSeconds] = useState(1800);
  const [sessionCount, setSessionCount] = useState(0);

  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    void (async () => {
      try {
        const [found, settings, keyStatus] = await Promise.all([
          invoke<ChatAccount[]>('chat_accounts'),
          invoke<Settings>('app_settings_get'),
          invoke<KeyStatus>('wechat_key_status'),
        ]);
        if (!active) return;
        const connections = settings.connectors ?? {};
        const accounts = (found ?? []).filter(account => account.source === 'whatsapp'
          ? connections.whatsapp_enabled !== false
          : connections.wechat_enabled !== false);
        const account = currentAccount(accounts, settings.active_account_id);
        if (!account) { setMode('new'); return; }
        const interval = settings.chat?.sync_interval_seconds ?? 1800;
        setSource(account.source === 'whatsapp' ? 'WhatsApp' : '微信');
        let sessions = 0;
        let sessionsFailed = false;
        try { sessions = (await invoke<Array<{ username: string }>>('wechat_sessions', { accountId: account.id })).length; } catch { sessionsFailed = true; }
        if (!active) return;
        // WhatsApp needs no key: a failed read means permissions or a new schema, not a
        // missing key, so it must never fall into the WeChat unlock wizard.
        const locked = account.source !== 'whatsapp'
          && (((account.encrypted_count ?? 0) > 0 && !(keyStatus.keys ?? []).some(key => key.account === account.id)) || sessionsFailed);
        if (locked) { onUnlock(); setMode(null); return; }
        setSyncIntervalSeconds(Math.max(60, interval));
        setSessionCount(sessions);
        if (sessions > 0) onReady(account.source ?? 'wechat');
        setMode(sessions > 0 ? null : 'connected');
      } catch {
        if (active) setMode('new');
      }
    })();
    return () => { active = false; };
  }, [onReady, onUnlock]);

  if (!mode) return <div className="chat-landing chat-landing-checking"><div className="chat-landing-checking-card"><span className="chat-landing-checking-dot" /><p>正在检查本地聊天数据…</p></div></div>;
  return <ChatLanding mode={mode} source={source} sessionCount={sessionCount} syncIntervalSeconds={syncIntervalSeconds} onConnect={onConnect} onConnectWhatsApp={onConnectWhatsApp} onImport={onImport} onBackup={onBackup} onSchedule={onSchedule} />;
}
