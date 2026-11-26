import assert from 'node:assert/strict';
import { classifyWechatRefresh } from '../src/features/connectors/wechat/refreshIssue.ts';

const readable = { rel: 'session/session.db', status: 'ok', error: null };
const wal = classifyWechatRefresh([
  readable,
  { rel: 'message/message_0.db', status: 'failed', error: 'WAL 第 1 页 HMAC 校验失败' },
]);
assert.equal(wal.title, '部分微信聊天记录未能刷新');
assert.equal(wal.wal, true);
assert.equal(wal.kind, '消息数据库');
assert.equal(wal.message, '最新记录暂时读不到，已保留上一份记录。');
assert.equal(wal.message.includes('登录'), false);
assert.equal(wal.error, 'WAL 第 1 页 HMAC 校验失败');

const unavailable = classifyWechatRefresh([
  { rel: 'session/session.db', status: 'failed', error: '无法读取数据库：没有这个文件' },
]);
assert.equal(unavailable.wal, false);
assert.equal(unavailable.kind, '会话数据库');
assert.equal(unavailable.message, '请确认微信已打开并登录，再重试刷新。');

assert.equal(classifyWechatRefresh([readable, { rel: 'message/message_0.db', status: 'cached', error: null }]), null);

const keys = classifyWechatRefresh([
  { rel: 'message/message_0.db', status: 'no-key', error: '没有该数据库的密钥。请先用密钥工具提取并导入。' },
]);
assert.equal(keys.needsKeys, true);
assert.equal(keys.wal, false);
