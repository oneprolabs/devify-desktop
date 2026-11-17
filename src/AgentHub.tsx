import { useState } from 'react';
import { AgentSettings } from './AgentSettings';
import { AgentAccess } from './AgentAccess';
import { useEntitlements } from './entitlements';

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
  // 工具接入 installs a read-only CLI into other agents; the server only grants it to
  // accounts on the internal plan, so the tab is absent for everyone else.
  const { view } = useEntitlements();
  const internal = view?.internal === true;
  const sections = internal ? SECTIONS : SECTIONS.filter(item => item.id !== 'access');
  const active: Section = sections.some(item => item.id === section) ? section : 'summary';

  return <div className="settings-page agent-hub">
    <div className="hub-head">
      <h2>Agent 集成</h2>
      <p className="help">
        {internal
          ? <>一端把整理好的聊天交给本机 Agent 生成总结，另一端把只读 CLI <code>wxq</code> 与三个 <code>SKILL.md</code> 安装到你正在使用的 Agent。两处配置互不影响，随时可以切换。</>
          : <>把整理好的聊天交给本机 Agent 生成总结。工具接入仅对内部账号开放。</>}
      </p>
    </div>
    <div className="agent-hub-choices" role="tablist" aria-label="Agent 集成分区">
      {sections.map(item => <button
        key={item.id}
        type="button"
        role="tab"
        id={`agent-hub-tab-${item.id}`}
        aria-controls="agent-hub-panel"
        aria-selected={active === item.id}
        className={active === item.id ? 'on' : ''}
        title={item.hint}
        onClick={() => setSection(item.id)}
      >
        <strong>{item.label}</strong><small>{item.hint}</small>
      </button>)}
    </div>
    <div id="agent-hub-panel" role="tabpanel" aria-labelledby={`agent-hub-tab-${active}`}>
      {active === 'summary' ? <AgentSettings /> : <AgentAccess />}
    </div>
  </div>;
}
