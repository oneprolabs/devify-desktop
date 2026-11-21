export type SummaryBudget = { unit: 'utf8_bytes'; max_request_bytes: number; prompt_bytes: number; reserved_overhead_bytes: number; max_content_bytes: number };
export type SummaryScenario = { id: string; name: string; description: string; requires_instructions: boolean; sha256: string; budgets: { cloud: SummaryBudget; local_cli: SummaryBudget } };
export type SummaryCatalog = { schema_version: 1; version: string; default_scenario: string; scenarios: SummaryScenario[]; max_instructions_chars: number };
export type SummaryOptionsView = { config: SummaryCatalog; language: string; ui_language: string; instructions: string };
export type SummaryDraft = { scenario: string; language: string; instructions: string };
export const summaryScenarioLabel = (id: string | null | undefined): string =>
  ({ three_part: '综合分析', summary: '总结', action_items: '待办', timeline: '时间线', custom: '自定义' } as Record<string, string>)[id ?? ''] ?? '';

export function summaryOptionFields(view: SummaryOptionsView | null, draft: SummaryDraft) {
  if (!view) throw Error('总结方式尚未初始化，请联网后点击「重新初始化」。');
  const selected = view.config.scenarios.find(item => item.id === draft.scenario);
  if (!selected) throw Error('所选总结方式已不可用，请重新选择。');
  const instructions = draft.instructions.trim();
  if (Array.from(instructions).length > view.config.max_instructions_chars) throw Error('我的要求过长，请缩短后重试。');
  if ((selected.requires_instructions || selected.id === 'custom') && !instructions) throw Error('请填写自定义总结的目标或要求。');
  const language = draft.language === '跟随界面' ? view.ui_language : draft.language || view.language;
  return { scenario: selected.id, language, instructions, prompt_version: view.config.version, prompt_digest: selected.sha256 };
}
