import { t } from './locale';
import { useState } from 'react';
import { AgentSettings } from './AgentSettings';
import { AgentAccess } from './AgentAccess';
import { granted, useInternalAccount } from './entitlements';

type Section = 'summary' | 'access';

const SECTIONS: Array<{ id: Section; label: string; hint: string }> = [
  { id: 'summary', label: '本地总结', hint: '用本机 CLI Agent 生成聊天总结' },
  { id: 'access', label: '工具接入', hint: '把只读 CLI 与 Skills 安装到其它 Agent' },
];

/**
 * One settings tab for both sides of agent integration: the local CLI used to
 * summarize chats, and the read-only kit installed into other agents.
 */
export function AgentHub() {
  const [section, setSection] = useState<Section>('summary');
  const { internal, view } = useInternalAccount();
  const sections = SECTIONS.filter(item => internal && granted(view, item.id === 'summary' ? 'ai.local_agent' : 'bridge.agent_cli'));
  const active = sections.find(item => item.id === section)?.id ?? sections[0]?.id;
  if (!active) return null;

  return <div className="settings-page agent-hub">
    <div className="hub-head">
      <h2>{t("Agent 集成")}</h2>
      <p className="help">
        {sections.length === 2
          ? t("配置本机总结 Agent，或向其他 Agent 安装只读工具。")
          : sections[0].hint}
      </p>
    </div>
    <div className="agent-hub-choices" role="tablist" aria-label={t("Agent 集成分区")}>
      {sections.map(item => <button
        key={item.id}
        type="button"
        role="tab"
        id={`agent-hub-tab-${item.id}`}
        aria-controls="agent-hub-panel"
        aria-selected={active === item.id}
        className={active === item.id ? 'on' : ''}
        title={t(item.hint)}
        onClick={() => setSection(item.id)}
      >
        <strong>{t(item.label)}</strong><small>{t(item.hint)}</small>
      </button>)}
    </div>
    <div id="agent-hub-panel" role="tabpanel" aria-labelledby={`agent-hub-tab-${active}`}>
      {active === 'summary' ? <AgentSettings /> : <AgentAccess />}
    </div>
  </div>;
}
