import { t, tf } from '../../../shared/i18n/locale';
import type { WeChatConnection } from './useWeChatConnection';
import type { WeChatAccountSelection } from './connectionTypes';
import { formatAge } from '../../../shared/content/localDataFormat';

export function WeChatKeyImport({ connection, selection }: {
  connection: WeChatConnection; selection: WeChatAccountSelection;
}) {
  const { account } = connection;
  const { accountId, accountSwitching, busy, doScan, switchAccount } = selection;
  const { keyedCount } = connection.keys;
  const { keyImport, savedKeySource, freshSource, setShowKeyGuide, fixKeyPermissions, fixingPermissions, importSavedKey, refreshSavedKeySource } = connection.keys;
  const { captureOutcome } = connection.capture;
  const { clearKeys, clearConfirm, setClearConfirm } = connection.decryption;

  const importedForOther = keyImport && account
    ? keyImport.accounts.find(entry => !entry.selected && entry.matched + entry.already > 0)
    : undefined;
  const importedForCurrent = keyImport && account
    ? keyImport.accounts.find(entry => entry.selected && entry.matched + entry.already > 0)
    : undefined;

  return (
    <div id="wizard-detail-key-import" className="wxv-keyguide-step">
      <strong>{t("4 · 导入并校验")}</strong>
      <p className="wxv-muted">
        {captureOutcome?.matched
          ? t("本次捕获的口令已通过消息库与会话库校验，并已逐库导入保存。")
          : savedKeySource?.exists
            ? t("已检测到口令文件：点「导入已提取的口令」逐库用第 1 页 HMAC 校验。口令不会显示在界面、不会写日志、不会上传。")
            : t("捕获成功后口令会写入本机文件并自动导入；也可以手动粘贴或选择文件导入。")}
      </p>
      <div className="wxv-keyguide-actions">
        <button
          type="button"
          className="wxv-btn"
          onClick={() => void importSavedKey()}
          disabled={!savedKeySource?.exists || busy === 'import'}
        >
          {busy === 'import' ? t("校验中…") : t("导入已提取的口令")}
        </button>
        <button
          type="button"
          className="wxv-btn"
          onClick={() => void refreshSavedKeySource()}
        >
          {t("重新检测文件")}
        </button>
        {savedKeySource?.exists && !savedKeySource.secure && (
          <button
            type="button"
            className="wxv-btn"
            onClick={() => void fixKeyPermissions()}
            disabled={fixingPermissions}
          >
            {fixingPermissions ? t("正在修改…") : t("修复口令文件权限")}
          </button>
        )}
        <button
          type="button"
          className="wxv-btn"
          onClick={() => {
            setShowKeyGuide(false);
            void doScan(accountId);
          }}
          disabled={busy === 'scan'}
        >
          {busy === 'scan' ? t("扫描中…") : t("重新扫描账号")}
        </button>
        {keyedCount > 0 && (
          <button
            type="button"
            className="wxv-btn danger"
            onClick={() => setClearConfirm(true)}
            disabled={busy === 'import' || clearConfirm}
          >
            {t("清除本账号密钥")}
          </button>
        )}
        {!savedKeySource?.exists && (
          <span className="wxv-keyguide-missing">{t("尚未找到口令文件，导入按钮保持禁用。")}</span>
        )}
      </div>

      {clearConfirm && (
        <div className="wxv-keyguide-danger">
          <p>
            {t("确定清除账号")} <span className="mono">{accountId}</span> {t("已保存的")} {keyedCount}{' '}
            {t("个密钥？这会同时删除该账号已解密的本地副本，之后必须重新捕获或导入口令才能查看。\n                          微信原始数据库不受影响。")}
          </p>
          <div className="wxv-row">
            <button
              type="button"
              className="wxv-btn danger"
              onClick={() => void clearKeys()}
              disabled={busy === 'import'}
            >
              {busy === 'import' ? t("正在清除…") : t("确认清除密钥与解密副本")}
            </button>
            <button
              type="button"
              className="wxv-btn"
              onClick={() => setClearConfirm(false)}
              disabled={busy === 'import'}
            >
              {t("取消")}
            </button>
          </div>
        </div>
      )}

      {freshSource && (
        <p className="wxv-keyguide-fresh-note">
          {t("检测到新写入的口令，导入会替换当前账号里已失效的密钥。")}
        </p>
      )}

      {importedForOther && !importedForCurrent && (
        <p className="wxv-keyguide-partial">
          {t("该口令解锁的是账号")} <strong>{importedForOther.account}</strong>（{importedForOther.matched} {t("个新密钥），\n                        当前选择的是")} {account?.id}。
          <button
            type="button"
            className="wxv-link"
            disabled={accountSwitching}
            onClick={() => void switchAccount(importedForOther.account)}
          >
            {t("切换到")} {importedForOther.account}
          </button>
        </p>
      )}

      <p className="wxv-keyguide-path">
        {t("口令文件：")}<span className="mono">{savedKeySource?.path ?? '~/.wcdb-key-tool/wechat-passphrase.json'}</span>
        {savedKeySource?.exists
          ? tf("（{0}，权限 {1}）", formatAge(savedKeySource.age_seconds), savedKeySource.mode ?? t("未知"))
          : t("（尚未生成）")}
      </p>
      {savedKeySource?.exists && !savedKeySource.secure && (
        <p className="wxv-keyguide-warn">
          {t("口令文件权限是")} {savedKeySource.mode ?? t("未知")}{t("，其他本机用户可能读取它。点「修复口令文件权限」改成 600。")}
        </p>
      )}
      <p className="wxv-keyguide-path">
        {t("需要新增微信号时：在微信登录新账号 → 点「重新扫描账号」→ 从第 1 步重新捕获一次。\n                      每个账号单独捕获、单独校验，口令不通用。")}
      </p>
    </div>
  );
}
