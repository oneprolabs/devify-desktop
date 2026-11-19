import assert from 'node:assert/strict';

// Login UI checks use the production gate and isolated IPC, never personal credentials.
export async function runLoginUiE2E(task, base = 'http://localhost:1420', pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const click = name => page.click(`loc=role:button[name="${name}"]`);
  const start = async (query = '') => {
    await page.goto(`${base}/scripts/connectors-e2e.html?anonymous${query}`);
    await page.waitForSelector('.auth-login-gate .auth-gate-primary:not(:disabled)');
    await page.snapshot();
  };
  const clean = async () => assert.deepEqual(await page.evaluate(() => window.__E2E__.errors), []);
  const fits = async () => {
    assert.ok(await page.evaluate(() => {
      const box = document.querySelector('.auth-login-modal').getBoundingClientRect();
      const form = document.querySelector('.auth-login-modal form');
      return box.top >= 0 && box.bottom <= innerHeight && box.left >= 0 && box.right <= innerWidth && form.scrollWidth <= form.clientWidth;
    }), 'login fits the viewport without horizontal overflow');
  };
  await page.cdp('Emulation.setDeviceMetricsOverride', {width:1100,height:690,deviceScaleFactor:1,mobile:false});
  await start();
  await click('登录…');
  await fits();
  assert.equal(await page.evaluate(() => document.querySelector('.auth-login-modal').getBoundingClientRect().width), 420);
  assert.ok(await page.evaluate(() => [...document.querySelectorAll('.auth-input-row input')].every(input => {
    const style = getComputedStyle(input);
    return style.borderTopWidth === '0px' && style.boxShadow === 'none' && input.getBoundingClientRect().height === 42;
  })), 'global native input styling cannot add a second border or green focus ring');
  await page.screenshot({path:'/tmp/aimychats-login-v7-desktop.png'});
  await page.press('input[name="username"]', 'Enter');
  await page.waitForFunction(() => document.querySelector('.auth-error')?.textContent.includes('请输入用户名'));
  assert.equal(await page.evaluate(() => document.querySelectorAll('[role="alert"]').length), 1, 'an error appears only once');
  await page.fill('input[name="username"]', 'tester');
  await page.fill('input[name="password"]', 'example-password');
  await click('显示');
  assert.equal(await page.evaluate(() => document.querySelector('input[name="password"]').type), 'text');
  await click('隐藏');
  await click('忘记密码？');
  await page.waitForFunction(() => window.__E2E__.externalUrls.some(url => url.endsWith('/login?forgot=1')));
  await page.click('loc=role:tab[name="注册"]');
  await click('在浏览器中注册');
  await page.waitForFunction(() => window.__E2E__.externalUrls.some(url => url.endsWith('/register')));
  await page.screenshot({path:'/tmp/aimychats-login-v7-register.png'});
  await page.click('loc=role:tab[name="登录"]');
  await page.evaluate(() => { window.__E2E__.authStartPaused = true; });
  await click('通过浏览器登录');
  await page.waitForFunction(() => window.__E2E__.authStartResolvers.length === 1);
  assert.ok(await page.evaluate(() => document.querySelector('.auth-alt').disabled && document.querySelector('.auth-alt .auth-button-spinner') && document.querySelector('.auth-alt').textContent.includes('正在打开浏览器')));
  await page.evaluate(() => { window.__E2E__.authStartPaused = false; window.__E2E__.authStartResolvers.splice(0).forEach(resolve => resolve()); });
  await page.waitForSelector('.auth-waiting');
  assert.ok(await page.evaluate(() => [...document.querySelectorAll('.auth-login-modal input,.auth-tabs button,.auth-submit')].every(control => control.disabled)), 'browser wait locks competing login actions');
  await page.evaluate(() => document.activeElement?.blur());
  await page.keyboard.press('Tab');
  assert.ok(await page.evaluate(() => document.querySelector('.auth-login-modal').contains(document.activeElement)), 'pending authorization keeps keyboard focus in the modal');
  await click('取消');
  await page.waitForFunction(() => !document.querySelector('.auth-waiting'));
  await page.evaluate(() => { window.__E2E__.browserError = '浏览器暂时无法打开，请重试。'; });
  await click('通过浏览器登录');
  await page.waitForFunction(() => document.querySelector('.auth-error')?.textContent.includes('浏览器暂时无法打开'));
  await page.evaluate(() => { window.__E2E__.browserError = ''; });
  await page.evaluate(() => { window.__E2E__.authStartError = '上传期间无法更改登录状态，请稍后重试。'; });
  const urlsBeforeBlockedLogin = await page.evaluate(() => window.__E2E__.externalUrls.length);
  await click('通过浏览器登录');
  await page.waitForFunction(() => document.querySelector('.auth-error')?.textContent.includes('退出并重新打开 AIMyChats'));
  assert.equal(await page.evaluate(() => window.__E2E__.externalUrls.length), urlsBeforeBlockedLogin, 'a blocked login does not claim to have opened the browser');
  assert.ok(await page.evaluate(() => !document.querySelector('.auth-waiting') && !document.querySelector('.auth-alt').disabled));
  await page.evaluate(() => { window.__E2E__.authStartError = ''; });
  await page.focus('.auth-modal-head button');
  await page.keyboard.press('Shift+Tab');
  assert.ok(await page.evaluate(() => document.activeElement === document.querySelector('.auth-switch button')), 'focus wraps within the dialog');
  await page.keyboard.press('Tab');
  assert.ok(await page.evaluate(() => document.activeElement === document.querySelector('.auth-modal-head button')));
  await page.keyboard.press('Escape');
  assert.ok(await page.evaluate(() => !document.querySelector('.auth-login-modal') && document.activeElement === document.querySelector('.auth-gate-primary')), 'closing restores focus to the login entry');
  await click('登录…');
  await page.evaluate(() => { window.__E2E__.loginPaused = true; window.__E2E__.loginError = '用户名或密码不正确，请重新输入。'; });
  await page.press('input[name="password"]', 'Enter');
  await page.waitForSelector('.auth-button-spinner');
  await page.waitForFunction(() => window.__E2E__.loginResolvers.length === 1);
  assert.ok(await page.evaluate(() => document.querySelector('.auth-submit').disabled && document.querySelector('.auth-submit').getAttribute('aria-busy') === 'true'));
  await page.evaluate(() => { window.__E2E__.loginPaused = false; window.__E2E__.loginResolvers.splice(0).forEach(resolve => resolve()); });
  await page.waitForFunction(() => document.querySelector('.auth-error')?.textContent.includes('用户名或密码不正确'));
  await page.cdp('Emulation.setDeviceMetricsOverride', {width:940,height:640,deviceScaleFactor:1,mobile:false});
  await fits();
  await page.screenshot({path:'/tmp/aimychats-login-v7-small-error.png'});
  await page.evaluate(() => { window.__E2E__.loginError = ''; });
  await page.click('.auth-submit');
  await page.waitForFunction(() => !document.querySelector('.auth-gate'));
  await clean();

  await start('&no-server');
  await page.fill('.auth-gate-server input', 'not-a-url');
  await click('保存');
  await page.waitForFunction(() => document.querySelector('.auth-gate [role="status"]')?.textContent.includes('完整'));
  assert.equal(await page.evaluate(() => window.__E2E__.writes.length), 0);
  await page.fill('.auth-gate-server input', 'https://example.invalid');
  await click('保存');
  await page.waitForFunction(() => window.__E2E__.writes.length === 1);
  await click('登录…');
  await fits();
  await clean();

  await start('&english');
  await page.click('.auth-gate-primary');
  await fits();
  assert.equal(await page.evaluate(() => document.querySelector('.auth-alt-help').textContent), 'Finish signing in in your browser, then return to the app.');
  assert.equal(await page.evaluate(() => document.querySelector('.auth-divider').textContent), 'or');
  await page.screenshot({path:'/tmp/aimychats-login-v7-english-small.png'});
  await clean();
  console.log('PASS V7 login UI: password validation/loading/retry, browser wait/cancel/error, register/recovery links, keyboard focus, server setup, English and 940×640 layout');
  console.log(await page.snapshot());
}

