import { useState } from 'react';
import { AgentSettings } from './AgentSettings';
import { AgentAccess } from './AgentAccess';

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

  return <div className="settings-page agent-hub">
    <div className="hub-head">
      <h2>Agent 集成</h2>
      <p className="help">
        一端把整理好的聊天交给本机 Agent 生成总结，另一端把只读 CLI <code>wxq</code> 与三个
        <code>SKILL.md</code> 安装到你正在使用的 Agent。两处配置互不影响，随时可以切换。
      </p>
    </div>
    <div className="tabs" role="tablist" aria-label="Agent 集成分区">
      {SECTIONS.map(item => <button
        key={item.id}
        type="button"
        role="tab"
        id={`agent-hub-tab-${item.id}`}
        aria-controls="agent-hub-panel"
        aria-selected={section === item.id}
        className={section === item.id ? 'on' : ''}
        title={item.hint}
        onClick={() => setSection(item.id)}
      >
        {item.label}
      </button>)}
    </div>
    <div id="agent-hub-panel" role="tabpanel" aria-labelledby={`agent-hub-tab-${section}`}>
      {section === 'summary' ? <AgentSettings /> : <AgentAccess />}
    </div>
  </div>;
}
