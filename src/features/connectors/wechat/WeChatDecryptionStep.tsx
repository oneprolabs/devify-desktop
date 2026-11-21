import { t } from '../../../shared/i18n/locale';
import { Icons } from '../../../shared/ui/icons';
import type { WeChatConnection } from './useWeChatConnection';
import type { WeChatAccountSelection } from './connectionTypes';

export function WeChatDecryptionStep({ connection, selection, ready }: {
  connection: WeChatConnection; selection: WeChatAccountSelection;
  ready: boolean;
}) {
  const { account } = connection;
  const { accountSwitching } = selection;
  const { keyedCount } = connection.keys;
  const { outcomes, decrypting, decrypt, onlyChat, setOnlyChat, applyWal, setApplyWal, clearCache, showLog, setShowLog } = connection.decryption;
  return (
    <li id="wx-connect-decrypt" className={ready ? 'done' : ''}>
      <span className="wxv-step-no">{ready ? Icons.check : '3'}</span>
      <div>
        <strong>{t("解密并打开")}</strong>
        <p className="wxv-muted">
          {t("解密会逐页校验，任一页失败即整体失败，不会产出半成品。已解密过的库直接复用缓存。")}
        </p>
        <div className="wxv-row">
          <button
            type="button"
            className="wxv-btn primary"
            onClick={() => void decrypt()}
            disabled={!account || accountSwitching || decrypting || keyedCount === 0}
          >
            {decrypting ? t("解密中…") : t("解密当前账号")}
          </button>
          <label className="wxv-check">
            <input type="checkbox" checked={onlyChat} onChange={e => setOnlyChat(e.target.checked)} />
            {t("只处理聊天相关库")}
          </label>
          <label className="wxv-check">
            <input type="checkbox" checked={applyWal} onChange={e => setApplyWal(e.target.checked)} />
            {t("尝试回填 WAL")}
          </label>
          <button type="button" className="wxv-btn" disabled={decrypting} onClick={() => void clearCache()}>
            {t("清理解密缓存")}
          </button>
        </div>
        <p className="wxv-muted">
          {t("解密副本保存在应用数据目录；清理后会回到未解密状态，聊天记录页在重新解密前不显示内容。")}
        </p>
        {!!outcomes.length && (
          <>
            <button type="button" className="wxv-link" onClick={() => setShowLog(v => !v)}>
              {showLog ? t("收起") : t("查看")}{t("解密详情（")}{outcomes.length} {t("个库）")}
            </button>
            {showLog && (
              <table className="wxv-table">
                <thead>
                  <tr>
                    <th>{t("数据库")}</th>
                    <th>{t("状态")}</th>
                    <th>{t("页数")}</th>
                    <th>{t("说明")}</th>
                  </tr>
                </thead>
                <tbody>
                  {outcomes.map(outcome => (
                    <tr key={`${outcome.account}-${outcome.rel}`}>
                      <td className="mono">{outcome.rel}</td>
                      <td className={`wxv-status ${outcome.status}`}>{t(outcome.status)}</td>
                      <td>{outcome.pages || '—'}</td>
                      <td className="wxv-note">
                        {outcome.error ?? outcome.wal?.status ?? (outcome.status === 'cached' ? t("命中缓存") : '')}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </>
        )}
      </div>
    </li>
  );
}
