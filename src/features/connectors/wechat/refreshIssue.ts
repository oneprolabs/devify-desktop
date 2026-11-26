export type RefreshOutcome = { rel: string; status: string; error?: string | null };

export type ClassifiedRefresh = {
  title: string;
  message: string;
  needsKeys?: boolean;
  wal: boolean;
  kind?: string;
  error?: string;
};

// These databases supply the conversation list and message bodies. A missing
// avatar, attachment, or search index should not block the text refresh.
const isChatContent = (rel: string) => rel === 'session/session.db'
  || /^message\/(?:biz_)?message_\d+\.db$/.test(rel);
const isReadable = (outcome: RefreshOutcome) => outcome.status === 'ok' || outcome.status === 'cached';

export function databaseKind(rel: string): string {
  if (rel === 'session/session.db') return '会话数据库';
  if (rel.startsWith('message/')) return '消息数据库';
  return '数据库';
}

export function classifyWechatRefresh(outcomes: RefreshOutcome[]): ClassifiedRefresh | null {
  const content = outcomes.filter(outcome => isChatContent(outcome.rel));
  const failures = content.filter(outcome => !isReadable(outcome));
  if (!failures.length && content.length) return null;
  if (failures.some(outcome => outcome.status === 'no-key' || outcome.error === '保存的密钥格式无效。')) {
    return {
      wal: false,
      needsKeys: true,
      title: '微信密钥需要更新',
      message: '重新获取或导入当前微信账号的密钥，才能读取新消息。',
    };
  }
  const failure = failures[0];
  const wal = Boolean(failure?.error?.includes('WAL'));
  return {
    wal,
    title: !content.length
      ? '未找到微信聊天记录'
      : content.some(isReadable) ? '部分微信聊天记录未能刷新' : '微信聊天记录未能刷新',
    message: wal
      ? '最新记录暂时读不到，已保留上一份记录。'
      : '请确认微信已打开并登录，再重试刷新。',
    kind: failure ? databaseKind(failure.rel) : undefined,
    error: failure?.error || undefined,
  };
}
