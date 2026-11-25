import { desktopInvoke as invoke } from '../../../shared/api/desktopInvoke';
import { t } from '../../../shared/i18n/locale';
import { errorText } from '../../../shared/i18n/errorText';
import type { DecryptOutcome } from './nativeTypes';

export type WechatRefreshIssue = {
  title: string;
  message: string;
  needsKeys?: boolean;
  detail?: string;
};

// These databases supply the conversation list and message bodies. A missing
// avatar, attachment, or search index should not block the text refresh.
const isChatContent = (rel: string) => rel === 'session/session.db'
  || /^message\/(?:biz_)?message_\d+\.db$/.test(rel);
const isReadable = (outcome: DecryptOutcome) => outcome.status === 'ok' || outcome.status === 'cached';

export function localRefreshError(reason: unknown): WechatRefreshIssue {
  return {
    title: t("聊天记录未能刷新"),
    message: t("请稍等片刻，再重试刷新。"),
    detail: errorText(reason),
  };
}

export async function syncWechatForRefresh(accountId: string): Promise<WechatRefreshIssue | null> {
  try {
    const outcomes = await invoke<DecryptOutcome[]>('wechat_sync', { accountId });
    const content = outcomes.filter(outcome => isChatContent(outcome.rel));
    const failures = content.filter(outcome => !isReadable(outcome));
    if (!failures.length && content.length) return null;
    if (failures.some(outcome => outcome.status === 'no-key' || outcome.error === '保存的密钥格式无效。')) {
      return {
        needsKeys: true,
        title: t("微信密钥需要更新"),
        message: t("重新获取或导入当前微信账号的密钥，才能读取新消息。"),
      };
    }
    return {
      title: !content.length ? t("未找到微信聊天记录") : content.some(isReadable)
        ? t("部分微信聊天记录未能刷新") : t("微信聊天记录未能刷新"),
      message: t("请确认微信已打开并登录，再重试刷新。"),
      detail: failures[0]?.error ? errorText(failures[0].error) : undefined,
    };
  } catch (reason) {
    return localRefreshError(reason);
  }
}
