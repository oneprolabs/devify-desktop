import codex from '../../assets/agents/codex.png';
import claude from '../../assets/agents/claude.ico';
import gemini from '../../assets/agents/gemini.png';
import workbuddy from '../../assets/agents/workbuddy.svg';
import deepseek from '../../assets/agents/deepseek.png';
import opencode from '../../assets/agents/opencode.png';
import cursor from '../../assets/agents/cursor.svg';
import qwen from '../../assets/agents/qwen.png';
import type { AppSettings } from '../settings/settingsModel';

export type AgentProfile = { command: string; model: string; timeout_seconds: number; skills_dir: string };
export type AgentInfo = { id: string; name: string; command: string | null; version: string | null; available: boolean; note: string };
export type AccessTarget = { id: string; name: string; available: boolean; command: string | null; desktop_app?: string | null; config_dir?: string | null; skills_dir: string; installed: string[] };
export type AccessStatus = { running: boolean; reason: string | null; wxq: unknown | null; targets: AccessTarget[]; skills: { name: string }[] };
export const AGENTS = [
  { id: 'codex', name: 'Codex CLI', icon: codex, callable: true },
  { id: 'claude-code', name: 'Claude Code CLI', icon: claude, callable: true },
  { id: 'gemini-cli', name: 'Gemini CLI', icon: gemini, callable: true },
  { id: 'workbuddy', name: 'WorkBuddy', icon: workbuddy, callable: false },
  { id: 'deepseek-harness', name: 'DeepSeek Harness', icon: deepseek, callable: false },
  { id: 'opencode', name: 'OpenCode CLI', icon: opencode, callable: true },
  { id: 'cursor-agent', name: 'Cursor CLI', icon: cursor, callable: true },
  { id: 'qwen-code', name: 'Qwen Code', icon: qwen, callable: true },
] as const;
// Keep this short catalog separate from the CLI invocation adapters.
export const ACCESS_AGENTS = ['codex', 'claude-code', 'opencode', 'workbuddy'].map(id => AGENTS.find(agent => agent.id === id)!);
export const SUMMARY_AGENTS = AGENTS.filter(agent => agent.callable);
export const agentById = (id: string) => AGENTS.find(agent => agent.id === id);
export function agentInvocationNotice(id: string): string {
  if (id === 'workbuddy') return '此处安装 Skills，直接总结仅支持上方已适配的 CLI。';
  if (id === 'deepseek-harness') return 'CLI 总结暂未适配，仅支持 Skills 接入';
  return '';
}
export function profileFor(agent: AppSettings['agent'], id: string): AgentProfile {
  return agent.profiles?.[id] ?? (agent.provider_id === id
    ? { command: agent.command, model: agent.model, timeout_seconds: agent.timeout_seconds, skills_dir: '' }
    : { command: '', model: '', timeout_seconds: 180, skills_dir: '' });
}
export function selectAgent(agent: AppSettings['agent'], id: string): AppSettings['agent'] {
  const profiles = { ...agent.profiles };
  if (agent.provider_id && !profiles[agent.provider_id]) profiles[agent.provider_id] = profileFor(agent, agent.provider_id);
  const selected = id ? profileFor(agent, id) : { command: '', model: '', timeout_seconds: 180 };
  return { ...agent, enabled: localSummaryEnabled(agent), profiles, provider_id: id,
    command: selected.command, model: selected.model, timeout_seconds: selected.timeout_seconds };

}
export function effectiveAgent(agent: AppSettings['agent'], detected: AgentInfo[]): string | null {
  if (agent.provider_id) return SUMMARY_AGENTS.some(item => item.id === agent.provider_id) && detected.some(item => item.id === agent.provider_id && item.available) ? agent.provider_id : null;
  return SUMMARY_AGENTS.find(item => detected.some(found => found.id === item.id && found.available))?.id ?? null;
}
export const localSummaryEnabled = (agent: AppSettings['agent']) => agent.enabled ?? Boolean(agent.provider_id);

// A missing executable still needs its own path editor, rather than another
// detected provider's profile. Skills-only legacy choices cannot drive summaries.
export function summaryConfigurationAgent(agent: AppSettings['agent'], detected: AgentInfo[]): string {
  if (SUMMARY_AGENTS.some(item => item.id === agent.provider_id)) return agent.provider_id;
  return effectiveAgent({ ...agent, provider_id: '' }, detected) ?? 'codex';
}
