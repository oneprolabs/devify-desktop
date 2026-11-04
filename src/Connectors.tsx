import { useCallback, useEffect, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { type AppSettings, mergeSettings } from './Settings';

type Scan = { accounts: Array<{ id: string }> };
type KeyStatus = { keys: Array<{ account: string }> };
type Probe = { accounts: number; verifiedAccounts: number };

type Connector = { name: string; mark: string; tone: string; description: string; capabilities: string };
const futureConnectors: Connector[] = [
  { name: 'WhatsApp', mark: 'W', tone: 'teal', description: '读取桌面版本地数据', capabilities: '历史记录 · 群聊 · 增量同步' },
  { name: '邮件', mark: '@', tone: 'blue', description: '连接邮箱，整理邮件对话', capabilities: '历史记录 · 邮件夹' },
  { name: 'Telegram', mark: 'T', tone: 'sky', description: '读取群组与私聊', capabilities: '' },
  { name: 'Slack', mark: 'S', tone: 'purple', description: '读取工作区频道与私信', capabilities: '' },
  { name: '飞书', mark: '飞', tone: 'sky', description: '读取群聊与话题', capabilities: '' },
  { name: '钉钉', mark: '钉', tone: 'orange', description: '读取群消息', capabilities: '' },
  { name: '企业微信', mark: '企', tone: 'teal', description: '读取客户群与内部群', capabilities: '' },
  { name: 'Discord', mark: 'D', tone: 'blue', description: '读取服务器频道', capabilities: '' },
  { name: 'LINE', mark: 'L', tone: 'green', description: '导入聊天文本', capabilities: '' },
];

export function Connectors({ onOpenWechat }: { onOpenWechat: (configured: boolean) => void }) {
  const [probe, setProbe] = useState<Probe | null>(null);
  const [waitlist, setWaitlist] = useState<string[]>([]);
  const [enabled, setEnabled] = useState(true);
  const [confirming, setConfirming] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(isTauri());

  useEffect(() => {
    if (!isTauri()) return;
    void invoke<AppSettings>('app_settings_get')
      .then(stored => {
        const settings = mergeSettings(stored);
        setWaitlist(settings.connectors.waitlist);
        setEnabled(settings.connectors.wechat_enabled);
      })
      .catch(() => undefined);
  }, []);

  /** Remembered locally: there is no notification service, so the label says "已加入". */
  const toggleWaitlist = async (name: string) => {
    const next = waitlist.includes(name) ? waitlist.filter(item => item !== name) : [...waitlist, name];
    const previous = waitlist;
    setWaitlist(next);
    try {
      const stored = mergeSettings(await invoke<AppSettings>('app_settings_get'));
      await invoke('app_settings_save', { value: { ...stored, connectors: { ...stored.connectors, waitlist: next } } });
      setError('');
    } catch (e) {
      setWaitlist(previous);
      setError(`保存失败：${String(e)}`);
    }
  };

  /**
   * Disconnecting flips one switch: the backend stops reading, searching, uploading and
   * backing up, while keys and the decrypted snapshot stay on disk and uploaded records on
   * the server are untouched. Reconnecting only has to flip it back.
   */
  const setConnected = async (next: boolean) => {
    if (!isTauri()) return;
    setSaving(true);
    try {
      const stored = mergeSettings(await invoke<AppSettings>('app_settings_get'));
      await invoke('app_settings_save', { value: { ...stored, connectors: { ...stored.connectors, wechat_enabled: next } } });
      setEnabled(next);
      setConfirming(false);
      setError('');
    } catch (e) {
      setError(`保存失败：${String(e)}`);
    } finally {
      setSaving(false);
    }
  };

  const refresh = useCallback(async () => {
    if (!isTauri()) return;
    setLoading(true);
    setError('');
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

  const status = !isTauri() ? '请在桌面应用中检查' : loading ? '正在检查…' : error ? '状态检查失败' : !probe?.accounts ? '未发现账号' : probe.verifiedAccounts ? `已校验 ${probe.verifiedAccounts} 个账号的密钥` : `发现 ${probe.accounts} 个账号，待校验密钥`;

  return <div className="connector-page">
    <header className="connector-hero">
      <div><p>CONNECTORS</p><h1>你所有的对话，汇到一处</h1><span>连接聊天来源，选择要导入的会话，再按需要整理与上传。</span></div>
      <div className="connector-hero-marks" aria-hidden="true"><i className="green">微</i><i className="teal">W</i><i className="blue">@</i><i className="sky">T</i><i className="purple">S</i><i className="sky">飞</i></div>
    </header>
    <section aria-labelledby="connector-available">
      <h2 id="connector-available">已开放</h2>
      <div className="connector-grid">
        <article className={`connector-card${enabled ? '' : ' off'}`}>
          <div className="connector-card-head">
            <span className="connector-mark green">微</span>
            <div>
              <h3>微信</h3>
              <p className="connector-state">
                <span className={enabled && probe?.verifiedAccounts ? 'good' : ''} />
                {enabled ? status : '已断开 · 停止读取本机消息'}
              </p>
            </div>
          </div>
          <p className="connector-description">
            {enabled
              ? '读取本机数据库，只读，不改动聊天数据'
              : '本机密钥与解密快照仍然保留，重新连接不需要再次导入；云端已上传的记录保持不变。'}
          </p>
          <p className="connector-capabilities">历史记录 · 群聊 · 增量同步</p>
          <div className="connector-actions">
            {enabled ? <>
              <button type="button" className="connector-primary" onClick={() => onOpenWechat(Boolean(probe?.verifiedAccounts))}>{probe?.verifiedAccounts ? '重新配置' : '打开连接向导'}</button>
              {isTauri() && <button type="button" className="connector-quiet" onClick={() => void refresh()} disabled={loading}>重新检查</button>}
              {isTauri() && <button type="button" className="connector-quiet danger" onClick={() => setConfirming(true)} disabled={saving}>断开连接</button>}
            </> : <>
              <button type="button" className="connector-primary" onClick={() => void setConnected(true)} disabled={saving}>{saving ? '连接中…' : '重新连接'}</button>
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
              <button type="button" className="connector-danger" onClick={() => void setConnected(false)} disabled={saving}>{saving ? '断开中…' : '确认断开'}</button>
              <button type="button" className="connector-quiet" onClick={() => setConfirming(false)} disabled={saving}>取消</button>
            </div>
          </div>}
        </article>
      </div>
      {error && <p className="connector-error" role="alert">{error}</p>}
    </section>
    <section aria-labelledby="connector-upcoming">
      <div className="connector-section-head"><h2 id="connector-upcoming">其他来源</h2><span>尚未接入</span></div>
      <div className="connector-grid">{futureConnectors.map(connector => <article key={connector.name} className="connector-card future">
        <div className="connector-card-head"><span className={`connector-mark ${connector.tone}`}>{connector.mark}</span><div><h3>{connector.name}</h3><p className="connector-state">尚未接入</p></div></div>
        <p className="connector-description">{connector.description}</p>
        {connector.capabilities && <p className="connector-capabilities">{connector.capabilities}</p>}
        <div className="connector-actions"><button
          type="button"
          className={`connector-notify${waitlist.includes(connector.name) ? ' on' : ''}`}
          aria-pressed={waitlist.includes(connector.name)}
          onClick={() => void toggleWaitlist(connector.name)}
        >{waitlist.includes(connector.name) ? '已加入，上线通知你' : '通知我'}</button></div>
      </article>)}</div>
    </section>
  </div>;
}
