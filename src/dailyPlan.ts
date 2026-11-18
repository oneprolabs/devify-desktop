import { t } from './locale';
export type DailyPlan = { id: number; title: string; period: string; enabled: boolean; time_local: string | null; timezone: string; last_run_at?: string | null; last_error?: string | null };
export function findDailyPlan(value: unknown): DailyPlan | null {
  if (!Array.isArray(value)) throw new Error(t("服务端日报计划格式不正确。"));
  const candidates = value.filter((item): item is DailyPlan => [t("每日报告"), t("微信工作日报")].includes(item?.title) && item?.period === 'day');
  const preferredTitle = candidates.some(item => item.title === '每日报告') ? t("每日报告") : t("微信工作日报");
  const matches = candidates.filter(item => item.title === preferredTitle);
  if (matches.length > 1) throw new Error(t("发现多个同名日报计划，请先在服务端核对。"));
  const plan = matches[0];
  if (!plan) return null;
  if (!Number.isSafeInteger(plan.id) || typeof plan.enabled !== 'boolean' || typeof plan.timezone !== 'string' || (plan.time_local !== null && !/^([01]\d|2[0-3]):[0-5]\d(?::[0-5]\d)?$/.test(plan.time_local))) throw new Error(t("服务端日报计划格式不正确。"));
  return plan;
}

/** Recheck the server before creating; never replace an existing plan's time. */
export async function enableDailyPlan(api: { list: () => Promise<unknown>; create: (time: string) => Promise<unknown>; enable: (id: number) => Promise<unknown> }, time: string) {
  const existing = findDailyPlan(await api.list());
  if (existing) return api.enable(existing.id);
  if (!/^([01]\d|2[0-3]):[0-5]\d$/.test(time)) throw new Error(t("请选择有效的日报时间。"));
  return api.create(time);
}

export function stepTimePart(time: string, part: 'hour' | 'minute', step: number): string {
  const [hour, minute] = time.split(':').map(Number);
  const size = part === 'hour' ? 24 : 60;
  const current = part === 'hour' ? hour : minute;
  const next = (current + step % size + size) % size;
  return part === 'hour' ? `${String(next).padStart(2, '0')}:${String(minute).padStart(2, '0')}` : `${String(hour).padStart(2, '0')}:${String(next).padStart(2, '0')}`;
}
