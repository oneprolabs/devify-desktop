import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { sameServiceUrl } from '../../shared/api/serviceUrl';
import { t } from '../../shared/i18n/locale';

/** Development-only local status. No network request or polling timer. */
export function DevelopmentEnvironment({ authenticated }: { authenticated: boolean }) {
  const [base, setBase] = useState('');
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    let revision = 0;
    void listen<string>('development-service-changed', event => {
      revision++;
      if (active) setBase(event.payload);
    }).then(stop => {
      if (!active) { stop(); return; }
      unlisten = stop;
      const read = revision;
      void invoke<{ devify: { base_url: string } }>('app_settings_get')
        .then(value => { if (active && read === revision) setBase(value.devify.base_url); }).catch(() => {});
    }).catch(() => {});
    return () => { active = false; unlisten?.(); };
  }, []);
  const production = sameServiceUrl(base, 'https://aimychats.com');
  return <div className={`development-environment${production ? ' production' : ''}`} role="status">
    <strong>AIMyChats Dev</strong><span>{base || t('正在读取服务地址…')}</span>
    <span className="development-environment-hint">{authenticated ? t('修改地址：设置 → 账户') : t('修改地址：登录… → 连接设置')}</span>
    {production && <strong>{t('当前连接正式服务，操作会影响正式数据。')}</strong>}
  </div>;
}
