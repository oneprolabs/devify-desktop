import { t, tf } from '../../shared/i18n/locale';
import { errorText, nativeErrorCode } from '../../shared/i18n/errorText';
export function currentSessionSummaries<T extends { account_id: string; sessions: { username: string }[] }>(records: T[], accountId: string | null | undefined, username: string | null | undefined): T[] {
  return accountId && username ? records.filter(record => record.account_id === accountId && record.sessions.some(session => session.username === username)) : [];
}

export function eventViewState(loading: boolean, error: string, count: number): 'loading' | 'error' | 'empty' | 'results' {
  return loading ? 'loading' : count ? 'results' : error ? 'error' : 'empty';
}

export function syncErrorMessage(error: unknown): string {
  return nativeErrorCode(error) === 'ACCOUNT_UPLOAD_BUSY'
    ? t("该账号正在后台上传，请在任务中查看进度。")
    : tf("同步未完成：{0}", errorText(error));
}
