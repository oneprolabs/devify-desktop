import assert from 'node:assert/strict';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { WhatsAppAttachment } from '../src/features/chat/local-reader/WhatsAppAttachment.tsx';

const message = { accountId: '123456789@s.whatsapp.net', username: 'fixture@g.us', localId: 42, time: 100 };
const file = { kind: 'file', name: 'Report.pdf', size: 2048, file_type: 'pdf', can_open: true };
const render = (attachment) => renderToStaticMarkup(React.createElement(WhatsAppAttachment, {
  message, attachment, onImageOpen() {},
}));

const available = render({ ...file, status: 'available' });
assert.match(available, /<details[^>]*><summary[^>]*>.*Report\.pdf.*<\/summary>/);
assert.doesNotMatch(available.match(/<summary[^>]*>(.*?)<\/summary>/)?.[1] ?? '', /(?:PDF|KB|打开|保存文件)/);
assert.match(available, /<\/summary>.*PDF.*2(?:\.0)?\s?KB/);
assert.match(available, />打开<\/button>/);
assert.match(available, />保存文件<\/button>/);
for (const [status, label] of [
  ['unavailable', '附件在本机不可用'],
  ['denied', '没有权限读取此附件'],
  ['unsupported', '此 WhatsApp 版本的附件暂不支持'],
]) {
  const html = render({ ...file, status });
  assert.ok(html.includes(label));
  assert.doesNotMatch(html, /<button|<details|2 KB/);
}
assert.doesNotMatch(render({ ...file, status: 'available', can_open: false }), />打开<\/button>/);
assert.match(render({ kind: 'image', status: 'thumbnail_only' }), /正在加载图片/);
assert.match(render({ kind: 'video', status: 'available', has_thumbnail: true, can_open: true }), /class="wxv-attach video"/);
assert.match(render({ kind: 'video', status: 'available', has_thumbnail: false, can_open: true }), />视频<\/span>.*>打开<\/button>/);
console.log('WhatsApp attachments render metadata, file actions, and unavailable states.');
