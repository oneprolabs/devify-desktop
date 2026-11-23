import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { desktopInvoke as invoke } from '../../shared/api/desktopInvoke';
import { t, tf } from '../../shared/i18n/locale';
import { Icons } from '../../shared/ui/icons';
import './network-settings.css';

const MODES = {
  auto: {
    icon: Icons.networkAuto,
    label: '自动（默认）',
    hint: '官方服务优先直连，连接失败时可重试的请求会尝试系统代理。自托管服务使用系统代理。',
  },
  system_proxy: {
    icon: Icons.networkProxy,
    label: '使用系统代理',
    hint: '客户端遵循系统代理设置。',
  },
  direct: {
    icon: Icons.networkDirect,
    label: '直接连接',
    hint: '客户端连接服务时绕过系统代理。',
  },
} as const;

type ConnectionMode = keyof typeof MODES;

export function ConnectionSettingsDisclosure({ open, onToggle, disabled, children }: {
  open: boolean;
  onToggle: () => void;
  disabled?: boolean;
  children: ReactNode;
}) {
  const panelId = useId();
  return <div className="auth-connection">
    <button className="auth-connection-trigger" type="button" disabled={disabled}
      aria-expanded={open} aria-controls={open ? panelId : undefined} onClick={onToggle}>
      {t('连接设置')}<span aria-hidden="true">{Icons.chevronLeft}</span>
    </button>
    {open && <div id={panelId} className="auth-connection-panel">{children}</div>}
  </div>;
}

export function NetworkConnectionControl({ disabled = false, compact = false, onSavingChange }: {
  disabled?: boolean;
  compact?: boolean;
  onSavingChange?: (saving: boolean) => void;
}) {
  const [mode, setMode] = useState<ConnectionMode>();
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [loadAttempt, setLoadAttempt] = useState(0);
  const saveLock = useRef(false);

  useEffect(() => {
    let active = true;
    void invoke<ConnectionMode>('network_connection_get')
      .then(value => { if (active) setMode(value); })
      .catch(reason => { if (active) setError(tf('无法加载设置：{0}', String(reason))); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [loadAttempt]);

  async function save(next: ConnectionMode) {
    if (next === mode || saveLock.current) return;
    saveLock.current = true;
    onSavingChange?.(true);
    setSaving(true);
    setError('');
    try {
      setMode(await invoke<ConnectionMode>('network_connection_set', { mode: next }));
    } catch (reason) {
      setError(tf('无法保存网络连接设置：{0}', String(reason)));
    } finally {
      saveLock.current = false;
      setSaving(false);
      onSavingChange?.(false);
    }
  }

  return <div className={`network-connection${compact ? ' network-connection-compact' : ''}`}>
    <label>
      <span>{t('连接方式')}</span>
      <span className="network-connection-choice">
        {mode && <span className="network-connection-icon" aria-hidden="true">{MODES[mode].icon}</span>}
        <select value={mode ?? ''} disabled={disabled || loading || saving}
          onChange={event => void save(event.target.value as ConnectionMode)}>
          {!mode && <option value="" disabled>{loading ? t('正在加载设置…') : t('请选择连接方式')}</option>}
          {(Object.keys(MODES) as ConnectionMode[]).map(key =>
            <option key={key} value={key}>{t(MODES[key].label)}</option>)}
        </select>
      </span>
    </label>
    {saving && <p role="status">{t('正在保存…')}</p>}
    {mode && !compact && <p>{t(MODES[mode].hint)}</p>}
    {error && <p className="error" role="alert">{error}</p>}
    {!mode && !loading && <button type="button" className="auth-gate-secondary" onClick={() => {
      setLoading(true); setError(''); setLoadAttempt(attempt => attempt + 1);
    }}>{t('重新加载')}</button>}
  </div>;
}

export function NetworkSettings() {
  return <div className="settings-page settings-refreshed">
    <header className="page-head">
      <h2>{t('网络连接')}</h2>
      <p className="help">{t('选择 AIMyChats 客户端连接服务的方式。')}</p>
    </header>
    <section className="settings-group" aria-label={t('网络连接')}>
      <NetworkConnectionControl />
    </section>
    <p className="settings-footnote">{t('浏览器授权页使用浏览器自己的代理设置。')}</p>
  </div>;
}
