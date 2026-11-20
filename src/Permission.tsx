import { t, tf, ctf } from './locale';
import { useCallback, useEffect, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { WizardActionButton } from './WizardActionButton';

type PlatformCapabilities = { platform: string; key_acquisition: string; supports_memory_scan: boolean; supports_macos_capture: boolean; supports_key_import: boolean };
type DiskAccess = {
  granted: boolean;
  state: 'granted' | 'denied' | 'unknown' | string;
  app_path: string | null;
  bundled: boolean;
  translocated: boolean;
  location: 'applications' | 'volumes' | 'translocated' | 'build' | 'other' | string;
  installed: boolean;
  wechat_dir: string | null;
  detail: string;
};

/**
 * macOS has no consent alert for Full Disk Access and no API to request it: the switch only
 * moves when the user flips it in System Settings, and the grant is bound to the app's location
 * and code signature. There is nothing to trigger from here, so this is written as plain
 * instructions — with two shortcuts that only save the user the hunting: jump to the pane, and
 * reveal the app so it can be added.
 *
 * Renders nothing once the probe can actually read WeChat's container.
 */
export type DiskAccessCheck = { granted: boolean | null; platform: string | null; error: string };

export function DiskAccessCard({ onRescan, compact = false, checkOnEntry = true, onCheck }: {
  onRescan?: () => void;
  compact?: boolean;
  checkOnEntry?: boolean;
  onCheck?: (check: DiskAccessCheck) => void;
}) {
  const [access, setAccess] = useState<DiskAccess | null>(null);
  const [platform, setPlatform] = useState<PlatformCapabilities | null>(null);
  const [error, setError] = useState('');
  const [copied, setCopied] = useState(false);

  const refresh = useCallback(async () => {
    if (!isTauri()) {
      setError('');
      return;
    }
    try {
      setPlatform(await invoke<PlatformCapabilities>('wechat_platform_capabilities'));
      setAccess(await invoke<DiskAccess>('perm_status'));
      setError('');
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    if (checkOnEntry) void refresh();
  }, [checkOnEntry, refresh]);

  useEffect(() => {
    onCheck?.({
      granted: error ? null : platform?.platform === 'windows' ? true : access?.granted ?? null,
      platform: platform?.platform ?? null,
      error,
    });
  }, [access, platform, error, onCheck]);

  const copyPath = async () => {
    try {
      await navigator.clipboard.writeText(access?.app_path ?? '');
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    } catch (e) {
      setError(tf("复制失败：{0}", String(e)));
    }
  };

  if (access?.granted) return null;
  if (platform?.platform === 'windows') return <div className="wxv-keyguide attention perm-card">
    <div className="wxv-keyguide-head"><div><strong>{t("Windows 权限检查")}</strong><p className="wxv-muted">{t("Windows 不需要 macOS 的「完全磁盘访问权限」。请保持微信已登录，并确保 AIMyChats 与微信使用同一 Windows 用户；如果读取进程内存失败，请用管理员身份重新启动本应用。")}</p></div><div className="perm-badge">Windows</div></div>
    <div className="perm-actions"><button type="button" className="wxv-btn primary" onClick={() => { void refresh(); onRescan?.(); }}>{t("重新检测")}</button><span className="perm-poll">{t("密钥方式：")}{platform.key_acquisition}</span></div>
    {error && <p className="wxv-keyguide-warn">{t(error)}</p>}
  </div>;
  if (platform && platform.platform !== 'macos') return <div className="wxv-keyguide attention perm-card"><div className="wxv-keyguide-head"><div><strong>{t("当前系统使用导入模式")}</strong><p className="wxv-muted">{t("当前平台不提供应用内密钥捕获，请导入可信工具导出的密钥 JSON，再由本地数据库第 1 页 HMAC 校验。")}</p></div><div className="perm-badge">{platform.platform}</div></div></div>;

  const fromImage = access?.location === 'volumes' || access?.location === 'translocated';
  const misplaced = Boolean(access && access.bundled && !fromImage && !access.installed);

  if (compact) return <div className="wx-setup-permission">
    {!access && !error ? <p role="status">{t("正在检查读取权限…")}</p> : <>
      <p>{fromImage || misplaced ? t("请先将 AIMyChats 移到「应用程序」，从那里重新打开。") : t("请在「完全磁盘访问权限」中打开 AIMyChats 的开关。")}</p>
      <p><strong>{t("授权后，请在系统提示中选择「退出并重新打开」。")}</strong>{t("若没有弹窗，请按 ⌘Q 退出 AIMyChats，再重新打开后继续。")}</p>
      <div className="wx-setup-actions">
        <WizardActionButton className="wxv-btn primary" busyLabel={t("正在打开系统设置…")} onAction={() => invoke('perm_open_settings').catch(e => setError(String(e)))}>{t("打开系统设置")}</WizardActionButton>
        <WizardActionButton className="wxv-btn" busyLabel={t("正在检测权限…")} onAction={async () => { await refresh(); await onRescan?.(); }}>{t("重新检测")}</WizardActionButton>
      </div>
      <details><summary>{t("找不到 AIMyChats？")}</summary><p>{t("在权限列表中点「+」，添加「应用程序」中的 AIMyChats，再打开开关。")}</p><button type="button" className="wxv-btn" onClick={() => void invoke('perm_reveal_app').catch(e => setError(String(e)))}>{t("在 Finder 中显示")}</button></details>
      {error && <p role="alert">{t("未能完成权限检查，请点「重新检测」。")}</p>}
    </>}
  </div>;

  return (
    <div className="wxv-keyguide attention perm-card">
      <div className="wxv-keyguide-head">
        <div>
          <strong>{t("需要「完全磁盘访问权限」")}</strong>
          <p className="wxv-muted">
            {t("微信的数据库在它自己的沙盒容器里，macOS 只允许拿到这项授权的程序读取。\n            这项权限没有申请接口，也不会弹窗，只能手动打开一次。")}
          </p>
        </div>
        <div className="perm-badge">{t("未完成")}</div>
      </div>

      {fromImage && (
        <p className="wxv-keyguide-warn">
          {t("你现在直接从磁盘映像（DMG）里运行，授权不会保留。先把 AIMyChats 拖到「应用程序」文件夹，\n          再从那里启动，然后按下面的步骤授权。")}
        </p>
      )}

      {misplaced && (
        <p className="wxv-keyguide-warn">
          {t("当前这份不在「应用程序」里（")}{access?.app_path}{t("）。这项权限绑定位置，请在「应用程序」里\n          放一份并从那启动，再按下面的步骤授权。")}
        </p>
      )}

      <ol className="wxv-keyguide-steps">
        <li>
          {t("打开")} <strong>{t("系统设置 → 隐私与安全性 → 完全磁盘访问权限")}</strong>。
        </li>
        <li>
          {t("点列表下方的")} <strong>+</strong>{t("，在「应用程序」里选中")} <strong>AIMyChats</strong>{t("；如果已经能看见\n          AIMyChats 的条目，直接跳到下一步（重复条目先按 − 删掉）。首次添加需要输入密码或触控 ID。")}
        </li>
        <li>{t("确认 AIMyChats 右侧的开关是打开的。")}</li>
        <li>
          {t("授权后，请在系统提示中选择")}<strong>{t("「退出并重新打开」")}</strong>{t("。若没有弹窗， 请按 ⌘Q 完全退出 AIMyChats，再重新打开后继续。")}</li>
        <li>{t("重新打开后会自动检测权限；也可以回到这一步点「重新检测」。")}</li>
      </ol>

      <div className="perm-path">
        <span>{t("要添加的程序")}</span>
        <code>{access?.app_path ?? '—'}</code>
      </div>

      <div className="perm-actions">
        <button type="button" className="wxv-btn primary" onClick={() => void invoke('perm_open_settings').catch(e => setError(String(e)))}>
          {t("打开系统设置")}
        </button>
        <button type="button" className="wxv-btn" onClick={() => void invoke('perm_reveal_app').catch(e => setError(String(e)))}>
          {t("在 Finder 中显示")}
        </button>
        <button
          type="button"
          className="wxv-btn"
          onClick={() => {
            void refresh();
            onRescan?.();
          }}
        >
          {t("重新检测")}
        </button>
        <button type="button" className="wxv-btn" onClick={() => void copyPath()}>
          {copied ? t("已复制") : t("复制程序路径")}
        </button>
      </div>

      <details className="perm-more">
        <summary>{t("按上面做完还是读不到？")}</summary>
        <ul className="wxv-keyguide-steps">
          <li>
            {t("确认加进列表的是上面的")} <code>AIMyChats.app</code> {t("本身——不是里面的可执行文件，也不是 DMG 里的那份。")}
          </li>
          <li>{t("确认开关是开的，并且确实按 ⌘Q 完全退出过（不是点红叉关窗口）。")}</li>
          <li>
            {t("重新构建或移动过应用，macOS 会把它当成新的程序：旧条目删掉，重新添加一次。")}
          </li>
          <li>
            {t("不想给整块磁盘权限：在「终端」里运行")}
            <code>AIMyChats.app/Contents/MacOS/wechat-devify-bridge</code>{t("，它会继承终端已有的权限；\n            代价是关掉终端应用就退出。")}
          </li>
        </ul>
      </details>

      {access?.detail && <p className="perm-detail">{t("判定依据：")}{t(access.detail)}</p>}
      {error && isTauri() && <p className="wxv-keyguide-warn">{t(error)}</p>}
    </div>
  );
}
