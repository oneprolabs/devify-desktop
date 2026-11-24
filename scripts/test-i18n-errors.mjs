import assert from 'node:assert/strict';
import { applyLanguage, tf } from '../src/shared/i18n/locale.tsx';
import { nativeErrorCode, errorText, whatsAppReadErrorCode } from '../src/shared/i18n/errorText.ts';

const unavailable = '读取服务端登录配置失败：服务端暂不可用，请稍后重试。';
const busy = '该账号正在上传，请稍后重试。';
const missing = '未找到解密后的 session.db，请先完成解密。';
for (const ui of ['中文', 'English']) {
  applyLanguage({ ui });
  assert.equal(nativeErrorCode(unavailable), 'AUTH_CONFIG_UNAVAILABLE');
  assert.equal(nativeErrorCode('AUTH_CONFIG_INVALID: 服务端登录配置响应无效，请检查服务地址并稍后重试。'), 'AUTH_CONFIG_INVALID');
  assert.equal(nativeErrorCode(busy), 'ACCOUNT_UPLOAD_BUSY');
  assert.equal(nativeErrorCode(missing), 'WECHAT_SESSION_DB_MISSING');
  assert.equal(nativeErrorCode('unexpected failure'), undefined);
  assert.equal(nativeErrorCode('AUTH_UPLOAD_BUSY: 上传期间无法更改登录状态，请稍后重试。'), 'AUTH_UPLOAD_BUSY');
  assert.equal(whatsAppReadErrorCode('WHATSAPP_DATA_UNSUPPORTED: 无法读取'), 'WHATSAPP_DATA_UNSUPPORTED');
  assert.equal(whatsAppReadErrorCode('Operation not permitted (os error 1)'), 'WHATSAPP_ACCESS_DENIED');
}
assert.equal(errorText('ACCOUNT_UPLOAD_BUSY: ' + busy), 'This account is uploading. Try again in a moment.');
assert.match(errorText('读取服务端登录配置失败：503 Service Unavailable'), /503 Service Unavailable/);
assert.doesNotMatch(errorText('无法加载设置：服务端暂不可用，请稍后重试。'), /\p{Script=Han}/u);
assert.doesNotMatch(errorText('Error: 未知的客户端诊断'), /\p{Script=Han}/u);
assert.equal(tf("导入 {0}", '项目讨论群'), 'Import 项目讨论群');

applyLanguage({ ui: '中文' });
assert.equal(errorText('ACCOUNT_UPLOAD_BUSY: ' + busy), busy);
assert.equal(errorText('读取服务端登录配置失败：503 Service Unavailable'), '读取服务端登录配置失败：503 Service Unavailable');
console.log('Native error codes, localized messages and unchanged user text passed');
