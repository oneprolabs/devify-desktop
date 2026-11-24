import { t } from '../../shared/i18n/locale';
import { Icons } from '../../shared/ui/icons';
import type { WechatRefreshIssue } from '../connectors/wechat/refresh';
import './chat-refresh-notice.css';

export function ChatRefreshNotice({ issue, busy, hasHistory, onRecover, onRetry }: {
  issue: WechatRefreshIssue;
  busy: boolean;
  hasHistory: boolean;
  onRecover: () => void;
  onRetry: () => void;
}) {
  return <section className="chat-workbench-refresh-error chat-refresh-notice" aria-label={issue.title} aria-busy={busy}>
    <span className="chat-refresh-notice-icon" aria-hidden="true">{Icons.warning}</span>
    <div className="chat-refresh-notice-copy" role="alert">
      <strong>{issue.title}</strong>
      <p>{hasHistory && <>{t("已加载的聊天记录仍可查看。")} </>}{issue.message}</p>
    </div>
    <button type="button" className="chat-refresh-notice-action" disabled={busy} onClick={issue.needsKeys ? onRecover : onRetry}>
      {issue.needsKeys ? t("更新微信密钥") : t("重试刷新")}
    </button>
    {issue.detail && <details className="chat-refresh-notice-detail" key={issue.detail}>
      <summary>{t("查看错误详情")}</summary>
      <p>{issue.detail}</p>
    </details>}
  </section>;
}
