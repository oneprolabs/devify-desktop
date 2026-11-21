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
export type AccessTarget = { id: string; name: string; available: boolean; command: string | null; skills_dir: string; installed: string[] };
export type AccessStatus = { running: boolean; reason: string | null; wxq: unknown | null; targets: AccessTarget[]; skills: { name: string }[] };
export const AGENTS = [
  { id: 'codex', name: 'OpenAI Codex', icon: codex, callable: true },
  { id: 'claude-code', name: 'Claude Code', icon: claude, callable: true },
  { id: 'gemini-cli', name: 'Gemini CLI', icon: gemini, callable: true },
  { id: 'workbuddy', name: 'WorkBuddy', icon: workbuddy, callable: false },
  { id: 'deepseek-harness', name: 'DeepSeek Harness', icon: deepseek, callable: false },
  { id: 'opencode', name: 'OpenCode', icon: opencode, callable: true },
  { id: 'cursor-agent', name: 'Cursor CLI', icon: cursor, callable: true },
  { id: 'qwen-code', name: 'Qwen Code', icon: qwen, callable: true },
] as const;
export const agentById = (id: string) => AGENTS.find(agent => agent.id === id);
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
  if (agent.provider_id) return detected.some(item => item.id === agent.provider_id && item.available) ? agent.provider_id : null;
  return AGENTS.find(item => item.callable && detected.some(found => found.id === item.id && found.available))?.id ?? null;
}
export const localSummaryEnabled = (agent: AppSettings['agent']) => agent.enabled ?? Boolean(agent.provider_id);
