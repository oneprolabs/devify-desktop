type ReportPresentationInput = {
  status?: string | null;
  body?: string | null;
  content?: unknown;
};

/** Keep status separate from saved content so an incomplete report stays readable. */
export function reportPresentation(report: ReportPresentationInput): { notice: string | null; content: string | null } {
  const text = (value: unknown): string | null =>
    typeof value === 'string' && value.trim() ? value : null;
  const status = report.status ?? '';
  const waiting = status === 'queued' || status === 'processing';
  let content = text(report.body) ?? text(report.content);

  // Preserve the existing object fallback for completed, failed, and legacy reports.
  // In incomplete reports `content` can be metadata such as item counts, not a body.
  if (!content && !waiting && status !== 'partial' && report.content && typeof report.content !== 'string') {
    content = JSON.stringify(report.content, null, 2) || null;
  }

  if (waiting) {
    return {
      notice: content
        ? '正在生成这份报告，以下展示已保存的内容。'
        : '正在生成这份报告，稍后刷新即可看到内容。',
      content,
    };
  }
  if (status === 'partial') {
    return {
      notice: content
        ? '报告未完全生成，以下展示已保存的内容。'
        : '报告未完全生成，暂无已保存的正文。',
      content,
    };
  }
  if (status === 'failed') {
    return {
      notice: content
        ? '报告生成失败，以下展示已保存的内容。'
        : '报告生成失败，暂无已保存的正文。',
      content,
    };
  }
  return { notice: null, content };
}
