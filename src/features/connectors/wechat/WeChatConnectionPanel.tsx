import { WeChatAccountStep } from './WeChatAccountStep';
import { WeChatKeyStep } from './WeChatKeyStep';
import { WeChatDecryptionStep } from './WeChatDecryptionStep';
import { t } from '../../../shared/i18n/locale';
import { Icons } from '../../../shared/ui/icons';
import type { WeChatConnection } from './useWeChatConnection';
import type { WeChatAccountSelection, WeChatWizard } from './connectionTypes';

export type WeChatConnectionPanelProps = {
  connection: WeChatConnection; selection: WeChatAccountSelection; wizard: WeChatWizard;
  mode: 'chat' | 'settings'; ready: boolean; notice: string; onOpenChat?: () => void;
};

/** Existing standalone connection view; the embedded wizard uses its own checklist. */
export function WeChatConnectionPanel({ connection, selection, wizard: wizardOptions, mode, ready, notice, onOpenChat }: WeChatConnectionPanelProps) {
  const { account, env, setForceUnlock } = connection;
  const { enabled: wizard, step, onBack: onWizardBack, onNext: onWizardNext } = wizardOptions;
  const { keyedCount, savedKeySource } = connection.keys;
  const { captureOutcome } = connection.capture;
  const { outcomes, decrypting } = connection.decryption;
  const usableDb = (rel: string) => outcomes.some(outcome =>
    outcome.rel === rel && (outcome.status === 'ok' || outcome.status === 'cached'));
  const decryptedForWizard = usableDb('session/session.db') && outcomes.some(outcome =>
    /^message\/message_\d+\.db$/.test(outcome.rel) && (outcome.status === 'ok' || outcome.status === 'cached'));
  const wizardCanNext = !wizard || (step === 'wizard-permission' || step === 'wizard-account'
    ? Boolean(account)
    : step === 'wizard-key'
      ? Boolean(savedKeySource?.exists || captureOutcome?.matched || keyedCount > 0)
      : step === 'wizard-import'
        ? keyedCount > 0
        : step === 'wx-connect-decrypt'
          ? !decrypting && (ready || decryptedForWizard)
          : true);
  return (
    <div className="wxv-onboard">
      <div className="wxv-onboard-head">
        {ready && (
          <button
            type="button"
            className="wxv-link"
            onClick={() => (onOpenChat ? onOpenChat() : setForceUnlock(false))}
          >
            {t("← 返回聊天记录")}
          </button>
        )}
        {mode !== 'settings' && <p className="wxv-eyebrow">{t("只读 · 本地解密")}</p>}
        <h2>{t("连接微信")}</h2>
        <p>
          {t("程序读取微信 4.x 的数据库副本，用你的密钥逐库校验第 1 页 HMAC 后解密到应用数据目录。原始数据库不会被修改。")}
        </p>
      </div>

      <nav className="wxv-connection-progress" aria-label={t("微信连接步骤")}>
        {[
          { id: 'wx-connect-account', label: t("查找账号"), done: Boolean(account) },
          { id: 'wx-connect-key', label: t("获取密钥"), done: keyedCount > 0 },
          { id: 'wx-connect-decrypt', label: t("解密记录"), done: ready },
        ].map((step, index) => <button key={step.id} type="button" onClick={() => document.getElementById(step.id)?.scrollIntoView({ block: 'start' })}>
          <span className={step.done ? 'complete' : ''}>{step.done ? Icons.check : index + 1}</span>{t(step.label)}
        </button>)}
      </nav>

      <ol className="wxv-steps">
        <WeChatAccountStep connection={connection} selection={selection} wizard={wizardOptions} />

        <WeChatKeyStep connection={connection} selection={selection} wizard={wizardOptions} />

        <WeChatDecryptionStep connection={connection} selection={selection} wizard={wizardOptions} ready={ready} notice={notice} />
      </ol>

      {wizard && step !== 'wizard-method' && (
        <div className="wizard-detail-actions">
          <button type="button" className="wxv-btn" onClick={onWizardBack}>{t("上一步")}</button>
          <button type="button" className="wxv-btn primary" onClick={onWizardNext} disabled={!wizardCanNext}>
            {step === 'wizard-permission' ? t("下一步：账号扫描") : step === 'wizard-account' ? t("下一步：获取密钥") : step === 'wizard-key' ? t("下一步：导入校验") : step === 'wizard-import' ? t("下一步：解密记录") : t("下一步：完成")}
          </button>
        </div>
      )}

      <details className="wxv-warn">
        <summary>{t("密钥获取与安全说明")}</summary>
        <p>
          {t("官方微信启用了 Hardened Runtime 和沙盒，直接附加调试器可能被系统拒绝。\n          应用内获取流程会先备份官方微信，再经你确认后使用临时签名捕获，完成或取消时恢复并校验原版。")}
        </p>
        <p>
          <strong>{t("不要自行重签微信。")}</strong>{t("签名或权限处理不当可能导致微信无法启动。\n          如不希望使用应用内捕获，也可以手动导入可信来源的密钥文件。")}
        </p>
        {env && (
          <p className="wxv-muted mono">
            {t("密钥文件：")}{env.keys_path}
            <br />
            {t("解密缓存：")}{env.cache_root}
          </p>
        )}
      </details>
    </div>
  );
}