export async function runLoginRecoveryE2E(task, base = 'http://localhost:1420', pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const click = name => page.click(`loc=role:button[name="${name}"]`);
  await page.goto(`${base}/scripts/connectors-e2e.html?auth-unavailable`);
  await page.waitForFunction(() => document.querySelector('.auth-gate-primary')?.textContent === '重试');
  await page.snapshot();
  await page.evaluate(() => {
    window.__loginGate = document.querySelector('.auth-gate');
    window.__E2E__.authPaused = true;
  });
  await click('重试');
  await page.waitForFunction(() => window.__E2E__.authResolvers.length === 1);
  await page.evaluate(() => {
    window.dispatchEvent(new Event('online'));
    window.dispatchEvent(new Event('online'));
  });
  assert.deepEqual(await page.evaluate(() => ({
    sameGate: window.__loginGate === document.querySelector('.auth-gate'),
    fullPageLoading: !!document.querySelector('.auth-checking'),
    busy: document.querySelector('.auth-gate-primary')?.getAttribute('aria-busy'),
    disabled: document.querySelector('.auth-gate-primary')?.disabled,
    requests: window.__E2E__.calls.filter(c => c.command === 'devify_auth_status').length,
  })), {sameGate:true,fullPageLoading:false,busy:'true',disabled:true,requests:2});
  await page.evaluate(() => { window.__E2E__.authPaused = false; window.__E2E__.authResolvers.splice(0).forEach(resolve => resolve()); });
  await page.waitForFunction(() => document.querySelector('.auth-gate-primary')?.textContent === '重试');
  assert.ok(await page.evaluate(() => document.querySelector('.auth-gate')?.textContent.includes('备份正在使用登录状态')));

  // Leaving the retry page must invalidate even a successful, delayed response.
  await page.evaluate(() => { window.__E2E__.authPaused = true; });
  await click('重试');
  await page.waitForFunction(() => window.__E2E__.authResolvers.length === 1);
  await click('返回登录');
  await page.waitForFunction(() => document.querySelector('.auth-gate h1')?.textContent === '需要登录');
  await page.evaluate(() => {
    window.__E2E__.authStatus = {authenticated:true,unavailable:false,error:''};
    window.__E2E__.authPaused = false;
    window.__E2E__.authResolvers.splice(0).forEach(resolve => resolve());
  });
  await page.waitForFunction(() => !!document.querySelector('.auth-gate-primary:not(:disabled)'));
  assert.equal(await page.evaluate(() => document.querySelector('.auth-gate h1')?.textContent), '需要登录');
  await click('重新检查已有登录');
  await page.waitForFunction(() => !document.querySelector('.auth-gate'));
  assert.deepEqual(await page.evaluate(() => window.__E2E__.errors), []);
  assert.equal(await page.evaluate(() => window.__E2E__.calls.filter(c => c.command === 'devify_auth_cancel').length), 0, 'successful recovery never cancels the recovered login');
  console.log('PASS login recovery: stable retry view, single in-flight check, stale response ignored, saved-session recovery');
  console.log(await page.snapshot());
}

