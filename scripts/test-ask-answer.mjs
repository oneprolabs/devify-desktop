import assert from 'node:assert/strict';
import { formatAskAnswer } from '../src/askAnswer.ts';

const ORDERED_LINE = /^(\s*)(\d{1,3})[.)]\s+(.*)$/;

const blob = '今天的聊天大致可分为： 1. 客户端登录授权仍停留在旧页面。 2. 模型调用返回 503。';
assert.equal(
  formatAskAnswer(blob),
  '今天的聊天大致可分为：\n\n1. 客户端登录授权仍停留在旧页面。\n2. 模型调用返回 503。',
);

const circled = '若按被记录成事件的事项来算，是 3 件：①提醒会员明天到期；②说明工具是配置问题；③提出要借会员卡。';
assert.equal(
  formatAskAnswer(circled),
  '若按被记录成事件的事项来算，是 3 件：\n\n1. 提醒会员明天到期；\n2. 说明工具是配置问题；\n3. 提出要借会员卡。',
);

assert.equal(
  formatAskAnswer('今天大致可分为：\n\n1. 客户端\n2. 模型'),
  '今天大致可分为：\n\n1. 客户端\n2. 模型',
);

assert.equal(formatAskAnswer('王欣要求周五前交付。'), '王欣要求周五前交付。');
assert.equal(formatAskAnswer('更新到 0.12 后再看 1.6.11。'), '更新到 0.12 后再看 1.6.11。');

const realistic = '按本次筛选记录（2026-11-18），今天的聊天大致可分为： 1. 客户端登录仍停留在旧页面，0.12 已修复默认地址，并让 Carlton 先更新 0.1.9、再更新 0.1.11。 2. 模型调用返回 503，几小时后会再验证。';
const formatted = formatAskAnswer(realistic);
assert.match(formatted, /0\.12 已修复/);
assert.match(formatted, /0\.1\.9/);
assert.match(formatted, /0\.1\.11/);
const lines = formatted.split('\n').filter(line => line.trim());
assert.equal(ORDERED_LINE.test(lines[0]), false);
assert.deepEqual(lines.slice(1).map(line => ORDERED_LINE.test(line)), [true, true]);
assert.equal(ORDERED_LINE.test(formatAskAnswer('王欣要求周五前交付。')), false);

const sections = formatAskAnswer(
  '客户端问题可以分成几块：\n一、登录与授权 - 浏览器授权点不开 - 忘记密码跳转 404\n二、性能与加载 - 打开授权页大约 10-15 秒',
);
assert.equal(
  sections,
  '客户端问题可以分成几块：\n\n## 登录与授权\n\n- 浏览器授权点不开\n- 忘记密码跳转 404\n\n## 性能与加载\n\n- 打开授权页大约 10-15 秒',
);

console.log('ask answer format checks passed');
