import { t } from '../i18n/locale';

export type ViewerEventRelevance = {
  viewer_relevance?: 'my_todo' | 'related' | 'other';
  viewer_evidence_message_ids?: number[];
};

export type RelevanceCounts = { my_todo: number; related: number; other: number };
export function readRelevanceCounts(value: unknown): RelevanceCounts | null {
  if (!value || typeof value !== 'object') return null;
  const counts = value as RelevanceCounts;
  return [counts.my_todo, counts.related, counts.other].every(count => Number.isSafeInteger(count) && count >= 0) ? counts : null;
}

export function viewerRelevanceLabel(event: ViewerEventRelevance): string {
  switch (event.viewer_relevance) {
    case 'my_todo': return t('我的待办');
    case 'related': return t('与我有关');
    case 'other': return t('其他');
    default: return '';
  }
}

export function isViewerEvidence(event: ViewerEventRelevance, messageId?: number): boolean {
  return messageId !== undefined && event.viewer_evidence_message_ids?.includes(messageId) === true;
}
