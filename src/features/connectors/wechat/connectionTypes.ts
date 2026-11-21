import type { ScanResult } from './nativeTypes';

/** The reader retains its existing operation indicator; connection imports share it. */
export type ConnectionFeedback = {
  setNotice: (value: string) => void;
  setError: (value: string) => void;
  setBusy: (value: 'import' | null) => void;
};

export type WeChatAccountSelection = {
  accountId: string;
  scan: ScanResult | null;
  accountSwitching: boolean;
  busy: string | null;
  doScan: (preferredId?: string) => Promise<void>;
  switchAccount: (id: string) => Promise<void>;
};

export type WeChatWizard = {
  enabled: boolean;
  step: string;
  manual: boolean;
  finishing: boolean;
  install: { ready: boolean | null; message: string };
  onInstallRecheck?: () => void;
  onStep?: (step: string) => void;
  onBack?: () => void;
  onNext?: () => void;
};
