import { t, tf, ctf } from '../../shared/i18n/locale';
import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { type AppSettings, mergeSettings, requireSettingsRevision, settingsSaveError } from '../settings/Settings';
import { detectTitle, detectTitleFor, useConnectorDetect } from './connectorDetect';
import { useTaskRuntime } from '../tasks/taskRuntime';
import wechatIcon from '../../assets/connectors/wechat.svg';
import whatsappIcon from '../../assets/connectors/whatsapp.svg';
import telegramIcon from '../../assets/connectors/telegram.svg';
import slackIcon from '../../assets/connectors/slack.svg';
import feishuIcon from '../../assets/connectors/feishu.svg';
import dingtalkIcon from '../../assets/connectors/dingtalk.svg';
import wecomIcon from '../../assets/connectors/wecom.svg';
import discordIcon from '../../assets/connectors/discord.svg';
import lineIcon from '../../assets/connectors/line.svg';

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

function ConnectorTile({ name }: { name: string }) {
  return <span className="connector-tile connector-tile-brand" aria-hidden="true">
    <img src={connectorIcons[name]} alt="" />
  </span>;
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

type Connector = { name: string; description: string; capabilities: string };
const futureConnectors: Connector[] = [
  { name: 'Telegram', description: '读取群组与私聊', capabilities: '' },
  { name: 'Slack', description: '读取工作区频道与私信', capabilities: '' },
  { name: '飞书', description: '读取群聊与话题', capabilities: '' },
  { name: '钉钉', description: '读取群消息', capabilities: '' },
  { name: '企业微信', description: '读取客户群与内部群', capabilities: '' },
  { name: 'Discord', description: '读取服务器频道', capabilities: '' },
  { name: 'LINE', description: '导入聊天文本', capabilities: '' },
];

export function Connectors({ onOpenWechat, onOpenWhatsApp }: { onOpenWechat: (configured: boolean, resumeAccountId?: string) => void; onOpenWhatsApp: (resumeAccountId?: string) => void }) {
  const runtime = useTaskRuntime();
  const wechatIncompleteSource = (runtime.sources ?? []).find(source => source.platform === 'wechat' && !source.setup_complete);
  const wechatIncomplete = Boolean(wechatIncompleteSource);
  const install = useConnectorDetect('wechat');
  const whatsappInstall = useConnectorDetect('whatsapp');
  const [whatsapp, setWhatsapp] = useState<WhatsAppStatus | null>(null);
  const whatsappIncomplete = (runtime.sources ?? []).some(source => source.account_id === whatsapp?.account && !source.setup_complete);
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
      .catch(reason => setError(tf("设置加载失败：{0}，请重新打开页面。", String(reason))));
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
        setSaveError(tf("{0} 修改已保留，请重新打开页面确认后重试。", settingsSaveError(reason)));
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
      setError(tf("无法检查微信连接状态：{0}", String(e)));
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
      setError(tf("无法检查 WhatsApp 连接状态：{0}", String(e)));
    } finally {
      setWhatsappBusy(false);
    }
  }, []);
  useEffect(() => { void refreshWhatsapp(); }, [refreshWhatsapp]);

  const setWhatsappConnected = (next: boolean) => {
    if (!isTauri() || !confirmed.current || queueFailed.current) return;
    enqueueConnectorSave({ whatsapp_enabled: next }, () => setWhatsappEnabled(next));
  };

  const status = !isTauri() ? t("请在桌面应用中检查") : loading ? t("正在检查…") : error ? t("状态检查失败") : !probe?.accounts ? t("未发现账号") : probe.verifiedAccounts ? tf("已校验 {0} 个账号的密钥", probe.verifiedAccounts) : tf("发现 {0} 个账号，待校验密钥", probe.accounts);
  const missing = !install.checking && !install.canProceed && install.detect;
  const signatureWarning = !install.checking && install.detect?.state === 'untrusted';
  const whatsappMissing = !whatsappInstall.checking && !whatsappInstall.usable && whatsappInstall.detect;
  const recheckAll = () => { void install.recheck(); void refresh(); };
  const hasHistory = Boolean(probe?.verifiedAccounts);

  const interestButton = (name: string) => <button
    type="button"
    className={`connector-notify${waitlist.includes(name) ? ' on' : ''}`}
    aria-label={`${waitlist.includes(name) ? t("取消关注") : t("关注")}${name}`}
    aria-pressed={waitlist.includes(name)}
    disabled={!settingsLoaded}
    onClick={() => toggleWaitlist(name)}
  >{waitlist.includes(name) !== confirmedWaitlist.includes(name) ? (waitlist.includes(name) ? t("已关注（未保存）") : t("未关注（未保存）")) : waitlist.includes(name) ? t("已关注") : t("关注进展")}</button>;

  const futureCard = (connector: Connector) => <article key={connector.name} className="connector-card future">
    <div className="connector-card-head"><ConnectorTile name={connector.name} /><div><h3>{t(connector.name)}</h3><p className="connector-state">{t("尚未接入")}</p></div></div>
    <p className="connector-description">{t(connector.description)}</p>
    {connector.capabilities && <p className="connector-capabilities">{connector.capabilities}</p>}
    <div className="connector-actions">{interestButton(connector.name)}</div>
  </article>;

  return <div className="connector-page connector-catalog">
    <header className="connector-hero">
      <div className="connector-hero-copy"><p>CONNECTORS</p><h1>{t("你所有的对话，汇到一处")}</h1><span>{t("微信和 WhatsApp 已开放。想要其他来源，可以先关注，后续在这里查看接入进展。")}</span></div>
      <div className="connector-hero-marks" aria-hidden="true">{['微信', 'WhatsApp', 'Telegram', 'Slack', '飞书'].map(name => <ConnectorTile key={name} name={name} />)}</div>
    </header>
    <section aria-labelledby="connector-available">
      <h2 id="connector-available">{t("已开放")}</h2>
      {missing && enabled && hasHistory && <div className="connector-install-banner" role="status">
        <span className="connector-install-icon" aria-hidden="true">!</span>
        <div><strong>{t("未检测到微信，已停止读取新消息")}</strong><p>{t("已导入的聊天记录仍可查看，云端记录不受影响。重新安装微信后点「重新检查」即可恢复。")}</p></div>
        <button type="button" className="connector-quiet" onClick={recheckAll} disabled={install.checking}>{t("重新检查")}</button>
      </div>}
      <div className="connector-grid">
        <article className={`connector-card${enabled ? '' : ' off'}${missing ? ' blocked' : ''}`}>
          <div className="connector-card-head">
            <ConnectorTile name="微信" />
            <div>
              <h3>{t("微信")}</h3>
              <p className="connector-state">
                <span className={missing ? 'bad' : enabled && probe?.verifiedAccounts && !wechatIncomplete ? 'good' : ''} />
                {install.checking ? t("正在检测本机微信…") : missing ? detectTitle[install.detect!.state] : signatureWarning ? t("已检测到微信 · 签名待确认") : enabled ? status : t("已断开 · 停止读取本机消息")}
              </p>
            </div>
          </div>
          {missing && !(enabled && hasHistory) ? <div className="connector-install-check">
            <p>{t(install.detect!.message)}</p>
            {install.detect!.app_path && <p className="connector-install-path">{t("检测位置：")}{install.detect!.app_path}</p>}
            <button type="button" className="connector-quiet" onClick={recheckAll} disabled={loading || install.checking}>{t("重新检查")}</button>
          </div> : signatureWarning ? <div className="connector-install-check"><p>{t('检测到微信，但签名尚未确认。可继续查看已有记录或导入密钥文件；自动连接仍需确认官方版本。')}</p><button type="button" className="connector-quiet" onClick={recheckAll} disabled={loading || install.checking}>{t('重新检查')}</button></div> : <p className="connector-description">
            {enabled
              ? t("读取本机数据库，只读，不改动微信")
              : t("本机密钥与解密快照仍然保留，重新连接不需要再次导入；云端已上传的记录保持不变。")}
          </p>}
          {!missing && !signatureWarning && install.detect?.version && <details className="connector-install-details"><summary>{t("微信")}{install.detect.version} {t("· 官方签名")}</summary><p className="connector-install-meta">{install.detect.app_path}</p><button type="button" className="connector-quiet" onClick={recheckAll} disabled={loading || install.checking}>{t("重新检查")}</button></details>}
          <p className="connector-capabilities">{t("历史记录 · 群聊 · 增量同步")}</p>
          {enabled && wechatIncomplete && <p className="connector-setup-state" role="status">{t("配置未完成，备份已暂停。")}</p>}
          <div className="connector-actions">
            {enabled ? <>
              <button type="button" className={`connector-primary${probe?.verifiedAccounts && !wechatIncomplete ? ' connector-configured' : ''}`} onClick={() => onOpenWechat(Boolean(probe?.verifiedAccounts), wechatIncompleteSource?.account_id)} disabled={!install.canProceed} title={missing ? t("请先安装微信") : undefined}>{wechatIncomplete ? t("继续配置") : probe?.verifiedAccounts ? t("重新配置") : t("打开连接向导")}</button>
              {missing && install.detect!.state === 'not_installed' && <a className="connector-link" href="https://pc.weixin.qq.com/" target="_blank" rel="noreferrer">{t("前往下载微信 ↗")}</a>}
              {isTauri() && (!missing || hasHistory) && <button type="button" className="connector-quiet danger" onClick={() => setConfirming(true)} disabled={saving || !settingsLoaded}>{t("断开")}</button>}
            </> : <>
              <button type="button" className="connector-primary" onClick={() => void setConnected(true)} disabled={saving || !settingsLoaded}>{saving ? t("连接中…") : t("重新连接")}</button>
            </>}
          </div>
          {enabled && confirming && <div className="connector-confirm" role="dialog" aria-label={t("确认断开微信连接")}>
            <p>{t("断开后本机不再读取微信消息，也不会自动备份。")}</p>
            <ul>
              <li>{t("保留本机密钥与解密快照，重新连接无需再次导入")}</li>
              <li>{t("停止聊天读取、搜索、整理与自动备份")}</li>
              <li>{t("云端已上传的记录不会删除")}</li>
            </ul>
            <div className="connector-actions">
              <button type="button" className="connector-danger" onClick={() => void setConnected(false)} disabled={saving || !settingsLoaded}>{saving ? t("断开中…") : t("确认断开")}</button>
              <button type="button" className="connector-quiet" onClick={() => setConfirming(false)} disabled={saving || !settingsLoaded}>{t("取消")}</button>
            </div>
          </div>}
        </article>
        <article className={`connector-card${whatsappEnabled ? '' : ' off'}${whatsappMissing ? ' blocked' : ''}`}>
          <div className="connector-card-head">
            <ConnectorTile name="WhatsApp" />
            <div>
              <h3>WhatsApp</h3>
              <p className="connector-state">
                <span className={whatsappMissing ? 'bad' : whatsappEnabled && whatsapp?.sessions && !whatsappIncomplete ? 'good' : ''} />
                {whatsappInstall.checking || whatsappBusy ? t("正在检测本机 WhatsApp…")
                  : whatsappMissing ? detectTitleFor('WhatsApp', whatsappInstall.detect!.state)
                    : !whatsappEnabled ? t("已断开 · 停止读取本机消息")
                      : whatsapp?.error ? t("读取失败")
                        : whatsapp?.sessions ? tf("{0} · {1} 个会话 · {2} 条消息", whatsapp.name ?? '已登录', whatsapp.sessions, whatsapp.messages) : t("已连接 · 还没有会话")}
              </p>
            </div>
          </div>
          {whatsappMissing ? <div className="connector-install-check">
            <p>{t(whatsappInstall.detect!.message)}</p>
            {whatsappInstall.detect!.app_path && <p className="connector-install-path">{t("检测位置：")}{whatsappInstall.detect!.app_path}</p>}
            <button type="button" className="connector-quiet" onClick={() => { void whatsappInstall.recheck(); void refreshWhatsapp(); }} disabled={whatsappBusy || whatsappInstall.checking}>{t("重新检查")}</button>
          </div> : <p className="connector-description">
            {whatsappEnabled
              ? t("读取桌面版本地数据，只读，无需解密")
              : t("已断开读取；本机聊天数据不会被修改，重新连接即可继续读取。")}
          </p>}
          {!whatsappMissing && whatsappInstall.detect?.version && <details className="connector-install-details"><summary>WhatsApp {whatsappInstall.detect.version} {t("· 官方签名")}</summary><p className="connector-install-meta">{whatsappInstall.detect.app_path}</p><button type="button" className="connector-quiet" onClick={() => { void whatsappInstall.recheck(); void refreshWhatsapp(); }} disabled={whatsappBusy || whatsappInstall.checking}>{t("重新检查")}</button></details>}
          {whatsapp?.error && <p className="connector-error" role="alert">{whatsapp.error}</p>}
          <p className="connector-capabilities">{t("历史记录 · 群聊 · 增量同步")}</p>
          {whatsappEnabled && whatsappIncomplete && <p className="connector-setup-state" role="status">{t("配置未完成，备份已暂停。")}</p>}
          <div className="connector-actions">
            {whatsappEnabled ? <>
              <button type="button" className={`connector-primary${whatsapp?.account && !whatsapp.error && !whatsappIncomplete ? ' connector-configured' : ''}`} onClick={() => onOpenWhatsApp(whatsappIncomplete ? whatsapp?.account ?? undefined : undefined)}>{whatsappIncomplete ? t("继续配置") : whatsapp?.account && !whatsapp.error ? t("重新配置") : t("打开连接向导")}</button>
              {whatsappMissing && whatsappInstall.detect!.state === 'not_installed' && <a className="connector-link" href="https://www.whatsapp.com/download" target="_blank" rel="noreferrer">{t("前往下载 WhatsApp ↗")}</a>}
              {isTauri() && !whatsappMissing && <button type="button" className="connector-quiet danger" onClick={() => setWhatsappConnected(false)} disabled={saving || !settingsLoaded}>{t("断开")}</button>}
            </> : <>
              <button type="button" className="connector-primary" onClick={() => onOpenWhatsApp()}>{t("打开连接向导")}</button>
            </>}
          </div>
        </article>
      </div>
      {error && <p className="connector-error" role="alert">{t(error)}</p>}
      {saveError && <p className="connector-error" role="alert">{saveError}</p>}
    </section>
    <section aria-labelledby="connector-upcoming">
      <div className="connector-section-head"><h2 id="connector-upcoming">{t("即将支持")}</h2><p className="connector-section-description">{t("关注你期待的来源")}</p></div>
      <div className="connector-grid">{futureConnectors.map(futureCard)}
        <article className="connector-card connector-developer">
          <div className="connector-card-head"><span className="connector-tile" aria-hidden="true">＋</span><div><h3>{t("自己写一个")}</h3><p className="connector-state">{t("即将开放")}</p></div></div>
          <p className="connector-description">{t("连接器扩展接口正在规划中，未来可通过扩展包接入更多来源。")}</p>
          <div className="connector-actions">{interestButton(t("自定义连接器"))}</div>
        </article>
      </div>
    </section>
  </div>;
}
