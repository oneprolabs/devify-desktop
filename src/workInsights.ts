/** Rule-based helpers shared by the raw-record workspace and the chat summary tab.
 *  They only look at message text, so both callers can pass their own message shape. */

export type InsightCategory = '待办' | '决定' | '风险' | '问题' | '数据';

export function classifyWorkText(text: string): InsightCategory | null {
  if (/(风险|问题|异常|报错|失败|阻塞|卡住|延期|延迟|缺少|缺口|bug|故障|回滚)/i.test(text)) return '风险';
  if (/(决定|确定|定了|结论|同意|通过|就按|最终|敲定|拍板)/i.test(text)) return '决定';
  if (/(需要|请|麻烦|尽快|务必|安排|负责|跟进|推进|提交|截止|deadline|之前完成|今天|明天|后天|本周|下周|月底)/i.test(text)) return '待办';
  if (/(https?:\/\/|www\.|%|％|万元|完成率|gmv|roi|成本|预算|收入|转化)/i.test(text)) return '数据';
  if (text.trim().endsWith('？') || text.trim().endsWith('?')) return '问题';
  return null;
}

/** The three most repeated content words, used as the exploration keywords. */
export function extractWorkTerms(messages: Array<{ text: string }>): string[] {
  const stopWords = new Set(['我们', '这个', '那个', '可以', '已经', '还是', '然后', '一下', '现在', '因为', '所以', '没有', '不是']);
  const counts = new Map<string, number>();
  for (const item of messages) {
    const terms = item.text.match(/[\u4e00-\u9fff]{2,8}|[A-Za-z][A-Za-z0-9_-]{2,}/g) ?? [];
    for (const term of terms) {
      const normalized = term.toLowerCase();
      if (!stopWords.has(normalized) && normalized.length >= 2) counts.set(normalized, (counts.get(normalized) ?? 0) + 1);
    }
  }
  return [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 3).map(([term]) => term);
}
