import assert from 'node:assert/strict';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import { ChatEventsPane } from '../src/features/chat/ChatEventsPane.tsx';
import { isViewerEvidence, viewerRelevanceLabel } from '../src/shared/api/eventRelevance.ts';

const events = [
  { id: 1, title: 'Assign quote', status: 'open', viewer_relevance: 'my_todo' },
  { id: 2, title: 'Quote sent', status: 'in_progress', viewer_relevance: 'related' },
  { id: 3, title: 'Another assignment', status: 'open', viewer_relevance: 'other' },
  { id: 4, title: 'Legacy event', status: 'open' },
];

const html = renderToStaticMarkup(React.createElement(ChatEventsPane, {
  active: true, loading: false, error: '', events, sessionCount: 1, query: '',
  filter: 'all', onRetry() {}, onSchedule() {}, onOpenEvidence() {},
}));
for (const label of ['我的待办', '与我有关', '其他', '待跟进', '推进中']) {
  assert.ok(html.includes(label), label);
}
assert.equal((html.match(/我的待办/g) ?? []).length, 1);
assert.ok(!html.includes('待我处理'));
assert.equal(viewerRelevanceLabel(events[3]), '');
assert.equal(isViewerEvidence({ viewer_evidence_message_ids: [8] }, 8), true);
assert.equal(isViewerEvidence({ viewer_evidence_message_ids: [8] }, 9), false);
assert.equal(isViewerEvidence({}, 8), false);
console.log('event viewer labels and source matching passed');
