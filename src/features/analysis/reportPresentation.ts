import { t } from '../../shared/i18n/locale';

export type ReportPresentationInput = {
  title?: string;
  status?: string | null;
  body?: string | null;
  content?: unknown;
  period_start?: string | null;
  period_end?: string | null;
  snapshot_ref?: { schema_version?: string } | null;
  processing?: { generator?: string } | null;
};

function ruleSnapshot(report: ReportPresentationInput): boolean {
  const generator = report.processing?.generator;
  return report.snapshot_ref?.schema_version === 'report.v1'
    && (generator === 'rules' || generator === 'llm_pending' || generator === 'llm_failed');
}

export function reportSectionTitle(section: { section_key?: string; title: string }, report: ReportPresentationInput): string {
  const content = report.content;
  // A successful LLM run can replace the body without replacing the rule sections.
  const retained = report.snapshot_ref?.schema_version === 'report.v1' && report.processing?.generator === 'llm'
    && content && typeof content === 'object' && 'llm_persisted' in content && content.llm_persisted === false;
  if (!ruleSnapshot(report) && !retained) return section.title;
  const template = [
    ['ongoing', 'Ongoing events', '进行中的待办'],
    ['notes', 'Notes', '笔记'],
    ['done', 'Completed events', '已完成的待办'],
  ].find(([key, title]) => section.section_key === key && section.title === title);
  return template ? t(template[2]) : section.title;
}

function periodInstant(value?: string | null): number {
  // Python's snapshot uses a space, while the API uses ISO 8601. Compare instants without dropping microseconds.
  const stamp = /^(\d{4}-\d\d-\d\d)[T ](\d\d:\d\d:\d\d)(?:\.(\d{1,6}))?(Z|[+-]\d\d:\d\d)$/.exec(value ?? '');
  if (!stamp) return NaN;
  const fraction = (stamp[3] ?? '').padEnd(6, '0');
  return Date.parse(`${stamp[1]}T${stamp[2]}.${fraction.slice(0, 3)}${stamp[4]}`) * 1000 + Number(fraction.slice(3));
}

export function reportPeriodDate(value?: string | null): string | null {
  const instant = periodInstant(value);
  return Number.isFinite(instant) ? new Date(Math.floor(instant / 1000)).toISOString() : null;
}

function snapshotBody(body: string, report: ReportPresentationInput): string | null {
  const prefix = /^(# ([^\r\n]+)\r?\n\r?\n)Period: ([^\r\n]+) to ([^\r\n]+)(?:\r?\n\r?\n|$)/.exec(body);
  if (body === `# ${report.title}`) return null;
  if (!prefix || prefix[2] !== report.title || (!report.period_start && !report.period_end)) return body;
  const matches = (shown: string, value?: string | null) =>
    value ? periodInstant(shown) === periodInstant(value) : shown === '—';
  if (!matches(prefix[3], report.period_start) || !matches(prefix[4], report.period_end)) return body;
  const rest = body.slice(prefix[0].length);
  return rest.trim() ? prefix[1] + rest : null;
}

/** Keep status separate from saved content so an incomplete report stays readable. */
export function reportPresentation(report: ReportPresentationInput, hasItems = false): { notice: string | null; content: string | null } {
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
  if (content && content === report.body && ruleSnapshot(report)) content = snapshotBody(content, report);

  const hasSavedContent = Boolean(content) || hasItems;
  if (waiting) {
    return {
      notice: hasSavedContent
        ? t("正在生成这份报告，以下展示已保存的内容。")
        : t("正在生成这份报告，完成后会自动显示。"),
      content,
    };
  }
  if (status === 'partial') {
    return {
      notice: hasSavedContent
        ? t("报告未完全生成，以下展示已保存的内容。")
        : t("报告未完全生成，暂无已保存的正文。"),
      content,
    };
  }
  if (status === 'failed') {
    return {
      notice: hasSavedContent
        ? t("报告生成失败，以下展示已保存的内容。")
        : t("报告生成失败，暂无已保存的正文。"),
      content,
    };
  }
  return { notice: null, content };
}
