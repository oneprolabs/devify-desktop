import { t, tf } from '../../../shared/i18n/locale';
import { Icons } from '../../../shared/ui/icons';
import type { WeChatConnection } from './useWeChatConnection';
import type { WeChatAccountSelection } from './connectionTypes';
import { DiskAccessCard } from '../Permission';
import { formatSize } from '../../../shared/content/localDataFormat';

export function WeChatAccountStep({ connection, selection }: {
  connection: WeChatConnection; selection: WeChatAccountSelection;
}) {
  const { account } = connection;
  const { accountId, scan, accountSwitching, busy, doScan, switchAccount } = selection;
  return (
    <li id="wx-connect-account" className={account ? 'done' : ''}>
      <span className="wxv-step-no">{account ? Icons.check : '1'}</span>
      <div>
        <strong>{scan && scan.accounts.length > 1 ? t("选择账号") : t("当前账号")}</strong>
        {scan?.accounts.length ? (
          scan.accounts.length > 1 ? (
            <div className="wxv-accounts">
              {scan.accounts.map(item => (
                <label key={item.id} className={`wxv-account ${item.id === accountId ? 'on' : ''}`}>
                  <input
                    type="radio"
                    name="wx-account"
                    checked={item.id === accountId}
                    disabled={accountSwitching}
                    onChange={() => void switchAccount(item.id)}
                  />
                  <span>
                    <strong>{item.id}</strong>
                    <small>
                      {item.db_count} {t("个库 ·")} {formatSize(item.total_bytes)} {t("· 加密")} {item.encrypted_count}
                    </small>
                  </span>
                </label>
              ))}
            </div>
          ) : (
            <p className="wxv-muted">
              {accountId ? <>{t("已自动使用本机微信账号")} <span className="mono">{accountId}</span>。</> : t("尚未确认本机微信账号。")}
              {account
                ? tf(" {0} 个库 · {1} · 加密 {2}", account.db_count, formatSize(account.total_bytes), account.encrypted_count)
                : ''}
            </p>
          )
        ) : (
          <>
            <p className="wxv-muted">
              {busy === 'scan' ? t("正在定位微信目录…") : t("没有发现账号目录，请确认微信已登录过。")}
            </p>
            {busy !== 'scan' && <DiskAccessCard onRescan={() => void doScan()} />}
          </>
        )}
      </div>
    </li>
  );
}
