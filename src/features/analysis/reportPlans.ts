import { t } from '../../locale';
import { findDailyPlan, type DailyPlan } from '../../dailyPlan';

export type ReportPeriod = 'day' | 'week' | 'month';
export const REPORT_PLANS = [
  { period: 'day', title: '每日报告', label: '日报', frequency: '每天生成，汇总此前 24 小时。', aliases: ['每日报告', 'Daily report', '微信工作日报', 'WeChat daily'] },
  { period: 'week', title: '每周报告', label: '周报', frequency: '每周一生成，汇总此前 7 天。', aliases: ['每周报告', 'Weekly report', '微信工作周报', 'WeChat weekly'] },
  { period: 'month', title: '每月报告', label: '月报', frequency: '每月 1 日生成，汇总此前一个月。', aliases: ['每月报告', 'Monthly report', '微信工作月报', 'WeChat monthly'] },
] as const;
export type ReportDefinition = (typeof REPORT_PLANS)[number];

export function reportPlanSelection(value: unknown, definition: ReportDefinition): { plan: DailyPlan | null; others: number } {
  if (!Array.isArray(value)) throw new Error(t('未能读取报告计划，请重新读取。'));
  const candidates = value.filter(item => item?.period === definition.period && (definition.aliases as readonly string[]).includes(item?.title));
  const generic = candidates.filter(item => item.title === definition.title || item.title === definition.aliases[1]);
  const preferred = generic.length ? generic : candidates;
  if (preferred.length > 1) throw new Error(t('发现多个同类报告计划，请在主题中核对后重新读取。'));
  const plan: DailyPlan | null = definition.period === 'day' ? findDailyPlan(value) : preferred[0] ?? null;
  if (plan && (!Number.isSafeInteger(plan.id) || plan.id <= 0 || typeof plan.enabled !== 'boolean'
    || typeof plan.timezone !== 'string' || (plan.time_local !== null && !/^([01]\d|2[0-3]):[0-5]\d(?::[0-5]\d)?$/.test(plan.time_local)))) {
    throw new Error(t('未能读取报告计划，请重新读取。'));
  }
  return { plan, others: value.filter(item => item?.period === definition.period && item.id !== plan?.id && item.enabled && item.time_local).length };
}
