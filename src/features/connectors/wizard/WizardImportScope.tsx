import { t, tf } from '../../../shared/i18n/locale';
import { Icons } from '../../../shared/ui/icons';
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
  const source = p.sourceName ?? t("微信");
  const privateOn = direct.length > 0 && privateCount === direct.length;
  const groupOn = groups.length > 0 && groupCount === groups.length;
  const openCustom = () => { setCustom(true); p.onClearIssue(); };
  return <div className="wizard-import-scope">
    <div className="wizard-scope-row">
      <div className="wizard-scope-label"><span>{t("导入范围")}</span><small>{!p.loading && p.catalog.length > 1000 && !p.selectionIssue ? t("每个账号最多导入 1000 个会话，请通过「自定义」筛选和选择。") : t("私聊和群聊可同时选择。选「自定义」可批量设置或逐个设置。")}</small></div>
      <div className="wizard-scope-pills" role="group" aria-label={t("导入范围")}>
        <button type="button" aria-pressed={privateOn} disabled={p.disabled || p.loading || !direct.length} onClick={() => { p.onScope(false, !privateOn); setCustom(false); }}>{t("私聊")}</button>
        <button type="button" aria-pressed={groupOn} disabled={p.disabled || p.loading || !groups.length} onClick={() => { p.onScope(true, !groupOn); setCustom(false); }}>{t("群聊")}</button>
        <button type="button" aria-pressed={custom} aria-expanded={custom} disabled={p.disabled || p.loading} onClick={() => { if (custom) setCustom(false); else openCustom(); }}>{t("自定义")}</button>
      </div>
    </div>
    {p.selectionIssue && <div className="wizard-scope-warning" role="alert">
      <div><strong>{t("本次选择未生效")}</strong><p>{tf("最多可导入 1000 个会话，本次选择了 {0} 个。已保留原来的 {1} 个会话。", p.selectionIssue.attempted, privateCount + groupCount)}</p></div>
      <button type="button" disabled={p.disabled || p.loading} onClick={openCustom}>{t("自定义选择")}</button>
    </div>}
      <div className="wizard-scope-row"><span>{t("预计导入")}</span><span className="wizard-scope-value" role="status">{p.loading ? t("正在读取会话…") : tf("{0} 个会话（私聊 {1} · 群聊 {2}）", privateCount + groupCount, privateCount, groupCount)}</span></div>
      <div className="wizard-scope-row">
        <div className="wizard-scope-label"><span>{t("新增的私聊和群聊")}</span><small>{t("新会话需在「导入管理」中手动选择。")}</small></div>
        <div className="wizard-scope-pills" role="group" aria-label={t("新增会话的导入方式")}>
          <button type="button" disabled title={t("暂未支持新增会话自动导入")}>{t("自动导入")}</button>
          <button type="button" disabled title={t("暂未支持新增会话提醒")}>{t("询问我")}</button>
          <button type="button" aria-pressed="true" disabled={p.disabled}>{t("忽略")}</button>
        </div>
      </div>
      <div className="wizard-scope-row">
        <div className="wizard-scope-label"><span>{t("会话读取状态")}</span><small>{p.loading ? tf("正在读取当前 {0} 账号的会话。", source) : p.catalog.length ? tf("已读取 {0} 个会话，保存后按所选范围导入。", p.catalog.length) : tf("未读取到会话，请返回连接 {0}。", source)}</small></div>
        <span className="wizard-scope-value wizard-read-status" data-completed={!p.loading && p.catalog.length > 0}>{!p.loading && p.catalog.length > 0 && <span className="wizard-status-check" aria-hidden="true">{Icons.check}</span>}{!p.loading && p.catalog.length > 0 ? t("已完成") : t("未完成")}</span>
      </div>
    {custom && <div className="wizard-scope-custom">{p.children}</div>}
  </div>;
}
