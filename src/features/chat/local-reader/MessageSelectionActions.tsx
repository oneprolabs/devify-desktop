import { t, tf } from '../../../shared/i18n/locale';

export function MessageSelectionActions({ count, onSummarize, onCancel }: {
  count: number;
  onSummarize: () => void;
  onCancel: () => void;
}) {
  return <div className="wxv-selection-actions" role="group" aria-label={t('选中消息操作')}>
    <span aria-live="polite">{tf('已选 {0} 条消息', count)}</span>
    <div>
      <button type="button" onClick={onCancel}>{t('取消选择')}</button>
      <button type="button" className="primary" onClick={onSummarize}>{tf('总结所选 {0} 条', count)}</button>
    </div>
  </div>;
}
