export type DiagnosticSnapshot = { schema_version: number; generated_at: string; lines?: string[]; events?: unknown[] };
/** Schema version 2 is one greppable text line per record; older drafts are JSON. */
export function diagnosticText(snapshot: DiagnosticSnapshot | null): string {
  if (!snapshot) return '';
  if (snapshot.lines) return snapshot.lines.join('\n');
  return JSON.stringify(snapshot, null, 2);
}
export function diagnosticCount(snapshot: DiagnosticSnapshot | null): number {
  return snapshot?.lines?.length ?? snapshot?.events?.length ?? 0;
}
export type FeedbackDraft = {
  client_id: string; environment: { app_version: string; platform: string; arch: string; os_version: string; wechat_version: string }; related_batch_ids: number[]; receiver: string; owner_id: number | null;
  category: string; description: string; page: string;
  screenshots: { data: string }[]; diagnostics: DiagnosticSnapshot | null; attempted: boolean;
};
export type FeedbackReport = {
  id: string; category: string; description: string; status: string;
  reply: string; fix_version: string; created_at: string;
  diagnostics_dropped?: boolean;
};
export const feedbackCategories: Record<string, string> = { wechat: '连接微信', backup: '聊天备份', upload: '上传分析', login: '登录', interface: '界面异常', other: '其他问题' };
// From the reporter's side "received" and "processing" are the same phase — we have it and
// are working on it — so both read 处理中; only "needs_info" and "resolved" need their own word.
export const feedbackStatuses: Record<string, string> = { received: '处理中', processing: '处理中', needs_info: '待补充', resolved: '已解决' };
export function imageSource(data: string) { return `data:image/${data.startsWith('/9j/') ? 'jpeg' : 'png'};base64,${data}`; }
/** A UUID is unreadable and can't be quoted over chat; show a short stable handle instead. */
export function feedbackCode(id: string) { return `#${id.replace(/-/g, '').slice(0, 8).toUpperCase()}`; }
