import { desktopInvoke as invoke } from '../../../shared/api/desktopInvoke';
import { t } from '../../../shared/i18n/locale';
import { errorText, nativeErrorCode } from '../../../shared/i18n/errorText';
import type { DecryptOutcome } from './nativeTypes';
import { classifyWechatRefresh } from './refreshIssue';

export type WechatRefreshIssue = {
  title: string;
  message: string;
  needsKeys?: boolean;
  detail?: string;
};

type WechatRefreshResult =
  | { revision: string; issue?: never }
  | { revision?: never; issue: WechatRefreshIssue };

export function localRefreshError(reason: unknown): WechatRefreshIssue {
  return {
    title: t("聊天记录未能刷新"),
    message: t("请稍等片刻，再重试刷新。"),
    detail: errorText(reason),
  };
}

export async function syncWechatForRefresh(accountId: string): Promise<WechatRefreshResult> {
  try {
    const { outcomes, revision } = await invoke<{ outcomes: DecryptOutcome[]; revision: string }>('wechat_sync', { accountId });
    const classified = classifyWechatRefresh(outcomes);
    if (!classified) return { revision };
    return { issue: {
      needsKeys: classified.needsKeys,
      title: t(classified.title),
      message: t(classified.message),
      detail: classified.error ? `${t(classified.kind || '数据库')}：${errorText(classified.error)}` : undefined,
    } };
  } catch (reason) {
    if (nativeErrorCode(reason) === 'WECHAT_ACCOUNT_UNAVAILABLE') {
      return { issue: {
        title: errorText(reason),
        message: t("请打开微信并确认已登录原账号，且本机数据访问权限未被关闭。"),
      } };
    }
    return { issue: localRefreshError(reason) };
  }
}
