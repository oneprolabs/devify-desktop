import { useEffect, useState, type ReactNode } from 'react';

type Session = { username: string; is_group: boolean };
type Props = {
  catalog: Session[]; selected: Set<string>; loading: boolean; disabled: boolean; accountId: string;
  onScope: (group: boolean, select: boolean) => void; children: ReactNode;
  selectionIssue: { attempted: number } | null; onClearIssue: () => void;
  sourceName?: string;
};

/** Scope choices operate on the same explicit allowlist as the custom picker. */
export function WizardImportScope(p: Props) {
  const [custom, setCustom] = useState(false);
  useEffect(() => { setCustom(false); }, [p.accountId]);
  const direct = p.catalog.filter(item => !item.is_group);
  const groups = p.catalog.filter(item => item.is_group);
  const privateCount = direct.filter(item => p.selected.has(item.username)).length;
  const groupCount = groups.filter(item => p.selected.has(item.username)).length;
  const privateOn = direct.length > 0 && privateCount === direct.length;
  const groupOn = groups.length > 0 && groupCount === groups.length;
  const openCustom = () => { setCustom(true); p.onClearIssue(); };
  return <div className="wizard-import-scope">
    <div className="wizard-scope-row">
      <div className="wizard-scope-label"><span>导入范围</span><small>{!p.loading && p.catalog.length > 1000 && !p.selectionIssue ? '每个账号最多导入 1000 个会话，请通过「自定义」筛选和选择。' : '私聊和群聊可同时选择。选「自定义」可批量设置或逐个设置。'}</small></div>
      <div className="wizard-scope-pills" role="group" aria-label="导入范围">
        <button type="button" aria-pressed={privateOn} disabled={p.disabled || p.loading || !direct.length} onClick={() => { p.onScope(false, !privateOn); setCustom(false); }}>私聊</button>
        <button type="button" aria-pressed={groupOn} disabled={p.disabled || p.loading || !groups.length} onClick={() => { p.onScope(true, !groupOn); setCustom(false); }}>群聊</button>
        <button type="button" aria-pressed={custom} aria-expanded={custom} disabled={p.disabled || p.loading} onClick={() => { if (custom) setCustom(false); else openCustom(); }}>自定义</button>
      </div>
    </div>
    {p.selectionIssue && <div className="wizard-scope-warning" role="alert">
      <div><strong>本次选择未生效</strong><p>最多可导入 1000 个会话，本次选择了 {p.selectionIssue.attempted} 个。已保留原来的 {privateCount + groupCount} 个会话。</p></div>
      <button type="button" disabled={p.disabled || p.loading} onClick={openCustom}>自定义选择</button>
    </div>}
      <div className="wizard-scope-row"><span>预计导入</span><span className="wizard-scope-value" role="status">{p.loading ? '正在读取会话…' : `${privateCount + groupCount} 个会话（私聊 ${privateCount} · 群聊 ${groupCount}）`}</span></div>
      <div className="wizard-scope-row">
        <div className="wizard-scope-label"><span>新增的私聊和群聊</span><small>新会话需在「导入管理」中手动选择。</small></div>
        <div className="wizard-scope-pills" role="group" aria-label="新增会话的导入方式">
          <button type="button" disabled title="暂未支持新增会话自动导入">自动导入</button>
          <button type="button" disabled title="暂未支持新增会话提醒">询问我</button>
          <button type="button" aria-pressed="true" disabled={p.disabled}>忽略</button>
        </div>
      </div>
      <div className="wizard-scope-row">
        <div className="wizard-scope-label"><span>会话读取状态</span><small>{p.loading ? `正在读取当前${p.sourceName ?? '微信'}账号的会话。` : p.catalog.length ? `已读取 ${p.catalog.length} 个会话；保存后按所选范围导入。` : `未读取到会话，请返回连接${p.sourceName ?? '微信'}。`}</small></div>
        <span className="wizard-scope-value">{p.loading ? '检查中' : p.catalog.length ? '已读取' : '暂无会话'}</span>
      </div>
    {custom && <div className="wizard-scope-custom">{p.children}</div>}
  </div>;
}
