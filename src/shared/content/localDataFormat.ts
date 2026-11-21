import { t, tf, dateLocale } from '../i18n/locale';

export function formatSize(bytes: number): string {
  if (bytes > 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
  if (bytes > 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

export function formatAge(seconds: number | null | undefined): string {
  if (seconds == null) return t("时间未知");
  if (seconds < 60) return t("刚刚");
  if (seconds < 3600) return tf("{0} 分钟前", Math.floor(seconds / 60));
  if (seconds < 86400) return tf("{0} 小时前", Math.floor(seconds / 3600));
  return tf("{0} 天前", Math.floor(seconds / 86400));
}

export function formatKeyDate(seconds: number | null | undefined): string {
  if (!seconds) return t("时间未知");
  return new Date(seconds * 1000).toLocaleDateString(dateLocale(), {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  });
}
