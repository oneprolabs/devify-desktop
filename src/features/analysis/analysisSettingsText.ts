import { t } from '../../shared/i18n/locale';
// Translate known backend error prefixes only. Arbitrary server errors can contain
// user-provided addresses or usernames, so never rewrite their embedded text.
export function analysisSettingsError(reason: unknown): string {
  const message = reason instanceof Error ? reason.message : String(reason);
  if (!/devify/i.test(message)) return message;

  const translated = message
    .replace(/^当前\s+devify\s*服务端不可用$/i, t("当前服务端不可用"))
    .replace(/^devify 请求失败：/i, t("服务端请求失败："))
    .replace(/^访问 devify 失败：/i, t("访问服务端失败："))
    .replace(/^读取 devify 响应失败：/i, t("读取服务端响应失败："))
    .replace(/^刷新 devify token 失败：/i, t("刷新服务端登录状态失败："))
    .replace(/^devify_analysis_preferences_(?:get|save)\s*(?=失败|$)/i, t("服务端请求"));

  // If the remainder still contains the old name it may be a URL, email, or
  // username. Prefer a truthful generic error to a fabricated identifier.
  return /devify/i.test(translated) ? t("服务端请求失败，请检查设置或网络。") : translated;
}

export function questionAnswerError(reason: unknown): string {
  const message = (
    reason instanceof Error ? reason.message : String(reason ?? '')
  ).trim();
  if (!message) return t("回答失败，请重新提问。");
  if (
    /litellm|openai(?:exception)?|badrequesterror|invalid request parameters|the answer model request failed|traceback|provider_invalid_request/i.test(
      message,
    )
  ) {
    return t("回答生成失败，请稍后重试。");
  }
  return message;
}
