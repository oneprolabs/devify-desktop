import { dateLocale, t, tf } from '../../../shared/i18n/locale';
import { summaryOutputLanguage, type SummaryConfigError, type SummaryDraft, type SummaryOptionsView } from './summaryOptions';

export function SummaryRequestOptions({ view, draft, loading, error, disabled, onChange, onReload }: {
  view: SummaryOptionsView | null; draft: SummaryDraft; loading: boolean; error: SummaryConfigError | null; disabled: boolean;
  onChange: (patch: Partial<SummaryDraft>) => void; onReload: () => void;
}) {
  const selected = view?.config.scenarios.find(item => item.id === draft.scenario);
  const count = Array.from(draft.instructions).length;
  return <div className="summary-request-options">
    {!view && loading && <div className="summary-feedback" role="status">{t('正在读取总结方式…')}</div>}
    {view && <>
      <div className="chat-summary-row"><span className="chat-summary-label">{t('总结方式')}</span><div className="summary-option-field">
        <div className="chat-summary-pills summary-scenario-pills" role="group" aria-label={t('总结方式')}>{view.config.scenarios.map(item => <button key={item.id} type="button" className={draft.scenario === item.id ? 'on' : ''} aria-pressed={draft.scenario === item.id} disabled={disabled || loading} onClick={() => onChange({ scenario: item.id })}>{t(item.name)}</button>)}</div>
        <p className="chat-summary-note">{selected ? t(selected.description) : t('请选择总结方式。')}</p>
      </div></div>
      <div className="chat-summary-row"><label className="chat-summary-label" htmlFor="summary-output-language">{t('输出语言')}</label>
        <select id="summary-output-language" className="summary-output-language" value={summaryOutputLanguage(view, draft)} disabled={disabled || loading} onChange={event => onChange({ language: event.target.value })}>
          <option value="中文">{t('中文')}</option><option value="English">English</option><option value="按原文语言">{t('原文语言')}</option>
        </select></div>
      <div className="chat-summary-row summary-instructions-row"><label className="chat-summary-label" htmlFor="summary-user-instructions">{t('我的要求')}</label><div className="summary-option-field">
        <textarea id="summary-user-instructions" rows={3} value={draft.instructions} disabled={disabled || loading} aria-invalid={count > view.config.max_instructions_chars || undefined} aria-describedby="summary-instructions-hint" onChange={event => onChange({ instructions: event.target.value })}
          placeholder={t('例如：重点关注项目结论、负责人和截止时间。')}/>
        <div className="summary-instructions-hint" id="summary-instructions-hint"><span>{t(selected?.requires_instructions || selected?.id === 'custom' ? '自定义总结需填写目标、关注点或输出结构。' : '可选，仅用于本次总结。待办以文字展示，不自动创建任务。')}</span><span className={count > view.config.max_instructions_chars ? 'over-limit' : ''}>{count} / {view.config.max_instructions_chars}</span></div>
      </div></div>
    </>}
    {error && <div className="summary-feedback error" role="alert"><span>{t(error.message)}
      {error.retry_at != null && error.retry_at * 1000 > Date.now() && ` ${tf('可在 {0} 后重试。', new Date(error.retry_at * 1000).toLocaleString(dateLocale()))}`}
    </span><button type="button" disabled={loading || disabled} onClick={onReload}>{t('重试')}</button></div>}
  </div>;
}
