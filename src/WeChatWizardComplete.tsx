import { t, tf } from './locale';
import { useEffect, useRef, useState } from 'react';
import { desktopInvoke as invoke } from './desktopInvoke';
import { mergeSettings, type AppSettings } from './Settings';
import { WizardActionButton } from './WizardActionButton';

type Capture = { pending: boolean; needs_cleanup: boolean; recovery_error_code: string | null; wechat_official: boolean };
export function WeChatWizardComplete({ edit, onFinish, onBack }: { edit: boolean; onFinish: () => void; onBack: () => void }) {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [error, setError] = useState('');
  const [openingError, setOpeningError] = useState('');
  const [platform, setPlatform] = useState('');
  const [openRequested, setOpenRequested] = useState(false);
  const mounted = useRef(true);
  const load = async () => {
    try {
      const [stored, info] = await Promise.all([invoke<AppSettings>('app_settings_get'), invoke<{ platform: string }>('app_info')]);
      if (mounted.current) { setSettings(mergeSettings(stored)); setPlatform(info.platform); setError(''); }
    } catch { if (mounted.current) setError('未能读取已保存的设置，请重新检查。'); }
  };
  useEffect(() => { mounted.current = true; void load(); return () => { mounted.current = false; }; }, []);
  const openOfficial = async () => {
    setOpeningError('');
    try {
      const capture = await invoke<Capture>('wechat_capture_status');
      if (!mounted.current) return;
      if (capture.pending || capture.needs_cleanup || capture.recovery_error_code || !capture.wechat_official) {
        setOpeningError('尚未确认官方微信已恢复，请返回连接步骤处理恢复后再打开。'); return;
      }
      const result = await invoke<{ launched: boolean }>('wechat_capture_open');
      if (!mounted.current) return;
      if (!result.launched) { setOpeningError('微信未能打开，请重试；仍失败时，请从「应用程序」打开微信。'); return; }
      setOpenRequested(true);
    } catch { if (mounted.current) setOpeningError('未能打开官方微信，请重试。'); }
  };
  const count = settings?.chat.allowed_usernames[settings.active_account_id]?.length ?? 0;
  const plan = settings?.backup;
  const frequency = !settings?.sync.enabled ? '自动备份已关闭，可手动备份'
    : plan?.mode === 'manual' ? '仅手动备份' : plan?.mode === 'times' ? `每天 ${plan.times.join('、')}`
      : `每 ${settings.chat.sync_interval_seconds % 3600 === 0 ? `${settings.chat.sync_interval_seconds / 3600} 小时` : `${Math.round(settings.chat.sync_interval_seconds / 60)} 分钟`}`;
  return <section className="wizard-complete wizard-finish-v7">
    <header><h2>{edit ? t("配置已更新") : t("微信已配置完成")}</h2><p>{edit ? t("设置已保存并生效。") : t("之后可以随时回来重新配置。")}</p></header>
    {settings && <ul className="wizard-finish-facts"><li>{t("已选择")} {count} {t("个会话导入")}</li><li>{t("备份周期：")}{frequency}</li><li>{t("智能分析是统一设置，可在「智能分析」中调整")}</li></ul>}
    {!settings && !error && <p role="status">{t("正在确认已保存的设置…")}</p>}
    {error && <div role="alert"><p>{error}</p><WizardActionButton className="wxv-btn" onAction={load}>{t("重新检查")}</WizardActionButton></div>}
    {platform === 'macos' && <div className="wizard-open-wechat"><div><strong>{t("继续使用微信")}</strong><p>{openRequested ? t("已请求打开官方微信，请在微信中确认登录。") : t("连接设置完成后，重新打开官方微信即可继续使用。")}</p></div><WizardActionButton className="wxv-btn" busyLabel={t("正在检查并打开微信…")} onAction={openOfficial}>{openRequested || openingError ? t("重新打开微信") : t("打开微信")}</WizardActionButton></div>}
    {openingError && <div role="alert"><p>{openingError}</p><button type="button" className="wizard-history-toggle" onClick={onBack}>{t("返回连接步骤")}</button></div>}
    <footer className="wizard-finish-actions"><button type="button" className="wizard-next" disabled={!settings || Boolean(error)} onClick={onFinish}>{edit ? t("保存") : t("完成")}</button></footer>
  </section>;
}
