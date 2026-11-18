import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { type AppSettings, mergeSettings, requireSettingsRevision, settingsSaveError } from './Settings';
import { detectTitle, detectTitleFor, useConnectorDetect } from './connectorDetect';
import wechatIcon from './assets/connectors/wechat.svg';
import whatsappIcon from './assets/connectors/whatsapp.svg';
import telegramIcon from './assets/connectors/telegram.svg';
import slackIcon from './assets/connectors/slack.svg';
import feishuIcon from './assets/connectors/feishu.svg';
import dingtalkIcon from './assets/connectors/dingtalk.svg';
import wecomIcon from './assets/connectors/wecom.svg';
import discordIcon from './assets/connectors/discord.svg';
import lineIcon from './assets/connectors/line.svg';

const connectorIcons: Record<string, string> = {
  微信: wechatIcon, WhatsApp: whatsappIcon, Telegram: telegramIcon, Slack: slackIcon,
  飞书: feishuIcon, 钉钉: dingtalkIcon, 企业微信: wecomIcon, Discord: discordIcon, LINE: lineIcon,
};

export function ConnectorIcon({ name }: { name: string }) {
  const icon = connectorIcons[name];
  return <span className="connector-mark connector-brand" aria-hidden="true">{icon
    ? <img src={icon} alt="" />
    : <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7"><rect x="3" y="5" width="18" height="14" rx="2" /><path d="m4 7 8 6 8-6" /></svg>}</span>;
}

type Scan = { accounts: Array<{ id: string }> };
type KeyStatus = { keys: Array<{ account: string }> };
type Probe = { accounts: number; verifiedAccounts: number };
type WhatsAppStatus = {
  installed: boolean;
  app_path: string | null;
  version: string | null;
  account: string | null;
  name: string | null;
  sessions: number;
  messages: number;
  last_message_at: number | null;
  error: string | null;
};

type Connector = { name: string; description: string; capabilities: string; local?: boolean };
const futureConnectors: Connector[] = [
  { name: '邮件', description: '连接邮箱，整理邮件对话', capabilities: '历史记录 · 邮件夹' },
  { local: true, name: 'Telegram', description: '读取群组与私聊', capabilities: '' },
  { local: true, name: 'Slack', description: '读取工作区频道与私信', capabilities: '' },
  { local: true, name: '飞书', description: '读取群聊与话题', capabilities: '' },
  { local: true, name: '钉钉', description: '读取群消息', capabilities: '' },
  { local: true, name: '企业微信', description: '读取客户群与内部群', capabilities: '' },
  { local: true, name: 'Discord', description: '读取服务器频道', capabilities: '' },
  { name: 'LINE', description: '导入聊天文本', capabilities: '' },
];

export function Connectors({ onOpenWechat, onOpenWhatsApp }: { onOpenWechat: (configured: boolean) => void; onOpenWhatsApp: () => void }) {
  const install = useConnectorDetect('wechat');
  const whatsappInstall = useConnectorDetect('whatsapp');
  const [whatsapp, setWhatsapp] = useState<WhatsAppStatus | null>(null);
  const [whatsappEnabled, setWhatsappEnabled] = useState(true);
  const [whatsappBusy, setWhatsappBusy] = useState(false);
  const [probe, setProbe] = useState<Probe | null>(null);
  const [waitlist, setWaitlist] = useState<string[]>([]);
  const [confirmedWaitlist, setConfirmedWaitlist] = useState<string[]>([]);
  const [enabled, setEnabled] = useState(true);
  const [confirming, setConfirming] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [saveError, setSaveError] = useState('');
  const [loading, setLoading] = useState(isTauri());
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const confirmed = useRef<AppSettings | null>(null);
  const waitlistDraft = useRef<string[]>([]);
  const queue = useRef<Promise<void>>(Promise.resolve());
  const queueFailed = useRef(false);
  const connectionSaving = useRef(false);

  useEffect(() => {
    if (!isTauri()) return;
    void invoke<AppSettings>('app_settings_get')
      .then(stored => {
        const settings = mergeSettings(requireSettingsRevision(stored));
        confirmed.current = settings;
        waitlistDraft.current = settings.connectors.waitlist;
        setSettingsLoaded(true);
        setWaitlist(settings.connectors.waitlist);
        setConfirmedWaitlist(settings.connectors.waitlist);
        setEnabled(settings.connectors.wechat_enabled);
        setWhatsappEnabled(settings.connectors.whatsapp_enabled !== false);
      })
      .catch(reason => setError(`设置加载失败：${String(reason)}，请重新打开页面。`));
  }, []);

  /** Each action is explicit, but rapid actions must use the last confirmed revision. */
  const enqueueConnectorSave = (patch: Partial<AppSettings['connectors']>, onSaved?: () => void) => {
    if (!confirmed.current || queueFailed.current) return;
    queue.current = queue.current.then(async () => {
      if (!confirmed.current || queueFailed.current) return;
      const value = { ...confirmed.current, connectors: { ...confirmed.current.connectors, ...patch } };
      try {
        confirmed.current = mergeSettings(requireSettingsRevision(await invoke<AppSettings>('app_settings_save', { value })));
        setConfirmedWaitlist(confirmed.current.connectors.waitlist);
        onSaved?.();
        setSaveError('');
      } catch (reason) {
        queueFailed.current = true;
        setSaveError(`${settingsSaveError(reason)} 修改已保留，请重新打开页面确认后重试。`);
      }
    }).finally(() => {
      if (onSaved) { connectionSaving.current = false; setSaving(false); }
    });
  };

  /** Remembered locally: there is no notification service, so the label says "已加入". */
  const toggleWaitlist = (name: string) => {
    if (!confirmed.current) return;
    const next = waitlistDraft.current.includes(name) ? waitlistDraft.current.filter(item => item !== name) : [...waitlistDraft.current, name];
    waitlistDraft.current = next;
    setWaitlist(next);
    if (!queueFailed.current) enqueueConnectorSave({ waitlist: next });
  };

  /** Disconnecting stops reading and uploading; cached keys and cloud data are retained. */
  const setConnected = (next: boolean) => {
    if (!isTauri() || !confirmed.current || queueFailed.current || connectionSaving.current) return;
    connectionSaving.current = true;
    setSaving(true);
    enqueueConnectorSave({ wechat_enabled: next }, () => {
      setEnabled(next);
      setConfirming(false);
    });
  };

  const refresh = useCallback(async () => {
    if (!isTauri()) return;
    setLoading(true);
    try {
      const [scan, keys] = await Promise.all([invoke<Scan>('wechat_scan'), invoke<KeyStatus>('wechat_key_status')]);
      const verified = new Set(keys.keys.map(key => key.account));
      setProbe({ accounts: scan.accounts.length, verifiedAccounts: scan.accounts.filter(account => verified.has(account.id)).length });
    } catch (e) {
      setProbe(null);
      setError(`无法检查微信连接状态：${String(e)}`);
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => { void refresh(); }, [refresh]);

  const refreshWhatsapp = useCallback(async () => {
    if (!isTauri()) return;
    setWhatsappBusy(true);
    try {
      setWhatsapp(await invoke<WhatsAppStatus>('whatsapp_status'));
    } catch (e) {
      setWhatsapp(null);
      setError(`无法检查 WhatsApp 连接状态：${String(e)}`);
    } finally {
      setWhatsappBusy(false);
    }
  }, []);
  useEffect(() => { void refreshWhatsapp(); }, [refreshWhatsapp]);

  const setWhatsappConnected = (next: boolean) => {
    if (!isTauri() || !confirmed.current || queueFailed.current) return;
    enqueueConnectorSave({ whatsapp_enabled: next }, () => setWhatsappEnabled(next));
  };

  const status = !isTauri() ? '请在桌面应用中检查' : loading ? '正在检查…' : error ? '状态检查失败' : !probe?.accounts ? '未发现账号' : probe.verifiedAccounts ? `已校验 ${probe.verifiedAccounts} 个账号的密钥` : `发现 ${probe.accounts} 个账号，待校验密钥`;
  const missing = !install.checking && !install.usable && install.detect;
  const whatsappMissing = !whatsappInstall.checking && !whatsappInstall.usable && whatsappInstall.detect;
  const recheckAll = () => { void install.recheck(); void refresh(); };
  const hasHistory = Boolean(probe?.verifiedAccounts);

  return <div className="connector-page">
    <header className="connector-hero">
      <div><p>CONNECTORS</p><h1>连接器</h1><span>连接并管理聊天平台，选择要导入的会话。</span></div>
      <div className="connector-hero-marks" aria-hidden="true">{['微信', 'WhatsApp', '邮件', 'Telegram', 'Slack', '飞书'].map(name => <ConnectorIcon key={name} name={name} />)}</div>
    </header>
    <section aria-labelledby="connector-available">
      <h2 id="connector-available">聊天来源</h2>
      {missing && enabled && hasHistory && <div className="connector-install-banner" role="status">
        <span className="connector-install-icon" aria-hidden="true">!</span>
        <div><strong>未检测到微信，已停止读取新消息</strong><p>已导入的聊天记录仍可查看，云端记录不受影响。重新安装微信后点「重新检查」即可恢复。</p></div>
        <button type="button" className="connector-quiet" onClick={recheckAll} disabled={install.checking}>重新检查</button>
      </div>}
      <div className="connector-grid">
        <article className={`connector-card${enabled ? '' : ' off'}${missing ? ' blocked' : ''}`}>
          <div className="connector-card-head">
            <ConnectorIcon name="微信" />
            <div>
              <h3>微信</h3>
              <p className="connector-state">
                <span className={missing ? 'bad' : enabled && probe?.verifiedAccounts ? 'good' : ''} />
                {install.checking ? '正在检测本机微信…' : missing ? detectTitle[install.detect!.state] : enabled ? status : '已断开 · 停止读取本机消息'}
              </p>
            </div>
          </div>
          {missing && !(enabled && hasHistory) ? <div className="connector-install-check">
            <p>{install.detect!.message}</p>
            {install.detect!.app_path && <p className="connector-install-path">检测位置：{install.detect!.app_path}</p>}
          </div> : <p className="connector-description">
            {enabled
              ? '读取本机数据库，只读，不改动聊天数据'
              : '本机密钥与解密快照仍然保留，重新连接不需要再次导入；云端已上传的记录保持不变。'}
          </p>}
          {!missing && install.detect?.version && <p className="connector-install-meta">微信 {install.detect.version} · 官方签名 · {install.detect.app_path}</p>}
          <p className="connector-capabilities">历史记录 · 群聊 · 增量同步</p>
          <div className="connector-actions">
            {enabled ? <>
              <button type="button" className="connector-primary" onClick={() => onOpenWechat(Boolean(probe?.verifiedAccounts))} disabled={!install.usable} title={missing ? '请先安装微信' : undefined}>{probe?.verifiedAccounts ? '重新配置' : '打开连接向导'}</button>
              {isTauri() && <button type="button" className="connector-quiet" onClick={recheckAll} disabled={loading || install.checking}>重新检查</button>}
              {missing && install.detect!.state === 'not_installed' && <a className="connector-link" href="https://pc.weixin.qq.com/" target="_blank" rel="noreferrer">前往下载微信 ↗</a>}
              {isTauri() && (!missing || hasHistory) && <button type="button" className="connector-quiet danger" onClick={() => setConfirming(true)} disabled={saving || !settingsLoaded}>断开连接</button>}
            </> : <>
              <button type="button" className="connector-primary" onClick={() => void setConnected(true)} disabled={saving || !settingsLoaded}>{saving ? '连接中…' : '重新连接'}</button>
              {isTauri() && <button type="button" className="connector-quiet" onClick={() => void refresh()} disabled={loading}>重新检查</button>}
            </>}
          </div>
          {enabled && confirming && <div className="connector-confirm" role="dialog" aria-label="确认断开微信连接">
            <p>断开后本机不再读取微信消息，也不会自动备份。</p>
            <ul>
              <li>保留本机密钥与解密快照，重新连接无需再次导入</li>
              <li>停止聊天读取、搜索、整理与自动备份</li>
              <li>云端已上传的记录不会删除</li>
            </ul>
            <div className="connector-actions">
              <button type="button" className="connector-danger" onClick={() => void setConnected(false)} disabled={saving || !settingsLoaded}>{saving ? '断开中…' : '确认断开'}</button>
              <button type="button" className="connector-quiet" onClick={() => setConfirming(false)} disabled={saving || !settingsLoaded}>取消</button>
            </div>
          </div>}
        </article>
        <article className={`connector-card${whatsappEnabled ? '' : ' off'}${whatsappMissing ? ' blocked' : ''}`}>
          <div className="connector-card-head">
            <ConnectorIcon name="WhatsApp" />
            <div>
              <h3>WhatsApp</h3>
              <p className="connector-state">
                <span className={whatsappMissing ? 'bad' : whatsappEnabled && whatsapp?.sessions ? 'good' : ''} />
                {whatsappInstall.checking || whatsappBusy ? '正在检测本机 WhatsApp…'
                  : whatsappMissing ? detectTitleFor('WhatsApp', whatsappInstall.detect!.state)
                    : !whatsappEnabled ? '已断开 · 停止读取本机消息'
                      : whatsapp?.error ? '读取失败'
                        : whatsapp?.sessions ? `${whatsapp.name ?? '已登录'} · ${whatsapp.sessions} 个会话 · ${whatsapp.messages} 条消息` : '已连接 · 还没有会话'}
              </p>
            </div>
          </div>
          {whatsappMissing ? <div className="connector-install-check">
            <p>{whatsappInstall.detect!.message}</p>
            {whatsappInstall.detect!.app_path && <p className="connector-install-path">检测位置：{whatsappInstall.detect!.app_path}</p>}
          </div> : <p className="connector-description">
            {whatsappEnabled
              ? '读取本机 WhatsApp 数据库，只读，不改动聊天数据；不需要密钥或解密。'
              : '已断开读取；本机聊天数据不会被修改，重新连接即可继续读取。'}
          </p>}
          {!whatsappMissing && whatsappInstall.detect?.version && <p className="connector-install-meta">WhatsApp {whatsappInstall.detect.version} · 官方签名 · {whatsappInstall.detect.app_path}</p>}
          {whatsapp?.error && <p className="connector-error" role="alert">{whatsapp.error}</p>}
          <p className="connector-capabilities">历史记录 · 群聊 · 手动备份</p>
          <div className="connector-actions">
            {whatsappEnabled ? <>
              <button type="button" className="connector-primary" onClick={onOpenWhatsApp} disabled={!whatsappInstall.usable || whatsappBusy} title={whatsappMissing ? '请先安装 WhatsApp' : undefined}>查看聊天</button>
              {isTauri() && <button type="button" className="connector-quiet" onClick={() => { void whatsappInstall.recheck(); void refreshWhatsapp(); }} disabled={whatsappBusy || whatsappInstall.checking}>重新检查</button>}
              {whatsappMissing && whatsappInstall.detect!.state === 'not_installed' && <a className="connector-link" href="https://www.whatsapp.com/download" target="_blank" rel="noreferrer">前往下载 WhatsApp ↗</a>}
              {isTauri() && !whatsappMissing && <button type="button" className="connector-quiet danger" onClick={() => setWhatsappConnected(false)} disabled={saving || !settingsLoaded}>断开连接</button>}
            </> : <>
              <button type="button" className="connector-primary" onClick={() => setWhatsappConnected(true)} disabled={saving || !settingsLoaded}>{saving ? '连接中…' : '重新连接'}</button>
              {isTauri() && <button type="button" className="connector-quiet" onClick={() => void refreshWhatsapp()} disabled={whatsappBusy}>重新检查</button>}
            </>}
          </div>
        </article>
      </div>
      {error && <p className="connector-error" role="alert">{error}</p>}
      {saveError && <p className="connector-error" role="alert">{saveError}</p>}
    </section>
    <section aria-labelledby="connector-upcoming">
      <div className="connector-section-head"><h2 id="connector-upcoming">其他来源</h2></div>
      <p className="connector-section-description">未来可连接更多平台，统一管理你的聊天记录。</p>
      <div className="connector-grid">{futureConnectors.map(connector => <article key={connector.name} className="connector-card future">
        <div className="connector-card-head"><ConnectorIcon name={connector.name} /><div><h3>{connector.name}</h3><p className="connector-state">尚未接入</p></div></div>
        <p className="connector-description">{connector.description}</p>
        {connector.capabilities && <p className="connector-capabilities">{connector.capabilities}</p>}
        <p className="connector-install-meta">{connector.local ? '接入时会先检测本机是否安装桌面版' : '无需安装本机应用'}</p>
        <div className="connector-actions"><button
          type="button"
          className={`connector-notify${waitlist.includes(connector.name) ? ' on' : ''}`}
          aria-pressed={waitlist.includes(connector.name)}
          disabled={!settingsLoaded}
          onClick={() => toggleWaitlist(connector.name)}
        >{waitlist.includes(connector.name) !== confirmedWaitlist.includes(connector.name) ? (waitlist.includes(connector.name) ? '已加入（未保存）' : '未加入（未保存）') : waitlist.includes(connector.name) ? '已加入，上线通知你' : '通知我'}</button></div>
      </article>)}</div>
    </section>
  </div>;
}
