import { desktopInvoke as invoke } from '../../../shared/api/desktopInvoke';
import { t, tf } from '../../../shared/i18n/locale';
import { errorText } from '../../../shared/i18n/errorText';
import type { DecryptOutcome } from './nativeTypes';

export type WechatRefreshIssue = { message: string; needsKeys?: boolean };

// These databases supply the conversation list and message bodies. A missing
// avatar, attachment, or search index should not block the text refresh.
const isChatContent = (rel: string) => rel === 'session/session.db'
  || /^message\/(?:biz_)?message_\d+\.db$/.test(rel);
const isReadable = (outcome: DecryptOutcome) => outcome.status === 'ok' || outcome.status === 'cached';

export function localRefreshError(reason: unknown): WechatRefreshIssue {
  return { message: tf("刷新会话失败：{0}", errorText(reason)) };
}

export async function syncWechatForRefresh(accountId: string): Promise<WechatRefreshIssue | null> {
  try {
    const outcomes = await invoke<DecryptOutcome[]>('wechat_sync', { accountId });
    const content = outcomes.filter(outcome => isChatContent(outcome.rel));
    const failures = content.filter(outcome => !isReadable(outcome));
    if (!failures.length && content.length) return null;
    const missingKey = failures.filter(outcome => outcome.status === 'no-key' || outcome.error === '保存的密钥格式无效。');
    if (missingKey.length) {
      const rotated = missingKey.some(outcome => outcome.error?.includes('数据库已重新加密'));
      return {
        needsKeys: true,
        message: rotated
          ? t("微信数据库已重新加密，旧密钥无法读取新消息。请到「连接器」重新导入当前账号的密钥。")
          : t("微信聊天数据库缺少有效密钥。请到「连接器」导入当前账号的密钥。"),
      };
    }
    if (!content.length) return { message: t("未找到微信聊天数据库。请确认微信已登录，再刷新重试。") };
    const detail = failures[0].error;
    return {
      message: detail
        ? (content.some(isReadable) ? tf("部分微信会话或消息未能刷新：{0}", errorText(detail)) : localRefreshError(detail).message)
        : t("微信聊天数据库读取失败，请刷新重试。"),
    };
  } catch (reason) {
    return localRefreshError(reason);
  }
}
