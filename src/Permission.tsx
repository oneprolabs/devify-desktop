import { useCallback, useEffect, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';

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

const POLL_MS = 2500;

/**
 * macOS has no consent alert for Full Disk Access and no API to request it: the switch only
 * moves when the user flips it in System Settings, and the grant is bound to the app's location
 * and code signature. There is nothing to trigger from here, so this is written as plain
 * instructions — with two shortcuts that only save the user the hunting: jump to the pane, and
 * reveal the app so it can be added.
 *
 * Renders nothing once the probe can actually read WeChat's container.
 */
export function DiskAccessCard({ onRescan }: { onRescan?: () => void }) {
  const [access, setAccess] = useState<DiskAccess | null>(null);
  const [error, setError] = useState('');
  const [copied, setCopied] = useState(false);

  const refresh = useCallback(async () => {
    if (!isTauri()) {
      setError('');
      return;
    }
    try {
      setAccess(await invoke<DiskAccess>('perm_status'));
      setError('');
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // The switch is flipped in another app, so the only way to notice is to look again.
  useEffect(() => {
    if (access?.granted) return;
    const timer = window.setInterval(() => void refresh(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [access?.granted, refresh]);

  const copyPath = async () => {
    try {
      await navigator.clipboard.writeText(access?.app_path ?? '');
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    } catch (e) {
      setError(`复制失败：${String(e)}`);
    }
  };

  if (access?.granted) return null;

  const fromImage = access?.location === 'volumes' || access?.location === 'translocated';
  const misplaced = Boolean(access && access.bundled && !fromImage && !access.installed);

  return (
    <div className="wxv-keyguide attention perm-card">
      <div className="wxv-keyguide-head">
        <div>
          <strong>需要「完全磁盘访问权限」</strong>
          <p className="wxv-muted">
            微信的数据库在它自己的沙盒容器里，macOS 只允许拿到这项授权的程序读取。
            这项权限没有申请接口，也不会弹窗，只能手动打开一次。
          </p>
        </div>
        <div className="perm-badge">{access?.state === 'denied' ? '未授权' : '待确认'}</div>
      </div>

      {fromImage && (
        <p className="wxv-keyguide-warn">
          你现在直接从磁盘映像（DMG）里运行，授权不会保留。先把 Wechatify 拖到「应用程序」文件夹，
          再从那里启动，然后按下面的步骤授权。
        </p>
      )}

      {misplaced && (
        <p className="wxv-keyguide-warn">
          当前这份不在「应用程序」里（{access?.app_path}）。这项权限绑定位置，请在「应用程序」里
          放一份并从那启动，再按下面的步骤授权。
        </p>
      )}

      <ol className="wxv-keyguide-steps">
        <li>
          打开 <strong>系统设置 → 隐私与安全性 → 完全磁盘访问权限</strong>。
        </li>
        <li>
          点列表下方的 <strong>+</strong>，在「应用程序」里选中 <strong>Wechatify</strong>；如果已经能看见
          Wechatify 的条目，直接跳到下一步（重复条目先按 − 删掉）。首次添加需要输入密码或触控 ID。
        </li>
        <li>确认 Wechatify 右侧的开关是打开的。</li>
        <li>
          回到 Wechatify，<strong>按 ⌘Q 完全退出再重新打开</strong>。权限只在应用启动时生效，
          只关窗口或切回前台不会刷新。
        </li>
        <li>重新打开后回到这一步，点「重新检测」——能读到微信目录时这张提示会自动消失。</li>
      </ol>

      <div className="perm-path">
        <span>要添加的程序</span>
        <code>{access?.app_path ?? '—'}</code>
      </div>

      <div className="perm-actions">
        <button type="button" className="wxv-btn primary" onClick={() => void invoke('perm_open_settings').catch(e => setError(String(e)))}>
          打开系统设置
        </button>
        <button type="button" className="wxv-btn" onClick={() => void invoke('perm_reveal_app').catch(e => setError(String(e)))}>
          在 Finder 中显示
        </button>
        <button
          type="button"
          className="wxv-btn"
          onClick={() => {
            void refresh();
            onRescan?.();
          }}
        >
          重新检测
        </button>
        <button type="button" className="wxv-btn" onClick={() => void copyPath()}>
          {copied ? '已复制' : '复制程序路径'}
        </button>
        <span className="perm-poll">每 {POLL_MS / 1000} 秒自动检测</span>
      </div>

      <details className="perm-more">
        <summary>按上面做完还是读不到？</summary>
        <ul className="wxv-keyguide-steps">
          <li>
            确认加进列表的是上面的 <code>Wechatify.app</code> 本身——不是里面的可执行文件，也不是 DMG 里的那份。
          </li>
          <li>确认开关是开的，并且确实按 ⌘Q 完全退出过（不是点红叉关窗口）。</li>
          <li>
            重新构建或移动过应用，macOS 会把它当成新的程序：旧条目删掉，重新添加一次。
          </li>
          <li>
            不想给整块磁盘权限：在「终端」里运行
            <code>Wechatify.app/Contents/MacOS/wechat-devify-bridge</code>，它会继承终端已有的权限；
            代价是关掉终端应用就退出。
          </li>
        </ul>
      </details>

      {access?.detail && <p className="perm-detail">判定依据：{access.detail}</p>}
      {error && isTauri() && <p className="wxv-keyguide-warn">{error}</p>}
    </div>
  );
}
