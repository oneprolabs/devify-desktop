import { en } from './enCatalog';
import { getUiLanguage, t, tf } from './locale';

const nativeErrorCodes = [
  'AUTH_CONFIG_UNAVAILABLE',
  'AUTH_CONFIG_INVALID',
  'ACCOUNT_UPLOAD_BUSY',
  'AUTH_UPLOAD_BUSY',
  'WHATSAPP_ACCESS_DENIED',
  'WHATSAPP_DATA_UNSUPPORTED',
  'WECHAT_SESSION_DB_MISSING',
  'WECHAT_ACCOUNT_UNAVAILABLE',
] as const;

type NativeErrorCode = typeof nativeErrorCodes[number];

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason ?? '');
}

function encodedError(message: string): NativeErrorCode | undefined {
  return nativeErrorCodes.find(code => message.startsWith(code + ': '));
}

export function nativeErrorCode(reason: unknown): NativeErrorCode | undefined {
  const message = errorMessage(reason);
  const code = encodedError(message);
  if (code) return code;

  // Compatibility with older native messages and entries already saved on disk.
  if (message.startsWith('读取服务端登录配置失败：服务端暂不可用')) return 'AUTH_CONFIG_UNAVAILABLE';
  if (/^(读取服务端登录配置失败|服务端登录配置)/.test(message)) return 'AUTH_CONFIG_INVALID';
  if (message.startsWith('该账号正在上传')) return 'ACCOUNT_UPLOAD_BUSY';
  if (message.startsWith('未找到解密后的')) return 'WECHAT_SESSION_DB_MISSING';
  if (/^(上传期间无法更改登录状态|备份正在使用登录状态)/.test(message)) return 'AUTH_UPLOAD_BUSY';
}

export function whatsAppReadErrorCode(reason: unknown): NativeErrorCode | undefined {
  const message = errorMessage(reason);
  const code = encodedError(message);
  if (code) return code;
  if (/permission denied|operation not permitted|os error (1|13)\b|权限|拒绝/i.test(message)) return 'WHATSAPP_ACCESS_DENIED';
  if (/不支持|缺少.*字段/.test(message)) return 'WHATSAPP_DATA_UNSUPPORTED';
}

export function errorText(reason: unknown, fallback?: string): string {
  let message = errorMessage(reason);
  if (!message) return '';
  const code = encodedError(message);
  if (code) message = message.slice(code.length + 2);
  if (getUiLanguage() !== 'English') return message;
  if (code === 'WHATSAPP_ACCESS_DENIED') return t("没有访问 WhatsApp 本地数据的权限。");
  if (code === 'WHATSAPP_DATA_UNSUPPORTED') return t("当前 WhatsApp 数据版本暂不支持读取，请保留数据并通过「问题反馈」联系我们。");

  const translated = t(message);
  if (translated !== message || !/\p{Script=Han}/u.test(message)) return translated;

  // Translate the leading context of native errors that carry a variable detail.
  const separator = message.indexOf('：');
  if (separator > 0) {
    const pattern = message.slice(0, separator + 1) + '{0}';
    if (Object.prototype.hasOwnProperty.call(en, pattern)) return tf(pattern, errorText(message.slice(separator + 1).trim()));
  }
  return t(fallback ?? "操作未完成，请重试。若问题仍存在，请通过问题反馈发送诊断信息。");
}
