import assert from 'node:assert/strict';
import {
  analysisSettingsError,
  questionAnswerError,
} from '../src/analysisSettingsText.ts';

assert.equal(analysisSettingsError('当前 Devify 服务端不可用'), '当前服务端不可用');
assert.equal(analysisSettingsError('Devify 请求失败：无权访问'), '服务端请求失败：无权访问');
assert.equal(analysisSettingsError('DEVIFY 请求失败：HTTP 403'), '服务端请求失败：HTTP 403');
assert.equal(analysisSettingsError('访问 Devify 失败：连接超时'), '访问服务端失败：连接超时');
assert.equal(analysisSettingsError(new Error('devify_analysis_preferences_get 失败')), '服务端请求失败');

// Unbranded details, including user-provided addresses and identifiers, remain exact.
for (const value of [
  '访问 https://service.example.com/api 失败',
  '邮箱 alice@example.com 无权访问',
  '用户名 Alice_123 无权访问',
  '请先选择空间',
]) assert.equal(analysisSettingsError(value), value);

// Branded dynamic data must never be turned into a different URL or username.
// A generic message is preferable to exposing the old name or fabricating values.
for (const value of [
  '访问 https://devify.example.com/api 失败',
  '访问 HTTPS://DEVIFY.example.com/a 失败',
  'Devify 请求失败：无法连接 https://mydevify.example.com/api',
  '邮箱 devify.user@example.com 无权访问',
  '邮箱 USER@DEVIFY.example 无权访问',
  '用户名 myDevifyUser 无权访问',
  '用户名 DEVIFY 无权访问',
  'https://devify.example.com。',
]) {
  assert.equal(analysisSettingsError(value), '服务端请求失败，请检查设置或网络。');
}
assert.equal(questionAnswerError(''), '回答失败，请重新提问。');
assert.equal(
  questionAnswerError(
    'litellm.BadRequestError: OpenAIException - Invalid request parameters.',
  ),
  '回答生成失败，请稍后重试。',
);
assert.equal(
  questionAnswerError('The answer model request failed. Please try again later.'),
  '回答生成失败，请稍后重试。',
);
assert.equal(questionAnswerError('请先选择空间'), '请先选择空间');
console.log('analysis settings text checks passed');
