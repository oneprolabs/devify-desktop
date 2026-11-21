import { t } from '../../../shared/i18n/locale';
import { Icons } from '../../../shared/ui/icons';
import type { WeChatCapture } from './useWeChatCapture';

export function WeChatCaptureGuide({ capture }: { capture: WeChatCapture }) {
  const { captureStatus, captureBusy, captureRiskAccepted, setCaptureRiskAccepted, prepareCapture, launchDiagnosis, openPrivacySecuritySettings, openCaptureWeChat, confirmCaptureLaunch, restoreWeChat, runCapturePreflight, startCapture } = capture;
  /** Null status means the first probe has not returned yet: stay neutral instead of alarming. */
  const checkTone = (value: boolean | undefined) => (value === undefined ? 'pending' : value ? 'ok' : 'bad');
  const checkMark = (value: boolean | undefined) =>
    value === undefined ? Icons.minus : value ? Icons.check : Icons.close;

  return (
    <>
      {/* Environment: the reference flow refuses to touch WeChat unless every
                      one of these checks passes, so they are shown before the first action. */}
      <div id="wizard-detail-key-env" className="wxv-keyguide-step">
        <strong>{t("1 · 环境检查")}</strong>
        <ul className="wxv-keyguide-checks">
          <li>
            <span className={`wxv-keyguide-check ${checkTone(captureStatus?.platform_supported)}`}>
              {checkMark(captureStatus?.platform_supported)}
            </span>
            {t("Apple Silicon Mac（应用内捕获目前只验证了这一架构）")}
          </li>
          <li>
            <span className={`wxv-keyguide-check ${checkTone(captureStatus?.lldb_available)}`}>
              {checkMark(captureStatus?.lldb_available)}
            </span>
            LLDB{captureStatus?.lldb_available ? t(" 已就绪") : t(" 或 Python 运行环境无法启动：请修复 Xcode Command Line Tools")}
          </li>
          <li>
            {/* A running transaction re-signs WeChat on purpose: showing that as a
                            failure would tell the user to undo the step they just took. */}
            <span
              className={`wxv-keyguide-check ${checkTone(
                captureStatus?.wechat_official ||
                (Boolean(captureStatus?.pending) && Boolean(captureStatus?.wechat_ad_hoc))
              )}`}
            >
              {checkMark(
                captureStatus?.wechat_official ||
                (Boolean(captureStatus?.pending) && Boolean(captureStatus?.wechat_ad_hoc))
              )}
            </span>
            {t("微信")} {captureStatus?.wechat_version ?? t("未找到")}
            {captureStatus?.wechat_build ? `（${captureStatus.wechat_build}）` : ''}
            {captureStatus?.wechat_official
              ? t(" · 腾讯原签名")
              : captureStatus?.wechat_ad_hoc
                ? captureStatus?.pending
                  ? t(" · 本次捕获的临时签名（结束后自动换回原版）")
                  : t(" · 当前是临时签名，请先恢复原版")
                : t(" · 不是可验证的腾讯签名")}
          </li>
          <li>
            <span className={`wxv-keyguide-check ${checkTone(captureStatus?.user_can_write)}`}>
              {checkMark(captureStatus?.user_can_write)}
            </span>
            {t("当前用户可写入微信与「应用程序」（保证异常退出后也能恢复原版）")}
          </li>
        </ul>
        <p className="wxv-keyguide-path">
          {t("备份目录：")}<span className="mono">{captureStatus?.backup_root ?? '~'}</span>
        </p>
        {captureStatus?.message && <p className="wxv-keyguide-note">{t(captureStatus.message)}</p>}
      </div>

      {!captureStatus?.pending && (
        <div id="wizard-detail-key-capture" className="wxv-keyguide-step">
          <strong>{t("2 · 备份并临时重签微信")}</strong>
          <p className="wxv-muted">
            {t("程序会先用")} <code>ditto</code> {t("建立腾讯原版压缩备份和同卷写时复制保护，再把「副本」临时\n                        ad-hoc 签名，最后原子替换安装位置。部分系统使用无沙箱的深度重签，微信可能使用不同的数据目录。")}
          </p>
          <div className="wxv-keyguide-confirm">
            <strong>{t("开始前请确认")}</strong>
            <span>
              {t("微信会被退出并需要重新扫码登录；请先处理未保存的内容。若微信或其安装目录不可写，程序会直接停止。")}
            </span>
            <p>
              {t("捕获期间微信会以临时签名运行。结束、取消或失败时程序会自动换回腾讯原版并校验\n                          TeamIdentifier / 版本 / CDHash；无法证明安全时不会覆盖，而是保留恢复资产并报错。")}
            </p>
            {captureStatus?.wechat_quarantined && (
              <p className="wxv-keyguide-note">
                {t("这台 Mac 的微信仍带")} <code>com.apple.quarantine</code> {t("隔离属性：临时副本是 ad-hoc\n                            签名，腾讯的公证票据随之失效，首次启动会被 Gatekeeper 拦下。macOS 15\n                            起没有右键「打开」入口，需要在「系统设置 → 隐私与安全性 → 安全性」里点一次\n                            「仍要打开」（被拦下后 1 小时内有效）；15.1 还可能连提示都不弹。")}
              </p>
            )}
            <label className="wxv-keyguide-accept">
              <input
                type="checkbox"
                checked={captureRiskAccepted}
                onChange={event => setCaptureRiskAccepted(event.target.checked)}
              />
              {t("我了解上述风险，并确认这台 Mac 与微信数据归我本人所有")}
            </label>
            <div className="wxv-row">
              <button
                type="button"
                className="wxv-btn primary"
                onClick={() => void prepareCapture()}
                disabled={
                  !captureRiskAccepted ||
                  captureBusy !== null ||
                  !captureStatus?.platform_supported ||
                  !captureStatus?.lldb_available ||
                  !captureStatus?.wechat_official ||
                  !captureStatus?.user_can_write
                }
              >
                {captureBusy === 'prepare' ? t("正在备份并重签…") : t("备份并临时重签")}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Step 3 only becomes actionable after step 2, but its existence has to be
                      visible beforehand — otherwise the checklist reads 1, 2, 4 and looks broken. */}
      {!captureStatus?.pending && (
        <div id="wizard-detail-key-login" className="wxv-keyguide-step">
          <strong>{t("3 · 打开微信、登录并完成捕获")}</strong>
          <p className="wxv-muted">
            {t("完成第 2 步后这里会出现具体操作：打开微信 → 检查启动 → 检查捕获断点（一次管理员授权）→\n                        在微信里退出账号 → 启动监测（再一次授权）→ 重新登录同一账号，程序随后自动恢复并导入。")}
          </p>
        </div>
      )}

      {captureStatus?.pending && (
        <div className="wxv-keyguide-step">
          <strong>{t("3 · 打开微信、登录并完成捕获")}</strong>
          {captureStatus.entitlement_mode === 'deep' && (
            <p className="wxv-keyguide-note">{t("临时微信及其内部组件已重签，并以无沙箱模式运行，可能使用不同的数据目录。结束或取消时会恢复官方微信。")}</p>
          )}
          {captureStatus.entitlement_mode === 'sanitized' && (
            <p className="wxv-keyguide-note">
              {t("本次临时副本已剔除腾讯团队的受限权限（application-identifier / team-identifier /\n                          application-groups / temporary-exception）。这台 Mac 的签名校验不允许 ad-hoc\n                          签名携带它们，保留就会在启动前被系统拒绝（launchd 153）。结束或取消时仍会换回腾讯原版。")}
            </p>
          )}
          <ol className="wxv-keyguide-steps">
            <li>
              <strong>{t("打开微信并完整登录")}</strong>{t("：点下面的「打开微信」；如果 macOS\n                          提示无法验证或双击没有反应，按下方说明放行一次。登录后进入要读取的聊天，让本地数据库生成。")}
            </li>
            <li>
              <strong>{t("检查启动")}</strong>{t("：程序确认运行的是本次临时副本后，再检查捕获断点（这一步会弹管理员\n                          授权，密码只交给 macOS）。")}
            </li>
            <li>
              <strong>{t("在微信里退出账号")}</strong>{t("：断点可用后再退出；不要提前退出，否则这次捕获作废。")}
            </li>
            <li>
              <strong>{t("启动监测并授权")}</strong>{t("：再次输入管理员密码。看到「监测已就绪」之前不要重新登录。")}
            </li>
            <li>
              <strong>{t("重新登录同一账号")}</strong>{t("：捕获成功后程序自动逐库校验、恢复腾讯原版微信并导入密钥。")}
            </li>
          </ol>

          {launchDiagnosis && (
            <div className="wxv-launch-diag">
              <strong>
                {launchDiagnosis.amfi_rejected
                  ? t("系统在启动前拒绝了这次临时签名（AMFI）")
                  : t("系统没有启动临时微信")}
              </strong>
              <p className="wxv-muted">{t(launchDiagnosis.hint)}</p>
              <ul className="wxv-launch-diag-facts">
                <li>
                  {t("隔离属性")} <code>com.apple.quarantine</code>：
                  {launchDiagnosis.quarantined ? t("存在（Gatekeeper 会评估这个副本）") : t("不存在")}
                </li>
                <li>
                  {t("签名：")}
                  {launchDiagnosis.resigned
                    ? t("本次临时 ad-hoc 副本")
                    : t("不是本次调试副本，请先检查是否在「本地数据」重新开始")}
                </li>
                <li>
                  {t("Gatekeeper 评估：")}
                  {launchDiagnosis.gatekeeper_accepted === null
                    ? t("无法评估")
                    : launchDiagnosis.gatekeeper_accepted
                      ? t("已通过")
                      : t("被拒绝")}
                  {launchDiagnosis.gatekeeper_reason
                    ? `（${launchDiagnosis.gatekeeper_reason}）`
                    : ''}
                </li>
                {launchDiagnosis.amfi_rejected && (
                  <li>{t("受限权限检查：被拒绝（launchd 153，进程在创建阶段就被杀死）")}</li>
                )}
                <li>{t("当前微信进程数：")}{launchDiagnosis.processes}</li>
              </ul>
              {launchDiagnosis.open_error && (
                <p className="wxv-launch-diag-error">{t("open 返回：")}{launchDiagnosis.open_error}</p>
              )}
              {!launchDiagnosis.amfi_rejected && (
                <ol className="wxv-keyguide-steps">
                  <li>
                    {t("在 Finder 里双击")} <code>/Applications/WeChat.app</code>
                    {t("。如果弹出「无法打开 / 无法验证」，点「完成」，不要点「移到废纸篓」。")}
                  </li>
                  <li>
                    {t("1 小时内打开")} <strong>{t("系统设置 → 隐私与安全性 → 安全性")}</strong>
                    {t("，看到「“WeChat” 已被阻止…」就点「仍要打开」→ 输入管理员密码 → 再点「打开」。\n                                macOS 15 起没有右键「打开」入口；15.1 可能完全不弹窗，只能走这里。")}
                  </li>
                  <li>
                    {t("放行后微信会正常启动；扫码登录完成后回到这里点「我已登录，检查启动」。")}
                  </li>
                  <li>
                    {t("如果系统设置里根本没有「仍要打开」，说明不是 Gatekeeper：先用「控制台」查看微信是否启动即退出，\n                                或直接点「取消并恢复微信」结束本次捕获，稍后再排查崩溃原因。")}
                  </li>
                </ol>
              )}
              <div className="wxv-keyguide-actions">
                {!launchDiagnosis.amfi_rejected && (
                  <>
                    <button
                      type="button"
                      className="wxv-btn primary"
                      onClick={() => void openPrivacySecuritySettings()}
                    >
                      {t("打开隐私与安全性设置")}
                    </button>
                    <button
                      type="button"
                      className="wxv-btn"
                      onClick={() => void openCaptureWeChat()}
                      disabled={captureBusy !== null}
                    >
                      {captureBusy === 'open' ? t("正在打开…") : t("再打开一次微信")}
                    </button>
                    <button
                      type="button"
                      className="wxv-btn"
                      onClick={() => void confirmCaptureLaunch()}
                      disabled={captureBusy !== null}
                    >
                      {captureBusy === 'confirm' ? t("正在检查…") : t("重新检查启动")}
                    </button>
                  </>
                )}
                <button
                  type="button"
                  className={`wxv-btn ${launchDiagnosis.amfi_rejected ? 'primary' : ''}`}
                  onClick={() => void restoreWeChat()}
                  disabled={captureBusy !== null}
                >
                  {captureBusy === 'restore' ? t("正在恢复…") : t("取消并恢复微信")}
                </button>
              </div>
            </div>
          )}

          <div className="wxv-keyguide-actions">
            {(captureStatus.stage === 'awaiting_manual_launch' ||
              captureStatus.stage === 'resigned' ||
              captureStatus.stage === 'debug_prepared' ||
              captureStatus.stage === 'backup_verified') && (
                <>
                  <button
                    type="button"
                    className="wxv-btn primary"
                    onClick={() => void openCaptureWeChat()}
                    disabled={captureBusy !== null}
                  >
                    {captureBusy === 'open' ? t("正在打开…") : t("打开微信")}
                  </button>
                  <button
                    type="button"
                    className="wxv-btn"
                    onClick={() => void confirmCaptureLaunch()}
                    disabled={captureBusy !== null}
                  >
                    {captureBusy === 'confirm' ? t("正在检查…") : t("我已登录，检查启动")}
                  </button>
                </>
              )}
            {captureStatus.stage === 'launched' && (
              <button
                type="button"
                className="wxv-btn primary"
                onClick={() => void runCapturePreflight()}
                disabled={captureBusy !== null}
              >
                {captureBusy === 'preflight' ? t("正在检查断点…") : t("检查捕获断点（需管理员授权）")}
              </button>
            )}
            {captureStatus.stage === 'preflight_passed' && (
              <button
                type="button"
                className="wxv-btn primary"
                onClick={() => void startCapture()}
                disabled={captureBusy !== null}
              >
                {captureBusy === 'start' ? t("监测中，等待登录…") : t("我已退出账号，启动监测（需管理员授权）")}
              </button>
            )}
            {captureStatus.stage === 'capturing' && (
              <>
                {captureStatus.resume_available ? (
                  // The monitor finished after an app restart: the result is on disk
                  // and only needs validating + restoring, so no second login.
                  <button
                    type="button"
                    className="wxv-btn primary"
                    onClick={() => void startCapture()}
                    disabled={captureBusy !== null}
                  >
                    {captureBusy === 'start'
                      ? t("正在校验并恢复…")
                      : t("校验并恢复已捕获的密钥")}
                  </button>
                ) : (
                  <span className="wxv-keyguide-progress">
                    {captureStatus.capture_phase === 'waiting_authorization'
                      ? t("等待管理员授权…")
                      : captureStatus.capture_phase === 'monitoring'
                        ? t("监测已就绪：现在重新登录同一账号")
                        : captureStatus.capture_phase === 'captured'
                          ? t("已捕获密钥，正在结束调试会话，随后校验并恢复微信")
                          : captureStatus.capture_phase === 'validating'
                            ? t("正在逐库校验…")
                            : t("正在恢复腾讯原版微信…")}
                  </span>
                )}
              </>
            )}
            <button
              type="button"
              className="wxv-btn danger"
              onClick={() => void restoreWeChat()}
              disabled={captureBusy !== null}
              title={captureBusy === 'start' ? t("监测进行中：请等它超时或完成后再恢复") : undefined}
            >
              {captureBusy === 'restore' ? t("正在恢复…") : t("取消并恢复官方微信")}
            </button>
          </div>

          {captureStatus.prepared_process_exited && captureStatus.stage !== 'capturing' && (
            <p className="wxv-keyguide-warn">
              {t("临时微信进程已退出。请点「取消并恢复官方微信」换回腾讯原版后重新开始，不要自行改签名。")}
            </p>
          )}
          {captureStatus.recovery_error_code && (
            <p className="wxv-keyguide-warn">
              {t("恢复被安全策略拦下（")}{captureStatus.recovery_error_code}{t("）。程序保留了应用、备份和恢复记录；\n                          请先点「取消并恢复官方微信」，按提示处理后再重新开始。")}
            </p>
          )}
        </div>
      )}
    </>
  );
}