// Run through ego-browser nodejs: import this module, then pass an owned TaskSpace.
// The fixture exercises production components and routing with strict, isolated IPC.
export async function runConnectorE2E(task, base = 'http://localhost:1420', pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const click = name => page.click(`loc=role:button[name="${name}"]`);
  const ready = name => page.waitForFunction(text => [...document.querySelectorAll('button')].some(b => b.textContent?.trim() === text && !b.disabled), name);
  const state = () => page.evaluate(() => ({ errors: window.__E2E__.errors, settings: window.__E2E__.settings, writes: window.__E2E__.writes, calls: window.__E2E__.calls.map(x => x.command), opened: window.__E2E__.opened }));
  const start = async query => {
    await page.goto(`${base}/scripts/connectors-e2e.html${query}`);
    await ready('设置'); await click('设置');
    await page.waitForFunction(() => document.querySelectorAll('.connector-actions .connector-primary:not(:disabled)').length === 2);
    await page.snapshot();
  };
  const clean = async () => assert.deepEqual((await state()).errors, [], 'unexpected IPC or browser errors');

  await start('?fresh');
  await page.click('loc=css:.connector-actions .connector-primary >> nth=0');
  await click('下一步'); await ready('下一步：连接微信');
  const permissionChecks = (await state()).calls.filter(x => x === 'perm_status').length;
  await page.evaluate(() => { window.dispatchEvent(new Event('focus')); document.dispatchEvent(new Event('visibilitychange')); });
  assert.equal((await state()).calls.filter(x => x === 'perm_status').length, permissionChecks, 'focus never polls permission');
  await click('下一步：连接微信');
  await page.snapshot({ scope: 'full_page' });
  await page.click('loc=role:checkbox[name="我已保存当前工作，同意上述操作，并确认读取的是本人微信数据"]');
  await click('准备微信'); await ready('打开微信');
  await click('打开微信'); await click('我已登录，继续'); await ready('授权并检查');
  await click('授权并检查');
  await page.click('loc=role:checkbox[name="我已退出微信账号，并保留登录窗口"]');
  await click('已退出，继续'); await ready('下一步：选择会话');
  assert.equal((await state()).calls.filter(x => x === 'wechat_capture_start').length, 1);
  await click('下一步：选择会话');
  await page.waitForSelector('[aria-label="首次备份时间选项"]');
  await page.snapshot({ scope: 'full_page' });
  assert.equal(await page.evaluate(() => document.querySelector('[aria-label="首次备份时间选项"] button[aria-pressed="true"]').textContent), '最近 7 天');
  await page.click('loc=css:[aria-label="导入范围"] button:last-child');
  await click('第 2 页');
  await click('单个会话');
  await page.selectOption('select[aria-label="选择要单独设置的会话"]', 'e2e-wechat-chat-44');
  await click('最近 30 天');
  await click('已选 45 个会话');
  assert.ok(await page.evaluate(() => document.querySelector('.import-history-range').textContent.includes('不同的时间范围')));
  await click('最近 7 天');
  await click('下一步'); await page.waitForSelector('.wizard-backup-page');
  let saved = (await state()).settings;
  assert.equal(saved.chat.allowed_usernames['e2e-wechat'].length, 45);
  assert.equal(Object.keys(saved.chat.initial_ranges['e2e-wechat']).length, 45, 'ranges persist beyond the current page');
  assert.ok(Object.values(saved.chat.initial_ranges['e2e-wechat']).every(x => x.since !== null && x.until === null));
  await page.evaluate(() => { window.__E2E__.saveDelay = 500; });
  await page.click('loc=role:switch[name="微信自动备份"]');
  assert.equal(await page.evaluate(() => document.querySelector('.wizard-backup-page .primary').disabled), true, 'pending saves block Next');
  await ready('下一步');
  await page.evaluate(() => { window.__E2E__.saveDelay = 0; });
  await click('5 分钟'); await ready('下一步');
  await page.click('loc=role:switch[name="限制上传时间"]'); await ready('下一步');
  await click('只在此时段上传'); await ready('下一步');
  await click('仅文本'); await ready('下一步');
  await click('下一步'); await page.waitForSelector('.wizard-finish-facts');
  await page.snapshot({ scope: 'full_page' });
  assert.ok(await page.evaluate(() => document.querySelector('.wizard-finish-facts').textContent.includes('每 5 分钟')));
  await click('打开微信');
  await page.waitForFunction(() => window.__E2E__.opened === 2);
  assert.equal((await state()).opened, 2, 'temporary launch and official launch are separate');
  await clean();
  console.log('PASS WeChat: system check → prepare/login/authorize/logout/capture → cross-page selection → 7-day ranges → acknowledged schedule → completion/official open');

  await start('');
  await page.click('loc=css:.connector-actions .connector-primary >> nth=1');
  await click('下一步：系统检查'); await ready('下一步：选择会话');
  await page.snapshot({ scope: 'full_page' });
  const waStart = (await state()).calls.length;
  await click('下一步：选择会话');
  await page.waitForSelector('[aria-label="导入范围"]');
  await page.click('loc=css:[aria-label="导入范围"] button:nth-child(1)');
  await page.click('loc=css:[aria-label="导入范围"] button:nth-child(2)');
  await click('最近 7 天'); await click('保存并继续');
  await page.waitForSelector('loc=role:heading[name="WhatsApp 已连接"]');
  saved = (await state()).settings;
  assert.equal(saved.active_account_id, '10000000000@s.whatsapp.net');
  assert.equal(saved.chat.allowed_usernames[saved.active_account_id].length,45);
  assert.equal(saved.sync.enabled, false, 'WA remains manual-only');
  assert.ok(!(await state()).calls.slice(waStart).some(x => /^wechat_(capture|decrypt|import|scan|key)/.test(x)), 'WA never runs WeChat connection operations');
  await clean();
  console.log('PASS WhatsApp: intro → system check → activation → private + group selection → ranges → persisted completion, without WeChat key/capture operations');

  await start('?fresh&permission=denied');
  await page.click('loc=css:.connector-actions .connector-primary >> nth=0');
  await click('下一步'); await page.waitForSelector('loc=role:button[name="重新检测"]');
  await page.snapshot({ scope: 'full_page' });
  assert.equal(await page.evaluate(() => [...document.querySelectorAll('button')].find(x=>x.textContent==='下一步：连接微信').disabled), true);
  assert.ok(await page.evaluate(() => document.querySelector('.wx-setup-permission').textContent.includes('退出并重新打开')));
  await page.evaluate(() => { window.__E2E__.permission = true; window.dispatchEvent(new Event('focus')); });
  assert.equal(await page.evaluate(() => [...document.querySelectorAll('button')].find(x=>x.textContent==='下一步：连接微信').disabled), true, 'permission only changes after an explicit check');
  await click('重新检测'); await ready('下一步：连接微信'); await clean();
  console.log('PASS WeChat permission: restart instruction, no focus polling or automatic navigation, explicit retry');

  await start('');
  await click('备份周期'); await page.waitForSelector('[aria-label="微信自动备份"]');
  await page.click('loc=role:switch[name="微信自动备份"]');
  await page.waitForFunction(() => window.__E2E__.settings.sync.enabled);
  await page.evaluate(() => { window.__E2E__.failNextSave = true; });
  await click('5 分钟'); await page.waitForSelector('[role="alert"]');
  await page.snapshot({ scope: 'full_page' });
  assert.ok(await page.evaluate(() => document.querySelector('fieldset').disabled));
  await click('放弃未保存修改并重新加载');
  await page.waitForFunction(() => !document.querySelector('fieldset').disabled);
  assert.equal((await state()).settings.chat.sync_interval_seconds,1800);
  assert.ok(await page.evaluate(() => !document.querySelector('.backup-page').textContent.includes('邮件')));
  assert.ok(await page.evaluate(() => document.querySelector('.backup-manual-platform').textContent.includes('仅手动备份')));
  await clean();
  console.log('PASS schedule: conflict blocks edits, explicit reload recovers confirmed revision, WA is manual, email is hidden');
  await start('?untrusted');
  await page.click('loc=css:.connector-actions .connector-primary >> nth=0');
  await page.waitForSelector('.wizard-method-options');
  await page.snapshot({ scope: 'full_page' });
  assert.ok(await page.evaluate(() => document.querySelector('.wizard-method').textContent.includes('签名待确认')));
  assert.ok(await page.evaluate(() => !document.querySelector('.wizard-method').textContent.includes('· 官方签名')));
  await page.click('loc=css:.wizard-method-option >> nth=1');
  await click('下一步'); await ready('下一步：连接微信');
  assert.ok(!(await state()).calls.includes('wechat_capture_prepare'), 'unconfirmed signatures can reach import checks without automatic re-signing');
  await clean();
  console.log('PASS upstream signature behavior: untrusted WeChat can reach manual import, without claiming official status or capturing');

  await runConnectorEnglishSmoke(task, base, pageLabel);
  return { scenarios: 6, errors: (await state()).errors };
}

