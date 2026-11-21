import { WeChatCaptureGuide } from './WeChatCaptureGuide';
import { WeChatKeyImport } from './WeChatKeyImport';
import { t, tf } from '../../../shared/i18n/locale';
import { Icons } from '../../../shared/ui/icons';
import type { WeChatConnection } from './useWeChatConnection';
import type { WeChatAccountSelection, WeChatWizard } from './connectionTypes';
import { formatAge, formatKeyDate } from '../../../shared/content/localDataFormat';
import type { PlatformCapabilities } from './nativeTypes';

export function WeChatKeyStep({ connection, selection, wizard: wizardOptions }: {
  connection: WeChatConnection; selection: WeChatAccountSelection; wizard: WeChatWizard;
}) {
  const { account } = connection;
  const { busy } = selection;
  const { keyedCount } = connection.keys;
  const { enabled: wizard, step } = wizardOptions;
  const { savedKeySource, noKeysYet, freshSource, showKeyGuide, setShowKeyGuide, scanWindowsKeys, windowsScanBusy, keyText, setKeyText, importKeys, encryptedDbs, keyVerifiedSecs, keyAgeDays, accountMissingKeys } = connection.keys;
  const { captureStatus } = connection.capture;
  const platform: PlatformCapabilities | null = null as PlatformCapabilities | null;

  return (
    <li id="wx-connect-key" className={keyedCount > 0 ? 'done' : ''}>
      <span className="wxv-step-no">{keyedCount > 0 ? Icons.check : '2'}</span>
      <div>
        <strong>{wizard && step === 'wizard-import' ? t("导入校验") : t("获取密钥")}</strong>
        <p className="wxv-muted">
          {t("支持账号口令（64 位十六进制，逐库 PBKDF2 派生）、内存密钥")} <code>x'&lt;hex&gt;'</code>{t("、或口令 JSON")}
          <code>{'{"passphrase":"…"}'}</code>{t("。导入时只保留通过第 1 页 HMAC 校验的内容。")}
        </p>

        <div className={`wxv-keyguide ${noKeysYet || freshSource ? 'attention' : ''} ${platform?.platform === 'windows' ? 'windows' : ''}`}>
          <div className="wxv-keyguide-head">
            <div>
              <strong>{platform?.platform === 'windows' ? t("使用 Windows 方式获取密钥") : t("没有密钥？在本应用里直接获取")}</strong>
              <p className="wxv-muted">
                {platform?.platform === 'windows'
                  ? t("Windows 通过读取已登录 Weixin.exe 的可读内存寻找候选密钥，并用本地数据库第 1 页 HMAC 逐库校验；找不到时可导入可信工具导出的密钥 JSON。")
                  : savedKeySource?.exists
                    ? tf("已检测到口令文件（{0}），可以导入并逐库校验。", formatAge(savedKeySource.age_seconds))
                    : captureStatus?.pending
                      ? t("正在进行密钥捕获：按下面的步骤完成即可，中途不要手动改动微信。")
                      : t("按下面四步在本机完成捕获：环境检查 → 安全准备并启动捕获 → 登录时读取口令 → 自动恢复并逐库校验。全程不需要修改系统完整性保护，也不需要另装工具。")}
              </p>
            </div>
            <div className="wxv-keyguide-head-actions">
              {freshSource && <span className="wxv-keyguide-fresh">{t("新口令")}</span>}
              <button type="button" className="wxv-btn" onClick={() => setShowKeyGuide(value => !value)}>
                {showKeyGuide ? t("收起步骤") : t("查看获取步骤")}
              </button>
            </div>
          </div>

          {platform?.platform === 'windows' && (
            <div className="wxv-windows-keyguide">
              <strong>{t("Windows 密钥扫描")}</strong>
              <p className="wxv-muted">{t("请先启动微信并登录目标账号。扫描只读取本机 Weixin.exe 的内存，不上传密钥；候选结果必须通过数据库 HMAC 校验才会保存。")}</p>
              <div className="wxv-row"><button type="button" className="wxv-btn primary" onClick={() => void scanWindowsKeys()} disabled={windowsScanBusy || !account}>{windowsScanBusy ? t("扫描并校验中…") : t("扫描微信内存并校验")}</button><span className="wxv-muted">{t("支持旧版 x'&lt;key&gt;&lt;salt&gt;' 格式；新版本可导入可信 JSON。")}</span></div>
              {platform.key_acquisition && <p className="wxv-keyguide-path">{t("当前策略：")}<code>{platform.key_acquisition}</code></p>}
            </div>
          )}

          {showKeyGuide && (
            <>
              <WeChatCaptureGuide capture={connection.capture} />

              <WeChatKeyImport connection={connection} selection={selection} wizard={wizardOptions} />
            </>
          )}
        </div>

        {/* Capture already writes the passphrase file and imports it automatically, so this
                box is only for keys obtained elsewhere — keep it available, not mandatory. */}
        <details className="wxv-keyguide-manual">
          <summary>{t("手动导入（可选）：粘贴或选择密钥文件")}</summary>
          <textarea
            className="wxv-textarea"
            rows={3}
            value={keyText}
            onChange={e => setKeyText(e.target.value)}
            placeholder={t("粘贴 64 位十六进制口令，或 x'<key><salt>'，或口令 JSON")}
            spellCheck={false}
          />
          <div className="wxv-row">
            <label className="wxv-file">
              {t("选择文件")}
              <input
                type="file"
                accept=".json,application/json,text/plain"
                onChange={e => void importKeys(e.target.files?.[0])}
              />
            </label>
            <button
              type="button"
              className="wxv-btn"
              onClick={() => void importKeys()}
              disabled={busy === 'import'}
            >
              {busy === 'import' ? t("校验中…") : t("导入并校验")}
            </button>
          </div>
        </details>
        <div className="wxv-row">
          <span className="wxv-key-state">
            {encryptedDbs.length ? tf("已就绪 {0}/{1} 个加密库", keyedCount, encryptedDbs.length) : t("等待发现账号")}
          </span>
        </div>
        {keyedCount > 0 && keyVerifiedSecs && (
          <p className="wxv-keyguide-path">
            {t("密钥最近校验：")}{formatKeyDate(keyVerifiedSecs)}（{keyAgeDays} {t("天前）。微信更新或重新登录后口令会轮换，需要重新提取并导入。")}
          </p>
        )}
        {accountMissingKeys && keyedCount > 0 && (
          <p className="wxv-keyguide-partial">
            {t("还有")} {encryptedDbs.length - keyedCount} {t("个加密库没有匹配到密钥，通常是微信从未打开过的库，不影响已就绪的聊天记录。")}
          </p>
        )}
      </div>
    </li>
  );
}
