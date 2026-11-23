import { WeChatSetupChecklist } from '../wizard/WeChatSetupChecklist';
import type { WeChatConnection } from './useWeChatConnection';
import type { WeChatAccountSelection, WeChatWizard } from './connectionTypes';

export function WeChatConnectionWizard({ connection, selection, wizard, ready, error, checkAccountExists }: {
  connection: WeChatConnection; selection: WeChatAccountSelection; wizard: WeChatWizard;
  ready: boolean; error: string; checkAccountExists: () => Promise<boolean>;
}) {
  const { accountId, scan, accountSwitching, busy, doScan, switchAccount } = selection;
  const { install: wizardInstall, onInstallRecheck: onWizardInstallRecheck, manual: wizardManual, step: wizardStep, finishing: wizardFinishing, onStep: onWizardStep } = wizard;
  const { capture, decryption } = connection;
  const { decrypt } = decryption;
  const { keyedCount, keyStatus, savedKeySource, freshSource, windowsScanBusy, fixKeyPermissions, scanWindowsKeys, importSavedKey, importKeys } = connection.keys;
  const { captureStatus, captureStatusChecking, captureStatusError, captureBusy, launchDiagnosis, recoveryError, recoveryNotice, refreshCaptureStatus, prepareCapture, openCaptureWeChat, confirmCaptureLaunch, runCapturePreflight, startCapture, restoreWeChat, acceptOfficialWeChat, openPrivacySecuritySettings } = capture;
  return <WeChatSetupChecklist
    accountId={accountId} accounts={scan?.accounts ?? []} scanLoaded={scan !== null}
    busy={busy} switching={accountSwitching} keyedCount={keyedCount} keyStatusLoaded={keyStatus !== null}
    savedKey={savedKeySource} freshKey={freshSource} decrypted={ready || decryption.decrypted}
    capture={captureStatus} captureChecking={captureStatusChecking} captureError={captureStatusError} captureBusy={captureBusy} windowsBusy={windowsScanBusy}
    install={wizardInstall} onInstallRecheck={() => onWizardInstallRecheck?.()}
    launchDiagnosis={launchDiagnosis} error={error} recoveryError={recoveryError} recoveryNotice={recoveryNotice} manual={wizardManual} permissionPhase={wizardStep === 'wizard-permission'} finishing={wizardFinishing}
    onPermissionNext={() => onWizardStep?.('wizard-connect')} onFixPermissions={fixKeyPermissions}
    onSystemBack={() => onWizardStep?.('wizard-permission')}
    onScan={() => doScan(accountId)} onChooseAccount={id => void switchAccount(id)}
    onCaptureRefresh={refreshCaptureStatus} onPrepare={prepareCapture}
    onOpen={openCaptureWeChat} onConfirmLogin={confirmCaptureLaunch}
    onPreflight={runCapturePreflight} onCapture={startCapture}
    onCheckStep={(action, context) => connection.checkStep(action, context, checkAccountExists)}
    onRestartConnection={capture.restart}
    onCheckConnectionResult={connection.checkConnectionResult}
    onRestore={restoreWeChat} onAcceptOfficial={acceptOfficialWeChat} onOpenSecurity={openPrivacySecuritySettings}
    onWindowsScan={scanWindowsKeys} onImportSaved={importSavedKey}
    onImport={importKeys} onImportText={text => importKeys(undefined, text)} onDecrypt={decrypt}
    onComplete={() => onWizardStep?.('wizard-sessions')} onBack={() => onWizardStep?.(wizardStep === 'wizard-permission' ? 'wizard-method' : 'wizard-permission')}
  />;
}