export async function runConnectorEnglishSmoke(task, base = 'http://localhost:1420', pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const click = name => page.click(`loc=role:button[name="${name}"]`);
  const clean = async () => assert.deepEqual(await page.evaluate(() => window.__E2E__.errors), []);
  await page.goto(`${base}/scripts/connectors-e2e.html?english`);
  await page.waitForSelector('loc=role:button[name="Settings"]');
  await click('Settings');
  await page.waitForSelector('loc=role:button[name="Backup"]');
  await click('Backup');
  await page.waitForSelector('.backup-v7');
  await page.snapshot({ scope: 'full_page' });
  assert.ok(await page.evaluate(() => document.querySelector('.backup-v7').textContent.includes('Manual backups only')));
  await click('Import');
  await page.waitForSelector('.import-history-range');
  assert.ok(await page.evaluate(() => document.querySelector('.import-history-range').textContent.includes('Chat history for the first backup')));
  assert.equal(await page.evaluate(() => document.documentElement.lang), 'en');
  await clean();
  console.log('PASS upstream English interface: navigation, backup schedule and new history-range controls');
  assert.ok(await page.evaluate(() => document.querySelector('.wechat-account-avatar img')?.getAttribute('src').includes('/wechat.svg')));
  return { scenarios: 1, errors: await page.evaluate(() => window.__E2E__.errors) };
}
