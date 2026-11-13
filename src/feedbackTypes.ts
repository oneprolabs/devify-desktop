export type DiagnosticSnapshot = { schema_version: number; generated_at: string; events: unknown[] };
export type FeedbackDraft = {
  client_id: string; environment: { app_version: string; platform: string; arch: string }; related_batch_ids: number[]; receiver: string; owner_id: number | null;
  category: string; description: string; contact: string; page: string;
  screenshots: { data: string }[]; diagnostics: DiagnosticSnapshot | null; attempted: boolean;
};
export type FeedbackReport = {
  id: string; category: string; description: string; status: string;
  reply: string; fix_version: string; created_at: string;
};
export const feedbackCategories: Record<string, string> = { wechat: '连接微信', backup: '聊天备份', upload: '上传分析', login: '登录', interface: '界面异常', other: '其他问题' };
export const feedbackStatuses: Record<string, string> = { received: '已收到', processing: '处理中', needs_info: '待补充', resolved: '已解决' };
export function imageSource(data: string) { return `data:image/${data.startsWith('/9j/') ? 'jpeg' : 'png'};base64,${data}`; }
