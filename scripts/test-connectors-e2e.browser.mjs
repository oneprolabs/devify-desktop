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
  const connectionIcon = () => page.evaluate(() => document.querySelector('.network-connection-icon svg')?.innerHTML);
  const fits = async () => {
    assert.ok(await page.evaluate(() => {
      const box = document.querySelector('.auth-login-modal').getBoundingClientRect();
      const form = document.querySelector('.auth-login-modal form');
      return box.top >= 0 && box.bottom <= innerHeight && box.left >= 0 && box.right <= innerWidth && form.scrollWidth <= form.clientWidth;
    }), 'login fits the viewport without horizontal overflow');
  };
  await page.cdp('Emulation.setDeviceMetricsOverride', {width:1100,height:690,deviceScaleFactor:1,mobile:false});
  await start();
  assert.ok(await page.evaluate(() => !document.querySelector('.auth-gate-card .auth-connection-trigger, .auth-gate-card select')));
  await click('登录…');
  await page.evaluate(() => { window.__loginDialog = document.querySelector('.auth-login-modal'); });
  await click('连接设置');
  await page.waitForSelector('.auth-connection-form select:not(:disabled)');
  assert.equal(await page.evaluate(() => document.querySelector('.auth-connection-form select').value), 'auto');
  const autoIcon = await connectionIcon();
  await page.selectOption('.auth-connection-form select', 'system_proxy');
  await page.waitForFunction(() => window.__E2E__.networkMode === 'system_proxy');
  await page.waitForSelector('.auth-connection-form select:not(:disabled)');
  const proxyIcon = await connectionIcon();
  await click('返回登录');
  assert.ok(await page.evaluate(() => window.__loginDialog === document.querySelector('.auth-login-modal')));
  assert.equal(await page.evaluate(() => document.querySelectorAll('.auth-login-gate select').length), 0, 'sign-in has no repeated connection control');
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
  await page.evaluate(() => { window.__E2E__.authStartError = '读取服务端登录配置失败：503 Service Unavailable'; });
  await click('通过浏览器登录');
  await page.waitForFunction(() => document.querySelector('.auth-error')?.textContent.includes('503'));
  assert.ok(await page.evaluate(() => !document.querySelector('.auth-connection-recovery')), 'an HTTP error does not suggest bypassing the proxy');
  await page.evaluate(() => {
    window.__E2E__.authStartError = '读取服务端登录配置失败：服务端暂不可用，请稍后重试。';
  });
  await click('通过浏览器登录');
  await page.waitForSelector('.auth-connection-recovery select:not(:disabled)');
  assert.ok(await page.evaluate(() =>
    window.__loginDialog === document.querySelector('.auth-login-modal') &&
    !document.querySelector('.auth-connection-form, .auth-connection-link') &&
    document.querySelector('.auth-connection-recovery').textContent.includes('如果本机开启了代理')
  ), 'a connection timeout reveals a single inline recovery control');
  assert.equal(await page.evaluate(() => document.querySelector('.auth-connection-recovery select').value), 'system_proxy');
  await page.selectOption('.auth-connection-recovery select', 'direct');
  await page.waitForFunction(() => window.__E2E__.networkMode === 'direct');
  await page.waitForSelector('.auth-connection-recovery select:not(:disabled)');
  const directIcon = await connectionIcon();
  assert.ok(autoIcon && proxyIcon && directIcon && new Set([autoIcon, proxyIcon, directIcon]).size === 3, 'each saved connection mode shows a distinct icon');
  assert.equal(await page.evaluate(() => document.querySelector('input[name="username"]').value), 'tester', 'changing the connection keeps the form draft');
  assert.equal(await page.evaluate(() => document.querySelector('input[name="password"]').value), 'example-password');
  await page.evaluate(() => {
    window.__E2E__.authStartError = '';
    window.__E2E__.authStartPaused = true;
    window.__E2E__.browserOpenPaused = true;
  });
  await click('重试浏览器登录');
  await page.waitForFunction(() => window.__E2E__.authStartResolvers.length === 1);
  assert.ok(await page.evaluate(() => document.querySelector('.auth-alt').disabled && document.querySelector('.auth-alt .auth-button-spinner') && document.querySelector('.auth-alt').textContent.includes('正在读取登录配置')));
  await page.evaluate(() => { window.__E2E__.authStartPaused = false; window.__E2E__.authStartResolvers.splice(0).forEach(resolve => resolve()); });
  await page.waitForFunction(() => window.__E2E__.browserOpenResolvers.length === 1);
  await page.waitForFunction(() => document.querySelector('.auth-alt').textContent.includes('正在打开浏览器'));
  await page.evaluate(() => { window.__E2E__.browserOpenPaused = false; window.__E2E__.browserOpenResolvers.splice(0).forEach(resolve => resolve()); });
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
  assert.ok(await page.evaluate(() => document.activeElement === document.querySelector('.auth-connection-link')), 'focus wraps within the dialog');
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
  await click('设置');
  await click('网络连接');
  await page.waitForFunction(() => document.querySelector('.settings-page .network-connection select')?.value === 'direct');
  await page.selectOption('.settings-page .network-connection select', 'auto');
  await page.waitForFunction(() => window.__E2E__.networkMode === 'auto');
  await clean();

  await start('&no-server');
  await click('登录…');
  await page.waitForSelector('.auth-connection-form');
  await page.fill('.auth-connection-form input', 'not-a-url');
  await click('保存并返回登录');
  await page.waitForFunction(() => document.querySelector('.auth-connection-form [role="alert"]')?.textContent.includes('完整'));
  assert.equal(await page.evaluate(() => window.__E2E__.writes.length), 0);
  await page.fill('.auth-connection-form input', 'https://example.invalid');
  await click('保存并返回登录');
  await page.waitForFunction(() => window.__E2E__.writes.length === 1);
  await page.waitForSelector('.auth-alt');
  await fits();
  await clean();

  await start('&english');
  await page.click('.auth-gate-primary');
  await fits();
  assert.equal(await page.evaluate(() => document.querySelector('.auth-alt-help').textContent), 'Finish signing in in your browser, then return to the app.');
  assert.equal(await page.evaluate(() => document.querySelector('.auth-divider').textContent), 'or');
  await page.screenshot({path:'/tmp/aimychats-login-v7-english-small.png'});
  await page.evaluate(() => {
    window.__E2E__.authStartError = 'AUTH_CONFIG_UNAVAILABLE: 读取服务端登录配置失败：服务端暂不可用，请稍后重试。';
  });
  await page.click('.auth-alt');
  await page.waitForSelector('.auth-connection-recovery select:not(:disabled)');
  await fits();
  assert.ok(await page.evaluate(() => {
    const recovery = document.querySelector('.auth-connection-recovery');
    const label = recovery.querySelector('label > span').getBoundingClientRect();
    const select = recovery.querySelector('select').getBoundingClientRect();
    const retry = recovery.querySelector('.auth-connection-retry').getBoundingClientRect();
    const centerY = rect => (rect.top + rect.bottom) / 2;
    const control = recovery.querySelector('.network-connection').getBoundingClientRect();
    return label.bottom <= select.top && select.right <= retry.left
      && Math.abs(control.width - select.width) <= 1
      && Math.abs(centerY(select) - centerY(retry)) <= 1;
  }), 'retry aligns with the full-width select below its label');
  assert.ok(await page.evaluate(() => {
    const select = document.querySelector('.auth-connection-recovery select');
    const style = getComputedStyle(select);
    const context = document.createElement('canvas').getContext('2d');
    context.font = style.font;
    const availableWidth = select.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
    return Array.from(select.options).every(option => context.measureText(option.text).width + 4 <= availableWidth);
  }), 'the compact recovery control fits every English connection label beside its icon and arrow');
  await clean();
  console.log('PASS V7 login UI: inline proxy recovery, shared settings, browser stages, password and browser retry, keyboard focus, first-use setup, English and 940×640 layout');
  console.log(await page.snapshot());
}

export async function runLoginRecoveryE2E(task, base = 'http://localhost:1420', pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const click = name => page.click(`loc=role:button[name="${name}"]`);
  await page.goto(`${base}/scripts/connectors-e2e.html?auth-unavailable`);
  await page.waitForFunction(() => document.querySelector('.auth-gate-primary')?.textContent === '重试');
  await page.snapshot();
  await click('连接设置');
  await page.waitForSelector('.auth-connection-panel select:not(:disabled)');
  await page.selectOption('.auth-connection-panel select', 'direct');
  await page.waitForFunction(() => window.__E2E__.networkMode === 'direct');
  await click('连接设置');
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

export async function runManualWechatKeySmoke(task, base = 'http://localhost:1420', pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const click = name => page.click(`loc=role:button[name="${name}"]`);
  const ready = name => page.waitForFunction(text => [...document.querySelectorAll('button')]
    .some(button => button.textContent.trim() === text && !button.disabled), name);
  const enter = async (intel = false) => {
    await page.goto(`${base}/scripts/connectors-e2e.html?fresh${intel ? '&intel' : ''}`);
    await ready('设置'); await click('设置');
    await page.waitForSelector('.connector-actions .connector-primary');
    await page.click('loc=css:.connector-actions .connector-primary >> nth=0');
    await ready('下一步');
    if (intel) {
      await page.waitForFunction(() => document.querySelectorAll('.wizard-method-option').length === 1);
      assert.match(await page.evaluate(() => document.querySelector('.wizard-method-option[aria-pressed="true"]')?.textContent), /手动导入微信密钥/);
    } else {
      await page.click('loc=css:.wizard-method-option >> nth=1');
    }
    await click('下一步');
    await ready('下一步：连接微信'); await click('下一步：连接微信');
    await page.waitForSelector('.wx-setup-file');
  };

  await enter(true);
  await click('手动输入密钥');
  await page.waitForSelector('#wx-manual-key');
  const form = '#wx-manual-key-form';
  assert.ok(await page.evaluate(() => document.querySelector('#wx-manual-key').placeholder.includes('64 位')));
  assert.ok(await page.evaluate(selector => document.querySelector(`${selector} button`).disabled, form));
  for (const [invalid, message] of [['invalid-key', '无法识别密钥格式。'], ['cd'.repeat(32), '导入内容未通过当前微信账号的校验。']]) {
    await page.fill('#wx-manual-key', invalid);
    await click('导入并校验');
    await page.waitForFunction(expected => document.querySelector('.wx-setup-details p')?.textContent === expected, message);
    await ready('导入并校验');
    assert.ok(await page.evaluate(() => !document.querySelector('#wx-manual-key').disabled));
  }
  await page.fill('#wx-manual-key', 'ab'.repeat(32));
  await page.press('#wx-manual-key', 'Enter');
  await ready('下一步：选择会话');
  assert.equal(await page.evaluate(() => document.querySelector('#wx-manual-key')), null);
  const importedAccounts = await page.evaluate(() => window.__E2E__.calls
    .filter(call => call.command === 'wechat_import_keys').map(call => call.args.accountId));
  assert.deepEqual(importedAccounts, ['e2e-wechat', 'e2e-wechat', 'e2e-wechat']);
  assert.ok(await page.evaluate(() => !window.__E2E__.calls.some(call => call.command === 'wechat_capture_prepare')));

  await enter();
  const chooseKeyFile = content => page.evaluate(text => {
    const input = document.querySelector('.wx-setup-file input');
    const data = new DataTransfer();
    data.items.add(new File([text], 'fixture-key.json', { type: 'application/json' }));
    input.files = data.files;
    input.dispatchEvent(new Event('change', { bubbles: true }));
  }, content);
  await chooseKeyFile('{malformed');
  await page.waitForFunction(() => document.querySelector('.wx-setup-details p')?.textContent === '无法识别密钥格式。');
  await page.waitForSelector('.wx-setup-file input:not(:disabled)', { state: 'attached' });
  await chooseKeyFile(JSON.stringify({ passphrase: 'ab'.repeat(32) }));
  await ready('下一步：选择会话');
  await click('下一步：选择会话');
  await ready('群聊');
  await click('群聊');
  await ready('下一步'); await click('下一步');
  await page.waitForSelector('.wizard-backup-page');
  await ready('沿用当前设置并继续'); await click('沿用当前设置并继续');
  await page.waitForSelector('.wizard-finish-facts');
  assert.ok(await page.evaluate(() => window.__E2E__.settings.chat.allowed_usernames['e2e-wechat'].length > 0));
  assert.deepEqual(await page.evaluate(() => window.__E2E__.errors), []);
  console.log('PASS WeChat manual import: Intel default and paste retries, Apple Silicon JSON import through completion');
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
    if (await page.evaluate(() => Boolean(document.querySelector('.connector-back')))) await click('← 返回连接器');
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
  assert.ok(await page.evaluate(() => Boolean(document.querySelector('.wizard-shared-plan-summary'))), 'wizard initially confirms the current shared schedule');
  await click('修改统一设置');
  await page.evaluate(() => { window.__E2E__.saveDelay = 500; });
  await page.click('loc=role:switch[name="所有平台自动备份"]');
  assert.equal(await page.evaluate(() => document.querySelector('.wizard-backup-page .primary').disabled), true, 'pending saves block Next');
  await ready('下一步');
  await page.evaluate(() => { window.__E2E__.saveDelay = 0; });
  await click('5 分钟'); await ready('下一步');
  await page.click('loc=role:switch[name="限制上传时间"]'); await ready('下一步');
  await click('只在此时段上传'); await ready('下一步');
  await page.evaluate(() => { window.__E2E__.saveDelay = 500; });
  await click('仅文本');
  assert.deepEqual(await page.evaluate(() => {
    const next = document.querySelector('.wizard-backup-page .primary');
    return { disabled: next.disabled, label: next.textContent.trim() };
  }), { disabled: true, label: '下一步' }, 'attachment autosave keeps the Next label stable');
  await ready('下一步');
  await page.evaluate(() => { window.__E2E__.saveDelay = 0; });
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
  await page.waitForSelector('.wizard-backup-page');
  await ready('沿用当前设置并继续');
  const sharedBefore = await page.evaluate(() => ({ backup: window.__E2E__.settings.backup, enabled: window.__E2E__.settings.sync.enabled, interval: window.__E2E__.settings.chat.sync_interval_seconds, writes: window.__E2E__.writes.length }));
  await click('沿用当前设置并继续');
  await page.waitForSelector('loc=role:heading[name="WhatsApp 已连接"]');
  saved = (await state()).settings;
  assert.equal(saved.active_account_id, '10000000000@s.whatsapp.net');
  assert.equal(saved.chat.allowed_usernames[saved.active_account_id].length,45);
  const sharedAfter = await page.evaluate(() => ({ backup: window.__E2E__.settings.backup, enabled: window.__E2E__.settings.sync.enabled, interval: window.__E2E__.settings.chat.sync_interval_seconds, writes: window.__E2E__.writes.length }));
  assert.deepEqual(sharedAfter, sharedBefore, 'continuing another platform preserves the shared backup settings without a redundant save');
  assert.ok(await page.evaluate(() => window.__E2E__.runtime.sources.some(source => source.platform === 'whatsapp' && source.setup_complete && source.backup_enabled)), 'WA completes its source setup without changing the global backup switch');
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
  await click('备份周期'); await page.waitForSelector('[aria-label="后台备份"]');
  await page.click('loc=role:switch[name="后台备份"]');
  await page.waitForFunction(() => window.__E2E__.runtime.paused && !document.querySelector('.backup-runtime-switch').disabled);
  await page.click('loc=role:switch[name="后台备份"]');
  await page.waitForFunction(() => !window.__E2E__.runtime.paused && !document.querySelector('.backup-runtime-switch').disabled);
  await page.evaluate(() => { window.__E2E__.failNextSave = true; });
  await click('5 分钟'); await page.waitForSelector('[role="alert"]');
  await page.snapshot({ scope: 'full_page' });
  assert.ok(await page.evaluate(() => document.querySelector('fieldset').disabled));
  await click('放弃未保存修改并重新加载');
  await page.waitForFunction(() => !document.querySelector('fieldset').disabled);
  assert.equal((await state()).settings.chat.sync_interval_seconds,1800);
  assert.ok(await page.evaluate(() => !document.querySelector('.backup-page').textContent.includes('邮件')));
  assert.equal(await page.evaluate(() => document.querySelectorAll('.backup-runtime-switch').length), 1, 'all platforms share one runtime pause control');
  await clean();
  console.log('PASS schedule: conflict blocks edits, explicit reload recovers confirmed revision, both platforms share the automatic schedule, email is hidden');
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

  await runManualWechatKeySmoke(task, base, pageLabel);
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
  assert.ok(await page.evaluate(() => document.querySelector('.backup-v7').textContent.includes('Connected platforms share one automatic backup schedule.')));
  await click('Import');
  await page.waitForSelector('.import-history-range');
  assert.ok(await page.evaluate(() => document.querySelector('.import-history-range').textContent.includes('Chat history for the first backup')));
  assert.equal(await page.evaluate(() => document.documentElement.lang), 'en');
  await clean();
  console.log('PASS upstream English interface: navigation, backup schedule and new history-range controls');
  assert.ok(await page.evaluate(() => document.querySelector('.wechat-account-avatar img')?.getAttribute('src').includes('/wechat.svg')));
  return { scenarios: 1, errors: await page.evaluate(() => window.__E2E__.errors) };
}

// Real React UI with synthetic native IPC; native runtime tests cover durable state.
export async function runEnglishWechatWizardE2E(task, base = 'http://127.0.0.1:1422', pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const click = async name => {
    await page.evaluate(text => [...document.querySelectorAll('button')]
      .find(button => button.textContent.trim() === text && !button.disabled)
      ?.scrollIntoView({ block: 'center' }), name);
    return page.click(`loc=role:button[name="${name}"]`);
  };
  const ready = name => page.waitForFunction(text => [...document.querySelectorAll('button')]
    .some(button => button.textContent.trim() === text && !button.disabled), name);
  const fit = async (selector, width, height) => {
    await page.cdp('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
    const overflow = await page.evaluate(rootSelector => {
      const root = document.querySelector(rootSelector);
      return [root, ...root.querySelectorAll('button, [role="columnheader"]')]
        .filter(element => element.getClientRects().length > 0 && element.scrollWidth > element.clientWidth + 1)
        .map(element => element === root ? rootSelector : element.textContent.trim());
    }, selector);
    assert.deepEqual(overflow, [], `${selector} fits ${width}×${height}`);
  };

  await page.cdp('Emulation.setDeviceMetricsOverride', { width: 940, height: 640, deviceScaleFactor: 1, mobile: false });
  await page.goto(`${base}/scripts/connectors-e2e.html?fresh&intel&english`);
  await ready('Settings');
  await click('Settings');
  await page.waitForSelector('.connector-actions .connector-primary');
  await page.click('loc=css:.connector-actions .connector-primary >> nth=0');
  await ready('Next');
  await click('Next');
  await ready('Next: connect WeChat');
  const systemCheck = await page.evaluate(() => document.querySelector('.wx-setup').innerText);
  assert.match(systemCheck, /Compatibility not yet verified/);
  assert.doesNotMatch(systemCheck, /\p{Script=Han}/u, 'system checks use English UI copy');
  await page.evaluate(() => {
    window.__wizardScrolls = [];
    const original = HTMLElement.prototype.scrollIntoView;
    HTMLElement.prototype.scrollIntoView = function (...args) {
      if (this.matches('.wx-setup-list .wx-setup-item.current')) window.__wizardScrolls.push(this.querySelector('strong')?.textContent);
      return original.apply(this, args);
    };
  });
  await click('Next: connect WeChat');
  await page.waitForSelector('[aria-label="WeChat connection steps"] .current');
  await page.waitForFunction(() => window.__wizardScrolls.length > 0);
  await fit('.wechat-wizard', 940, 640);
  await click('Enter key manually');
  await page.fill('#wx-manual-key', 'invalid-key');
  await page.press('#wx-manual-key', 'Enter');
  await page.waitForFunction(() => {
    const error = document.querySelector('.wx-setup-details p');
    return Boolean(error?.textContent && !/\p{Script=Han}/u.test(error.textContent));
  });
  await page.fill('#wx-manual-key', 'ab'.repeat(32));
  await page.press('#wx-manual-key', 'Enter');
  await ready('Next: choose chats');
  await click('Next: choose chats');
  await page.waitForFunction(() => /Chats found: \d+/.test(document.querySelector('.wizard-import-scope')?.textContent || ''));
  await page.click('[aria-label="Import scope"] button[aria-expanded]');
  await page.waitForSelector('input[aria-label^="Import "]');
  await page.waitForSelector('button[aria-label="Page 2"]');
  await fit('.wechat-wizard', 940, 640);
  await fit('.wechat-wizard', 1360, 900);
  assert.deepEqual(await page.evaluate(() => window.__E2E__.errors), []);
  console.log('PASS English WeChat wizard: current-step scrolling, validation error, dynamic import copy, accessible names and compact layout');
}

export async function runBackupSwitchUiE2E(task, base = 'http://localhost:1434', pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const master = 'button[role="switch"][aria-label="后台备份"]';
  await page.goto(`${base}/scripts/connectors-e2e.html?chat&backup-disabled`);
  await page.click('loc=role:button[name="任务"]');
  await page.waitForSelector(master);
  assert.equal(await page.evaluate(selector => document.querySelector(selector).getAttribute('aria-checked'), master), 'false');
  await page.click(master);
  await page.waitForFunction(() => window.__E2E__.settings.sync.enabled && !window.__E2E__.runtime.paused);
  await page.click('loc=role:button[name="设置"]');
  await page.click('loc=role:button[name="备份周期"]');
  await page.waitForSelector(`.backup-page ${master}`);
  assert.equal(await page.evaluate(selector => document.querySelector(selector).getAttribute('aria-checked'), master), 'true');
  await page.click(master);
  await page.waitForFunction(() => !window.__E2E__.settings.sync.enabled && window.__E2E__.runtime.paused && !document.querySelector('.backup-page fieldset').disabled);
  const revision = await page.evaluate(() => window.__E2E__.settings.user_settings_revision);
  await page.click('loc=role:button[name="5 分钟"]');
  await page.waitForFunction(revision => window.__E2E__.settings.user_settings_revision > revision, revision);
  assert.equal(await page.evaluate(() => window.__E2E__.settings.sync.enabled), false, 'schedule edits cannot restore the stale enabled flag');
  assert.equal(await page.evaluate(() => window.__E2E__.settings.chat.sync_interval_seconds), 300);
  await page.click(master);
  await page.waitForFunction(() => window.__E2E__.settings.sync.enabled && !document.querySelector('.backup-page fieldset').disabled);
  await page.evaluate(() => { window.__E2E__.saveDelay = 500; });
  await page.click('loc=role:button[name="15 分钟"]');
  assert.equal(await page.evaluate(selector => document.querySelector(selector).disabled, master), true, 'the master cannot race a pending schedule save');
  await page.waitForFunction(() => window.__E2E__.settings.chat.sync_interval_seconds === 900 && !document.querySelector('[aria-label="后台备份"]').disabled);
  await page.click('loc=role:button[name="任务"]');
  await page.waitForSelector(master);
  assert.equal(await page.evaluate(selector => document.querySelector(selector).getAttribute('aria-checked'), master), 'true');
  assert.deepEqual(await page.evaluate(() => window.__E2E__.errors), []);
  console.log('PASS backup switch: disabled legacy view, tasks/settings agreement, fresh revisions, schedule preservation and pending-save exclusion (synthetic IPC)');
  return { scenarios: 5 };
}


export async function runSummaryDateUiE2E(task, base = 'http://127.0.0.1:1422', pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const range = label => page.click(`loc=css:.summary-date-range button:text-is("${label}")`, {label:'Select summary date range'});
  const open = () => page.click('.chat-summary-cta', {label:'Open chat summary form'});
  const local = '.summary-agent-options button:nth-child(2):not(:disabled)';
  const generate = '.chat-summary-actions .primary';
  const dates = (from,to) => page.evaluate(values => {
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;
    document.querySelectorAll('.summary-custom-dates input').forEach((input,index) => {
      setValue.call(input,values[index]);
      input.dispatchEvent(new Event('input',{bubbles:true}));
      input.dispatchEvent(new Event('change',{bubbles:true}));
    });
  },[from,to]);
  const submit = async number => {
    await page.waitForSelector(`${generate}:not(:disabled)`);
    await page.click(generate,{label:'Create simulated summary task'});
    await page.waitForSelector('.chat-summary-cta');
    return page.evaluate(count => {
      const requests=window.__E2E__.calls.filter(call=>call.command==='summary_task_enqueue');
      return requests.length===count ? requests.at(-1).args : null;
    },number);
  };

  await page.goto(`${base}/scripts/connectors-e2e.html?chat&internal`);
  await page.waitForSelector('.wxv-msg[role="button"]');
  const expected=await page.evaluate(() => {
    const state=window.__E2E__;
    Object.assign(state.settings.agent,{enabled:true,provider_id:'codex'});
    window.dispatchEvent(new Event('agent-settings-changed'));
    const start=new Date(2024,9,30).getTime()/1000, end=new Date(2024,10,5).getTime()/1000-1;
    const message=(local_id,time,text) => ({local_id,time,text,kind:1,source_shard:'synthetic',sender:'tester'});
    state.chatMessageOverrides[state.settings.active_account_id]=[
      ...Array.from({length:2100},(_,i)=>message(4000-i,end+10000-i,'Newer history')),
      message(3,end+1,'Excluded'),message(2,end,'End of selected day'),message(1,start,'Start of selected day'),
    ];
    return {start:new Date(start*1000).toISOString(),end:new Date(end*1000).toISOString()};
  });
  await page.click('loc=role:tab[name="聊天总结"]',{label:'Open chat summary tab'});
  await page.evaluate(() => {
    window.__E2E__.summaryConfigError = {
      message: '无法连接总结服务，请检查网络。',
      retry_at: Math.floor(Date.now() / 1000) + 120,
    };
  });
  await open();
  const configError = '.summary-request-options [role="alert"]';
  await page.waitForSelector(configError);
  assert.match(await page.evaluate(selector => document.querySelector(selector).textContent, configError),
    /无法连接总结服务.*可在 .* 后重试/);
  assert.equal(await page.evaluate(() => document.querySelectorAll('.summary-request-options [role="alert"]').length), 1);
  assert.equal(await page.evaluate(selector => document.querySelector(selector).disabled, generate), true);
  assert.equal(await page.evaluate(() => window.__E2E__.calls.filter(call => call.command === 'summary_config_get').length), 1);
  await page.evaluate(() => { window.__E2E__.summaryConfigError = ''; });
  await page.click(`${configError} button`, { label: 'Retry reading summary configuration' });
  await page.waitForSelector('.summary-scenario-pills');
  assert.equal(await page.evaluate(selector => document.querySelector(selector), configError), null);
  assert.equal(await page.evaluate(() => window.__E2E__.calls.filter(call => call.command === 'summary_config_get').length), 2);
  await page.waitForSelector(local);
  assert.deepEqual(await page.evaluate(()=>[...document.querySelectorAll('.summary-date-range button')].map(el=>el.textContent)), ['今天','昨天','近 3 天','本周','本月','自定义']);
  await range('自定义');
  assert.equal(await page.evaluate(selector=>document.querySelector(selector).disabled,generate),true,'Missing dates block submission');
  await dates('2024-10-01','2024-11-04');
  await page.waitForSelector('.summary-date-feedback.invalid');
  assert.match(await page.evaluate(()=>document.querySelector('.summary-date-feedback').textContent),/超过一个自然月/);
  await dates('2024-10-25','2024-11-04');
  await page.waitForFunction(()=>document.querySelector('.summary-date-feedback')?.textContent.includes('一周'));
  assert.equal(await page.evaluate(selector=>document.querySelector(selector).disabled,generate),true);
  await dates('2024-10-30','2024-11-04');
  await page.waitForSelector('.summary-date-value');
  assert.match(await page.evaluate(()=>document.querySelector('.summary-date-value').textContent),/含结束日/);
  assert.ok(await page.evaluate(() => {
    const row = document.querySelector('.summary-date-range');
    return [...row.children].every(el=>Math.abs(el.getBoundingClientRect().left-row.getBoundingClientRect().left)<1);
  }),'Date controls and hints align with the form column');
  const cloud=await submit(1);
  assert.equal(cloud.engine,'cloud');
  assert.equal(cloud.body.period_start,expected.start);
  assert.equal(cloud.body.period_end,expected.end);
  assert.deepEqual(cloud.body.messages.map(m=>m.text),['End of selected day','Start of selected day']);

  await open();
  await page.click(local,{label:'Select local summary agent'});
  await dates('2024-10-08','2024-11-04');
  assert.equal((await submit(2)).engine,'local','Custom month is available to the local agent');
  const month=await page.evaluate(()=>{
    const now=new Date(), start=new Date(now.getFullYear(),now.getMonth(),1);
    const state=window.__E2E__;
    state.chatMessageOverrides[state.settings.active_account_id]=[{local_id:1,time:start.getTime()/1000,kind:1,text:'First of the month',source_shard:'synthetic'}];
    return start.toISOString();
  });
  await open();
  await range('本月');
  assert.equal(await page.evaluate(()=>document.querySelectorAll('.summary-custom-dates input').length),0);
  const currentMonth=await submit(3);
  assert.equal(currentMonth.engine,'local');
  assert.equal(currentMonth.body.period_start,month);
  assert.equal(currentMonth.body.messages[0].text,'First of the month');

  await page.click('loc=role:tab[name="原始记录"]',{label:'Return to original messages'});
  await page.click('.wxv-msg[role="button"] >> nth=0',{label:'Select a text message'});
  await page.click('.wxv-selection-actions button.primary',{label:'Summarize the selected message'});
  const picked=await submit(4);
  assert.equal(picked.body.mode,'pick');
  assert.equal(picked.body.messages.length,1);

  await open();
  await page.click('loc=role:button[name="按时间周期"]');
  await range('全部消息');
  assert.match(await page.evaluate(()=>document.querySelector('.summary-date-feedback').textContent),/在线模型/);
  await page.click('.summary-agent-options button:first-child');
  assert.equal(await page.evaluate(selector=>document.querySelector(selector).disabled,generate),true);
  await page.click(local);
  await page.evaluate(() => {
    const state=window.__E2E__;
    state.chatMessageOverrides[state.settings.active_account_id]=[
      {local_id:2,time:1735689600,kind:1,text:'new',source_shard:'synthetic'},
      {local_id:1,time:1609459200,kind:1,text:'old',source_shard:'synthetic'},
    ];
    state.chatLatestPaused=true;
  });
  await page.click(generate);
  await page.waitForFunction(()=>window.__E2E__.chatLatestResolvers.length === 1);
  await page.click('.chat-summary-actions button:first-child:not(:disabled)');
  await page.waitForSelector(`${generate}:not(:disabled)`);
  assert.equal(await page.evaluate(()=>window.__E2E__.chatLatestResolvers.length),1,'The form unlocks even while the cancelled IPC is still in flight');
  await page.evaluate(() => {
    const state=window.__E2E__;
    state.chatLatestPaused=false;
    state.chatLatestResolvers.splice(0).forEach(resolve=>resolve());
  });
  assert.equal(await page.evaluate(()=>window.__E2E__.calls.filter(call=>call.command==='summary_task_enqueue').length),4,'Cancelled reading does not enqueue');
  await page.click(generate);
  await page.waitForFunction(()=>document.querySelector('.summary-date-value')?.textContent.includes('2 条文字'));
  assert.equal(await page.evaluate(()=>window.__E2E__.calls.filter(call=>call.command==='summary_task_enqueue').length),4,'The full-history estimate appears before generation');
  await page.evaluate(selector=>document.querySelector(selector).click(),generate);
  assert.equal(await page.evaluate(()=>window.__E2E__.calls.filter(call=>call.command==='summary_task_enqueue').length),4,'A second read click cannot act as confirmation');
  await page.click('loc=role:tab[name="原始记录"]');
  await page.evaluate(() => { window.__E2E__.settings.language.summary='English'; });
  await page.click('loc=css:.chat-workbench-tabs button:has-text("聊天总结")',{label:'Return to summary tab'});
  await page.waitForFunction(()=>document.querySelector('#summary-output-language')?.value==='English');
  assert.equal(await page.evaluate(()=>document.querySelector('.summary-all-confirm')),null,'Returning with a different default language invalidates the unsubmitted preview');
  await page.click(generate);
  await page.waitForSelector('.summary-all-confirm:not(:disabled)');
  await page.evaluate(selector=>{
    const button=document.querySelector(selector);
    button.click();button.click();
  },'.summary-all-confirm');
  await page.waitForSelector('.chat-summary-cta');
  const all=await page.evaluate(()=>window.__E2E__.calls.filter(call=>call.command==='summary_task_enqueue').slice(4));
  assert.equal(all.length,1,'Duplicate confirmation enqueues only once');
  assert.equal(all[0].args.engine,'local');
  assert.equal(all[0].args.body.all_messages,true);
  assert.equal(all[0].args.body.language,'English');
  assert.deepEqual(all[0].args.body.messages.map(m=>m.text),['old','new']);
  await open();
  await page.evaluate(() => {
    const state=window.__E2E__;
    state.chatMessageOverrides[state.settings.active_account_id]=Array.from({length:401},(_,i)=>({
      local_id:401-i,time:1735689600,kind:1,text:'用于测试分批',source_shard:'synthetic',
    }));
  });
  await page.click(generate);
  await page.waitForFunction(() => document.querySelector('.summary-date-value')?.textContent.includes('401 条文字消息，分 2 批'));
  assert.match(
    await page.evaluate(() => document.querySelector('.summary-date-value').textContent),
    /预计输入 17 KB/,
    'A multi-batch preview shows the estimated input size before confirmation',
  );
  const summaryButtonColors = await page.evaluate(() => {
    const colors = (selector) => {
      const { backgroundColor, color } = getComputedStyle(document.querySelector(selector));
      return { backgroundColor, color };
    };
    return {
      confirm: colors('.summary-all-confirm'),
      primary: colors('.chat-summary-actions button.primary'),
      selectedFilter: colors('.chat-workbench-filters button.active'),
    };
  });
  assert.deepEqual(summaryButtonColors.confirm, summaryButtonColors.primary,
    'The confirmation uses the same colors as the primary summary action');
  assert.deepEqual(summaryButtonColors.confirm, summaryButtonColors.selectedFilter,
    'The confirmation matches the selected chat filter colors');
  await page.click('.summary-all-confirm');
  await page.waitForSelector('.chat-summary-cta');
  const larger = await page.evaluate(() => window.__E2E__.calls
    .filter(call => call.command === 'summary_task_enqueue').at(-1).args.body.messages);
  assert.equal(larger.length, 401);
  assert.deepEqual(larger.map(message => message.local_id), Array.from({ length: 401 }, (_, i) => i + 1));

  await open();
  await page.evaluate(() => {
    const state = window.__E2E__;
    state.chatMessageOverrides[state.settings.active_account_id] = [
      { local_id: 1, time: 1735689600, kind: 1, text: '文'.repeat(4001), source_shard: 'synthetic' },
    ];
  });
  await page.click(generate);
  await page.waitForFunction(() => document.querySelector('.summary-feedback.error')?.textContent.includes('超过本地单次总结范围'));
  await page.evaluate(() => {
    const state=window.__E2E__;
    state.chatMessageOverrides[state.settings.active_account_id]=[];
  });
  await page.click(generate);
  await page.waitForFunction(()=>document.querySelector('.summary-feedback.error')?.textContent.includes('没有可总结'));
  assert.equal(await page.evaluate(()=>window.__E2E__.calls.filter(call=>call.command==='summary_task_enqueue').length),6);
  console.log('PASS chat summary dates: configuration recovery, cloud guard, inclusive history, local date ranges and full history, selection, cancellation and duplicate confirmation (synthetic IPC)');
}

export async function runWechatBackupKeyRecoveryUiE2E(task, base, pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const click = name => page.click(`loc=role:button[name="${name}"]`);
  const ready = name => page.waitForFunction(text => [...document.querySelectorAll('.wechat-wizard button')]
    .some(button => button.textContent.trim() === text && !button.disabled && button.getClientRects().length), name);
  await page.goto(`${base}/scripts/connectors-e2e.html?chat`);
  await page.waitForSelector('.wxv-messages');
  await click('任务');
  await page.waitForSelector('.task-runtime-page');
  assert.equal(await page.evaluate(() => document.querySelector('.task-setup-notice [role="alert"]')), null);
  await page.evaluate(() => {
    const state = window.__E2E__;
    state.runtime.sources.find(source => source.platform === 'wechat').backup_needs_keys = true;
    state.runtime.revision += 1;
    window.__TASK_EMIT__();
  });
  await page.waitForFunction(() => document.querySelector('.task-setup-notice [role="alert"]')
    ?.textContent.includes('微信备份已暂停，请更新密钥'));
  await click('更新微信密钥');
  await page.waitForSelector('.wechat-wizard #wizard-method');
  await page.click('loc=role:button[name*="导入密钥"]');
  await click('下一步');
  await ready('下一步：连接微信');
  await click('下一步：连接微信');
  await page.waitForSelector('.wx-setup-file');
  await click('手动输入密钥');
  await page.fill('#wx-manual-key', 'ab'.repeat(32));
  await page.evaluate(() => {
    window.__E2E__.wechatSyncOutcomes = [
      {rel:'session/session.db',status:'ok',error:null},
      {rel:'message/message_0.db',status:'ok',error:null},
      {rel:'message/media_0.db',status:'failed',error:'Synthetic backup read error'},
    ];
    window.__E2E__.wechatBackupReadError = {kind:'read',detail:'Synthetic backup read error'};
  });
  await click('导入并校验');
  await ready('重新读取');
  await page.waitForFunction(() => document.querySelector('.wechat-wizard .wx-setup-details')
    ?.textContent.includes('Synthetic backup read error'));
  assert.ok(await page.evaluate(() => [...document.querySelectorAll('.wechat-wizard footer button')]
    .find(button => button.textContent.trim() === '返回聊天').disabled));
  await page.evaluate(() => {
    const state = window.__E2E__;
    state.wechatSyncOutcomes = null;
    state.wechatBackupReadError = null;
    state.runtime.sources.find(source => source.platform === 'wechat').backup_needs_keys = false;
    state.runtime.revision += 1;
    window.__TASK_EMIT__();
  });
  await click('重新读取');
  await ready('返回聊天');
  await click('返回聊天');
  await click('任务');
  await page.waitForSelector('.task-runtime-page');
  assert.equal(await page.evaluate(() => document.querySelector('.task-setup-notice [role="alert"]')), null);
  const calls = await page.evaluate(() => window.__E2E__.calls);
  assert.deepEqual(calls.filter(call => call.command === 'wechat_import_keys').map(call => call.args), [
    {accountId:'e2e-wechat',targetOnly:true},
  ]);
  assert.ok(!calls.some(call => call.command === 'connector_setup_state'));
  console.log('PASS WeChat backup key recovery entry and verification (synthetic IPC)');
}

// Real React shell with synthetic IPC, not a server integration test.
export async function runChatRefreshUiE2E(task, base = 'http://127.0.0.1:1437', pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const refresh = '.chat-workbench-refresh';
  const selected = '.chat-workbench-session.selected';
  const message = '.chat-workbench-raw-content .wxv-messages';
  const alert = '.chat-workbench-refresh-error';
  await page.goto(`${base}/scripts/connectors-e2e.html?chat`);
  await page.waitForFunction(() => document.querySelector('.wxv-messages')?.textContent.includes('的测试消息'));
  await page.snapshot();
  const sessionKey = () => page.evaluate(selector => document.querySelector(selector)?.getAttribute('data-session-key'), selected);
  const original = await sessionKey();
  const wx = JSON.parse(original)[0];
  const start = await page.evaluate(() => {
    window.__E2E__.wechatSyncOutcomes = [
      {rel:'contact/contact.db',status:'cached',error:null},
      {rel:'session/session.db',status:'ok',error:null},
      {rel:'message/message_0.db',status:'no-key',error:'旧 salt sensitive-test-salt：数据库已重新加密，需重新提取密钥。'},
    ];
    return window.__E2E__.calls.length;
  });
  await page.click(refresh, {label:'刷新当前微信会话'});
  await page.waitForSelector(alert);
  assert.deepEqual(await page.evaluate(selector => {
    const card = document.querySelector(selector);
    return {
      title: card.querySelector('[role="alert"] strong').textContent,
      action: card.querySelector('button').textContent,
      mainPane: Boolean(card.closest('.chat-workbench-main')),
      rawDetail: card.textContent.includes('sensitive-test-salt'),
    };
  }, alert), {title:'微信密钥需要更新',action:'更新微信密钥',mainPane:true,rawDetail:false});
  assert.ok(await page.evaluate(selector => document.querySelector(selector).textContent.includes('的测试消息'), message));
  const calls = () => page.evaluate(start => window.__E2E__.calls.slice(start), start);
  assert.deepEqual((await calls()).filter(c => c.command === 'wechat_sync').map(c => c.args), [{accountId:wx}]);
  const ready = name => page.waitForFunction(text => [...document.querySelectorAll('button')]
    .some(button => button.textContent.trim() === text && !button.disabled && button.getClientRects().length), name);
  const click = async name => {
    await page.evaluate(text => [...document.querySelectorAll('.wechat-wizard button')]
      .find(button => button.textContent.trim() === text)?.scrollIntoView({block:'center',behavior:'instant'}), name);
    await page.click(`loc=role:button[name="${name}"]`, {label:name});
  };
  await page.click(`${alert} button`, {label:'打开微信密钥更新向导'});
  await page.waitForSelector('.wechat-wizard #wizard-method');
  assert.equal(await page.evaluate(() => document.querySelector('#wizard-method h2').textContent), '更新微信密钥');
  await page.click('loc=role:button[name*="导入密钥"]', {label:'选择导入新密钥'});
  await click('下一步');
  await ready('下一步：连接微信');
  await click('下一步：连接微信');
  await page.waitForSelector('.wx-setup-file');
  await click('手动输入密钥');
  await page.fill('#wx-manual-key', 'ab'.repeat(32));
  await page.evaluate(() => { window.__E2E__.wechatSyncOutcomes = null; });
  await click('导入并校验');
  await ready('返回聊天');
  await click('返回聊天');
  await page.waitForFunction(() => document.querySelector('.wxv-messages')?.textContent.includes('的测试消息')
    && !document.querySelector('.chat-workbench-refresh-error'));
  assert.equal(await sessionKey(), original, 'returning from recovery preserves the selected chat');
  assert.deepEqual((await calls()).filter(c => c.command === 'wechat_import_keys').map(c => c.args), [{accountId:wx,targetOnly:true}]);
  assert.ok(!(await calls()).some(c => c.command === 'connector_setup_state'), 'key recovery preserves connector setup and backup settings');
  await page.waitForSelector(`${refresh}:not(:disabled)`);
  await page.evaluate(() => { window.__E2E__.wechatSyncError = '该账号正在同步，请稍后重试。'; });
  await page.click(refresh, {label:'验证同步暂忙时可重试'});
  await page.waitForSelector(alert);
  assert.match(await page.evaluate(selector => document.querySelector(selector).querySelector('[role="alert"]').textContent, alert), /稍等片刻.*重试/);
  await page.evaluate(() => {
    window.__E2E__.wechatSyncError = '';
    window.__E2E__.wechatSyncOutcomes = [
      {rel:'session/session.db',status:'cached',error:null},
      {rel:'message/message_0.db',status:'failed',error:'WAL 第 2 页 HMAC 校验失败'},
    ];
  });
  await page.click(refresh, {label:'验证日志读取失败提示'});
  await page.waitForFunction(selector => document.querySelector(selector + ' strong')?.textContent.includes('部分微信'), alert);
  assert.deepEqual(await page.evaluate(selector => {
    const card = document.querySelector(selector);
    return {
      title: card.querySelector('strong').textContent,
      action: card.querySelector('button').textContent,
      expanded: card.querySelector('details').open,
      readableHistory: card.querySelector('[role="alert"]').textContent.includes('仍可查看'),
    };
  }, alert), {title:'部分微信聊天记录未能刷新',action:'重试刷新',expanded:false,readableHistory:true});
  await page.click(`${alert} summary`, {label:'展开技术错误详情'});
  await page.waitForFunction(selector => document.querySelector(selector + ' details[open] p')?.textContent.includes('HMAC'), alert);
  await page.evaluate(() => {
    const state = window.__E2E__;
    state.wechatSyncPaused = true;
    state.wechatSyncOutcomes = [
      {rel:'session/session.db',status:'ok',error:null},
      {rel:'message/message_0.db',status:'ok',error:null},
      {rel:'message/media_0.db',status:'no-key',error:'fixture media error'},
    ];
  });
  await page.click(`${alert} button`, {label:'从提示卡重试刷新'});
  await page.waitForFunction(() => window.__E2E__.wechatSyncResolvers.length === 1);
  assert.ok(await page.evaluate(selector => document.querySelector(selector).disabled, refresh));
  assert.ok(await page.evaluate(selector => document.querySelector(selector + ' button').disabled, alert));
  const retry = await page.evaluate(accountId => {
    const state = window.__E2E__;
    state.chatMessageOverrides[accountId] = [{local_id:2,source_shard:'synthetic',time:1795000100,sender:'tester',sender_name:'测试联系人',kind:1,text:'微信同步后收到的新消息',is_self:false}];
    state.wechatSyncPaused = false;
    const start = state.calls.length;
    state.wechatSyncResolvers.shift()();
    return start;
  }, wx);
  await page.waitForFunction(() => document.querySelector('.wxv-messages')?.textContent.includes('微信同步后收到的新消息'));
  assert.equal(await sessionKey(), original);
  assert.ok(!await page.evaluate(selector => document.querySelector(selector), alert));
  const reads = await page.evaluate(start => window.__E2E__.calls.slice(start), retry);
  assert.ok(reads.some(c => c.command === 'chat_sessions' && c.args.accountId === wx));
  assert.ok(reads.some(c => c.command === 'wechat_messages' && c.args.accountId === wx));
  await page.click('[role="tab"][data-platform="whatsapp"]', {label:'切换到 WhatsApp 会话'});
  await page.waitForFunction(() => document.querySelector('.wxv-messages')?.textContent.includes('WhatsApp ·'));
  const wa = JSON.parse(await sessionKey())[0];
  const whatsappStart = await page.evaluate(accountId => {
    const state = window.__E2E__;
    state.chatMessageOverrides[accountId] = [{local_id:2,source_shard:'whatsapp',time:1795000100,sender:'tester',sender_name:'测试联系人',kind:0,text:'WhatsApp 刷新后的新消息',is_self:false}];
    return state.calls.length;
  }, wa);
  await page.click(refresh, {label:'刷新当前 WhatsApp 会话'});
  await page.waitForFunction(() => document.querySelector('.wxv-messages')?.textContent.includes('WhatsApp 刷新后的新消息'));
  assert.equal(await page.evaluate(() => document.querySelector('.chat-workbench-session.selected small')?.textContent), 'WhatsApp 刷新后的新消息');
  assert.deepEqual(await page.evaluate(start => window.__E2E__.calls.slice(start).filter(c => c.command === 'wechat_sync'), whatsappStart), []);
  assert.deepEqual(await page.evaluate(() => window.__E2E__.errors), []);
  return 'PASS chat refresh: prominent key recovery, preserved setup and selection, retry with collapsed diagnostics, refreshed WeChat messages, WhatsApp local reload';
}

export async function runWechatRestartUiE2E(task, base, pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const alert = '.chat-workbench-refresh-error';
  const refresh = '.chat-workbench-refresh';
  const unavailable = 'WECHAT_ACCOUNT_UNAVAILABLE: 暂时无法读取本机微信账号。';
  const tick = () => page.evaluate(() => { for (const poll of window.__E2E__.wechatPolls.values()) poll(); });
  for (const english of [false, true]) {
    await page.goto(`${base}/scripts/connectors-e2e.html?chat&backup-disabled&controlled-chat-poll${english ? '&english' : ''}`);
    await page.waitForFunction(() => window.__E2E__.wechatPolls.size === 1
      && document.querySelector('.wxv-messages')?.textContent.includes('的测试消息'));
    const selection = () => page.evaluate(() => document.querySelector('.chat-workbench-session.selected').dataset.sessionKey);
    const selected = await selection();
    const accountId = JSON.parse(selected)[0];
    await page.evaluate(error => { window.__E2E__.wechatSyncError = error; }, unavailable);
    await tick();
    await page.waitForSelector(alert);
    assert.equal(await page.evaluate(selector => document.querySelector(`${selector} strong`).textContent, alert),
      english ? 'The local WeChat account can’t be read right now.' : '暂时无法读取本机微信账号。');
    assert.ok(await page.evaluate(() => document.querySelector('.wxv-messages').textContent.includes('的测试消息')));
    await page.evaluate(id => {
      const state = window.__E2E__;
      state.wechatSyncError = '';
      // A different reader already wrote the new snapshot, so sync reports cache hits.
      state.wechatSyncRevision = 'after-restart';
      state.chatMessagesError = '临时消息读取失败（模拟）';
      state.chatMessageOverrides[id] = [{local_id:2,source_shard:'synthetic',time:1795000100,sender:'tester',sender_name:'测试联系人',kind:1,text:'重启后收到的新消息',is_self:false}];
    }, accountId);
    await tick();
    await page.waitForFunction(() => !document.querySelector('.chat-workbench-refresh').disabled
      && Boolean(document.querySelector('.wxv-toast.error')));
    assert.ok(await page.evaluate(() => Boolean(document.querySelector('.wxv-messages #msg-1'))), 'a failed message read keeps the previous chat visible');
    const retryStart = await page.evaluate(() => {
      window.__E2E__.chatMessagesError = '';
      return window.__E2E__.calls.length;
    });
    await tick();
    await page.waitForFunction(() => !document.querySelector('.chat-workbench-refresh-error')
      && !document.querySelector('.wxv-toast.error')
      && document.querySelector('.wxv-messages')?.textContent.includes('重启后收到的新消息'), undefined, {timeout:5000});
    const retryReads = await page.evaluate(start => window.__E2E__.calls.slice(start)
      .filter(call => ['chat_sessions', 'wechat_messages'].includes(call.command)).map(call => call.command), retryStart);
    assert.deepEqual(retryReads, ['wechat_messages'], 'retry the failed message read without reloading the session list');
    assert.equal(await selection(), selected);
    const start = await page.evaluate(() => window.__E2E__.calls.length);
    await tick();
    await page.waitForSelector(`${refresh}:not(:disabled)`);
    const unchanged = await page.evaluate(start => window.__E2E__.calls.slice(start).map(call => call.command), start);
    assert.ok(unchanged.includes('wechat_sync'));
    assert.ok(!unchanged.includes('chat_sessions') && !unchanged.includes('wechat_messages'), 'unchanged snapshots do not reload the chat');

    await page.evaluate(() => { window.__E2E__.wechatSyncPaused = true; });
    await tick();
    await page.waitForFunction(() => window.__E2E__.wechatSyncResolvers.length === 1);
    await page.snapshot();
    await page.click('[role="tab"][data-platform="whatsapp"]', {label:'切换到 WhatsApp 会话'});
    await page.waitForFunction(() => window.__E2E__.wechatPolls.size === 0);
    await page.evaluate(error => {
      const state = window.__E2E__;
      state.wechatSyncPaused = false;
      state.wechatSyncError = error;
      state.wechatSyncResolvers.shift()();
    }, unavailable);
    await page.waitForSelector(`${refresh}:not(:disabled)`);
    await page.click('[role="tab"][data-platform="wechat"]', {label:'切回原微信会话'});
    await page.waitForFunction(() => window.__E2E__.wechatPolls.size === 1);
    assert.equal(await selection(), selected);
    assert.equal(await page.evaluate(selector => Boolean(document.querySelector(selector)), alert), false, 'late errors cannot leak into the restored view');
    await page.click('.prototype-nav-item-tasks', {label:'离开聊天进入任务页'});
    await page.waitForFunction(() => window.__E2E__.wechatPolls.size === 0);
    assert.deepEqual(await page.evaluate(() => window.__E2E__.errors), []);
  }
  return 'PASS WeChat restart: localized notice, preserved history, automatic cache recovery, unchanged snapshot and stale-result guards';
}

export async function runWakeChatRefreshUiE2E(task, base, pageLabel = 'p1') {
  const page = task.page(pageLabel);
  await page.goto(`${base}/scripts/connectors-e2e.html?chat`);
  await page.waitForFunction(() => document.querySelector('.wxv-messages')?.textContent.includes('的测试消息'));
  const [accountId, username] = await page.evaluate(() =>
    JSON.parse(document.querySelector('.chat-workbench-session.selected').getAttribute('data-session-key')));
  const emit = (id, names) => page.evaluate(([account_id, usernames]) => {
    const calls = window.__E2E__.calls;
    const start = calls.length;
    window.__WAKE_CHAT_EMIT__({account_id, usernames});
    return calls.slice(start).map(call => call.command);
  }, [id, names]);
  const fillMessages = newest => page.evaluate(({ id, end }) => {
    window.__E2E__.chatMessageOverrides[id] = Array.from({ length: end - 39 }, (_, index) => {
      const local_id = end - index;
      return {local_id,source_shard:'synthetic',time:1795000000 + local_id,sender:'tester',kind:1,text:`唤醒测试消息 ${local_id}`,is_self:false};
    });
  }, { id: accountId, end: newest });
  const settle = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve(true)))));
  await fillMessages(99);
  assert.deepEqual(await emit('another-account', [username]), []);
  assert.deepEqual(await emit(accountId, ['another-conversation']), []);
  await page.fill('.chat-workbench-head-search', '测试消息');
  await settle();
  assert.deepEqual(await emit(accountId, [username]), []);
  assert.ok(await page.evaluate(() => !document.querySelector('.wxv-messages #msg-99')));
  await page.fill('.chat-workbench-head-search', '');
  await settle();
  assert.deepEqual(await emit(accountId, [username]), ['wechat_messages']);
  await page.waitForSelector('.wxv-messages #msg-99');
  assert.ok(await page.evaluate(() => document.querySelector('.wxv-messages').textContent.includes('唤醒测试消息 99')));

  const rowOffset = () => page.evaluate(() => {
    const root = document.querySelector('.wxv-messages');
    return document.querySelector('#msg-70').getBoundingClientRect().top - root.getBoundingClientRect().top;
  });
  await page.evaluate(() => document.querySelector('#msg-70').scrollIntoView({block:'center',behavior:'instant'}));
  await settle();
  const previousOffset = await rowOffset();
  await fillMessages(100);
  assert.deepEqual(await emit(accountId, [username]), ['wechat_messages']);
  await page.waitForSelector('.wxv-messages #msg-100');
  assert.ok(Math.abs(await rowOffset() - previousOffset) <= 2, 'late messages preserve the visible history position');
  return 'PASS wake chat refresh: scoped to the open conversation, preserves search and scroll, reads local messages only';
}

export async function runChatTodoCountUiE2E(task, base, pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const tab = '.chat-workbench-tabs [role="tab"]:nth-child(2)';
  const count = () => page.evaluate(selector => document.querySelector(`${selector} small`)?.textContent ?? null, tab);
  const select = index => page.click(`button[data-session-key='["e2e-wechat","e2e-wechat-chat-${index}"]']`);
  await page.goto(`${base}/scripts/connectors-e2e.html?chat&controlled-chat-poll`);
  await page.waitForSelector('.wxv-messages #msg-1');
  await page.evaluate(() => {
    const ipc = window.__TAURI_INTERNALS__;
    const invoke = ipc.invoke;
    const items = Array.from({ length: 60 }, (_, i) => ({ id: i + 1, title: `待办夹具 ${i + 1}` }));
    window.__todoReads = [];
    ipc.invoke = async (command, args) => {
      if (command === 'chat_remote_lookup') return { conversation: { id: Number(args.username.split('-').at(-1)) + 10 } };
      if (command !== 'devify_analysis_list' || args.kind !== 'events') return invoke(command, args);
      window.__todoReads.push(args);
      if (args.conversationId === 11) await new Promise(resolve => { window.__releaseTodoCount = resolve; });
      if (args.conversationId === 13) throw Error('Synthetic to-do count failure');
      const rows = args.conversationId === 11 ? items : [];
      return { list: args.pageSize ? rows.slice(0, args.pageSize) : rows, pagination: { total: rows.length } };
    };
  });
  await select(1);
  assert.equal(await count(), null, 'an unqueried conversation must not show zero');
  await page.waitForFunction(() => typeof window.__releaseTodoCount === 'function');
  await select(2);
  await page.waitForFunction(selector => document.querySelector(`${selector} small`)?.textContent === '0', tab);
  await page.evaluate(async () => {
    window.__releaseTodoCount();
    delete window.__releaseTodoCount;
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  });
  assert.equal(await count(), '0', 'a late response must not replace the current conversation count');

  await select(3);
  await page.waitForFunction(() => window.__todoReads.some(args => args.conversationId === 13));
  assert.equal(await count(), null, 'a failed count is not a confirmed zero');
  await select(1);
  await page.waitForFunction(() => typeof window.__releaseTodoCount === 'function');
  await page.evaluate(() => window.__releaseTodoCount());
  await page.waitForFunction(selector => document.querySelector(`${selector} small`)?.textContent === '60', tab);
  assert.deepEqual(await page.evaluate(() => window.__todoReads.map(args => args.pageSize)), [1, 1, 1, 1]);
  await page.click(tab);
  await page.waitForFunction(() => window.__todoReads.some(args => args.conversationId === 11 && !args.pageSize));
  await page.evaluate(() => window.__releaseTodoCount());
  await page.waitForFunction(() => document.querySelectorAll('.chat-event-card').length === 60);
  assert.equal(await count(), '60');
  return 'PASS chat to-do count: unknown, zero, request failure, stale response and lazy list';
}

export async function runChatMessageRetryUiE2E(task, base, pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const refresh = '.chat-workbench-refresh';
  await page.goto(`${base}/scripts/connectors-e2e.html?chat`);
  await page.waitForFunction(() => document.querySelector('.wxv-messages')?.textContent.includes('的测试消息'));
  await page.evaluate(() => { window.__E2E__.chatMessagesError = '临时消息读取失败（模拟）'; });
  await page.click(refresh, {label:'刷新并观察消息读取失败'});
  await page.waitForFunction(() => document.querySelector('.wxv-toast.error')?.textContent.includes('临时消息读取失败'));
  assert.ok(await page.evaluate(() => document.querySelector('.wxv-messages')?.textContent.includes('的测试消息')));
  await page.evaluate(() => {
    window.__E2E__.chatMessagesError = '';
    window.__E2E__.chatMessageOverrides['e2e-wechat'] = [{local_id:10,source_shard:'synthetic',time:1795000400,sender:'tester',kind:1,text:'消息读取恢复后的新消息',is_self:false}];
  });
  await page.click(refresh, {label:'重试并恢复消息读取'});
  await page.waitForSelector('.wxv-messages #msg-10');
  assert.ok(!await page.evaluate(() => Boolean(document.querySelector('.wxv-toast.error'))), 'a successful retry clears the previous refresh error');
  return 'PASS chat message retry: recover messages and dismiss the old error';
}

export async function runChatPreviewConsistencyUiE2E(task, base, pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const refresh = '.chat-workbench-refresh';
  const selectedPreview = '.chat-workbench-session.selected small';
  const readPreview = () => page.evaluate(selector => document.querySelector(selector)?.textContent, selectedPreview);
  await page.goto(`${base}/scripts/connectors-e2e.html?chat`);
  await page.waitForFunction(() => document.querySelector('.wxv-messages')?.textContent.includes('的测试消息'));
  const initial = await page.evaluate(() => document.querySelector('.wxv-messages #msg-1 .wxv-bubble').textContent);
  await page.evaluate(() => {
    const state = window.__E2E__;
    state.chatSessionPreviews['e2e-wechat-chat-0'] = {preview:'侧边索引独有的消息',last_time:1795000600};
    state.chatMessageOverrides['e2e-wechat'] = [{local_id:15,source_shard:'synthetic',time:1795000300,sender:'tester',kind:1,text:'正文已读到新消息',is_self:false}];
    state.chatLatestPaused = true;
  });
  await page.click(refresh, {label:'刷新，延迟正文读取'});
  await page.waitForFunction(() => window.__E2E__.chatLatestResolvers.length === 1);
  const pendingPreview = await readPreview();
  await page.evaluate(() => {
    window.__E2E__.chatLatestPaused = false;
    window.__E2E__.chatLatestResolvers.shift()();
  });
  await page.waitForSelector('.wxv-messages #msg-15');
  assert.equal(pendingPreview, initial, 'while the body is loading, keep the preview of the message still on screen');
  assert.equal(await readPreview(), '正文已读到新消息', 'the newest readable message replaces an inconsistent index preview');
  await page.evaluate(() => {
    window.__E2E__.chatSessionPreviews['e2e-wechat-chat-0'].preview = null;
    window.__E2E__.chatMessagesError = '临时消息读取失败（模拟）';
  });
  await page.click(refresh, {label:'刷新，模拟正文读取失败'});
  await page.waitForSelector('.wxv-toast.error');
  assert.equal(await readPreview(), '正文已读到新消息', 'a failed refresh keeps the previous preview and body together');
  await page.waitForSelector('.wxv-messages #msg-15');
  await page.evaluate(() => {
    const state = window.__E2E__;
    state.chatMessagesError = '';
    state.chatMessageOverrides['e2e-wechat'] = [{local_id:16,source_shard:'synthetic',time:1795000100,sender:'tester',kind:1,text:'快照更换后的可读消息',is_self:false}];
  });
  await page.click(refresh, {label:'刷新，侧边索引无预览而正文有记录'});
  await page.waitForSelector('.wxv-messages #msg-16');
  assert.equal(await readPreview(), '快照更换后的可读消息', 'a missing index preview is filled from the committed body');
  assert.ok(!await page.evaluate(() => document.querySelector('.wxv-messages #msg-15')), 'the replaced window does not retain a newer row that is absent in the source');
  return 'PASS chat preview consistency: sidebar ahead or empty, delayed or failed reads, and snapshot replacement';
}

export async function runConversationSearchCycleUiE2E(task, base, pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const field = '.chat-workbench-head-search';
  const marks = '.wxv-messages mark';
  await page.goto(`${base}/scripts/connectors-e2e.html?chat`);
  await page.waitForSelector('.wxv-messages #msg-1');
  await page.evaluate(() => {
    window.__E2E__.chatMessageOverrides['e2e-wechat'] = Array.from({length:40}, (_, index) => {
      const id = 50 - index;
      const text = id === 12 ? '关键词 关键词' : id === 32 ? '第三处关键词' : id === 48 ? '末尾关键词' : '其他消息';
      return {local_id:id,source_shard:'synthetic',time:1795000000+id,sender:'tester',kind:1,text,is_self:false};
    });
  });
  await page.click('.chat-workbench-refresh', {label:'加载有重复关键词的模拟会话'});
  await page.waitForSelector('.wxv-messages #msg-50');
  await page.fill(field, '关键词');
  await page.waitForFunction(selector => document.querySelectorAll(selector).length === 4, marks);
  const current = () => page.evaluate(selector => {
    const index = [...document.querySelectorAll(selector)].findIndex(node => node.getAttribute('aria-current') === 'true');
    return index;
  }, marks);
  assert.equal(await current(), 0, 'the first occurrence is active when a query finds matches');
  for (const index of [1, 2, 3, 0]) {
    await page.press(field, 'Enter');
    assert.equal(await current(), index);
    assert.ok(await page.evaluate(selector => {
      const bounds = document.querySelector(selector + '[aria-current="true"]').getBoundingClientRect();
      const pane = document.querySelector('.wxv-messages').getBoundingClientRect();
      return document.activeElement.matches('.chat-workbench-head-search') && bounds.top >= pane.top && bounds.bottom <= pane.bottom;
    }, marks), 'the active occurrence is visible and the input keeps keyboard focus');
  }
  await page.evaluate(selector => document.querySelector(selector).dispatchEvent(new KeyboardEvent('keydown', {
    key:'Enter', isComposing:true, bubbles:true,
  })), field);
  assert.equal(await current(), 0, 'committing an IME composition does not advance');
  await page.fill(field, '末尾');
  await page.waitForFunction(selector => document.querySelector(selector + '[aria-current="true"]')?.textContent === '末尾', marks);
  await page.press(field, 'Enter');
  assert.equal(await current(), 0, 'one match wraps to itself');
  await page.click('.wxv-filterbar button', {label:'取消本次会话搜索'});
  assert.equal(await page.evaluate(selector => document.querySelector(selector).value, field), '');
  await page.waitForFunction(selector => !document.querySelector(selector), marks);
  await page.evaluate(() => {
    window.__E2E__.chatMessageOverrides['e2e-wechat'] = Array.from({length:120}, (_, index) => {
      const id = 250 - index;
      return {local_id:id,source_shard:'synthetic',time:1795000000+id,sender:'tester',kind:1,text:id === 132 || id === 143 ? '历史记录命中' : '近期消息',is_self:false};
    });
  });
  await page.click('.chat-workbench-refresh', {label:'载入需要翻历史页才能命中的会话'});
  await page.waitForSelector('.wxv-messages #msg-250');
  assert.ok(!await page.evaluate(() => document.querySelector('.wxv-messages #msg-143')));
  await page.fill(field, '历史记录命中');
  await page.waitForFunction(selector => document.querySelectorAll(selector).length === 2, marks);
  assert.equal(await current(), 0);
  await page.press(field, 'Enter');
  assert.equal(await current(), 1);
  await page.press(field, 'Enter');
  assert.equal(await current(), 0);
  return 'PASS in-chat search: individual matches, Enter wraparound, retained focus, IME, reset and older messages';
}

export async function runChatRefreshPagingUiE2E(task, base, pageLabel = 'p1') {
  const page = task.page(pageLabel);
  const refresh = '.chat-workbench-refresh';
  const fillMessages = newest => page.evaluate(end => {
    window.__E2E__.chatMessageOverrides['e2e-wechat'] = Array.from({length:end-100},(_,i) => {
      const id = end - i;
      return {local_id:id,source_shard:'synthetic',time:1795000000+id,sender:'tester',kind:1,text:`分页测试 ${id}`,is_self:false};
    });
  }, newest);
  const older = async () => {
    await page.waitForSelector('.wxv-more:not(:disabled)');
    await page.evaluate(() => document.querySelector('.wxv-more').scrollIntoView({block:'center',behavior:'instant'}));
    await page.click('.wxv-more', {label:'加载更早一页消息'});
  };
  await page.goto(`${base}/scripts/connectors-e2e.html?chat`);
  await page.waitForFunction(() => document.querySelector('.wxv-messages')?.textContent.includes('的测试消息'));
  await fillMessages(300);
  await page.click(refresh, {label:'刷新初始消息窗口'});
  await page.waitForSelector('.wxv-messages #msg-300');
  await page.evaluate(() => { window.__E2E__.chatOlderPaused = true; });
  await older();
  await page.waitForFunction(() => window.__E2E__.chatOlderResolvers.length === 1);
  await fillMessages(450);
  await page.evaluate(() => { window.__E2E__.chatLatestPaused = true; });
  await page.click(refresh, {label:'旧分页等待时刷新'});
  await page.waitForFunction(() => window.__E2E__.chatLatestResolvers.length === 1);
  await page.waitForSelector('.wxv-more:disabled');
  await page.evaluate(() => {
    window.__E2E__.chatLatestPaused = false;
    window.__E2E__.chatLatestResolvers.shift()();
  });
  await page.waitForSelector('.wxv-messages #msg-450');
  await page.evaluate(async () => {
    window.__E2E__.chatOlderPaused = false;
    window.__E2E__.chatOlderResolvers.shift()();
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  });
  await older();
  await page.waitForFunction(() => !document.querySelector('.wxv-more:disabled'));
  assert.ok(await page.evaluate(() => Boolean(document.querySelector('.wxv-messages #msg-350'))), 'pagination fills the page directly below the refreshed messages');
  assert.ok(!await page.evaluate(() => Boolean(document.querySelector('.wxv-messages #msg-101'))), 'the old request cannot append a disconnected window');
  return 'PASS chat refresh while paging: the older request cannot skip messages';
}

export async function runEventRelevanceUiE2E(task, base = 'http://localhost:1434', pageLabel = 'p1') {
  const page = task.page(pageLabel);
  await page.goto(`${base}/scripts/connectors-e2e.html?analysis`);
  await page.waitForFunction(() => Boolean(window.__E2E__));
  await page.evaluate(() => {
    window.__E2E__.analysisEvents = Array.from({length:101}, (_, i) => ({
      id:i+1, title:i === 0 ? '确认上线窗口' : i === 60 ? '已完成的相关事项' : `分类测试事项 ${i+1}`,
      content:'负责人已确认，等待下一步处理。', status:i === 60 ? 'done' : 'open',
      viewer_relevance:i < 60 ? 'my_todo' : 'related', viewer_evidence_message_ids:[9001],
      evidence:[
        {id:9001,message_id:9002,sender_name:'李四',content:'普通原文：需要确认时间。'},
        {id:9002,message_id:9001,sender_name:'张三',content:'个人依据：我来确认上线时间。'},
      ],
    }));
  });
  await page.snapshot();
  await page.click('text="待办"');
  await page.waitForSelector('.event-workspace .analysis-items button');
  const tabs = () => page.evaluate(() => [...document.querySelectorAll('.analysis-pills button')].map(n => n.textContent));
  const lastRequest = () => page.evaluate(() => window.__E2E__.calls.filter(c => c.command === 'devify_analysis_list').at(-1).args);
  assert.deepEqual(await lastRequest(), {kind:'events',spaceId:'e2e-space',page:1,pageSize:50,viewerRelevance:'my_todo',includeRelevanceCounts:false});
  assert.equal(await page.evaluate(() => document.querySelector('.analysis-pills button[aria-pressed="true"]').textContent), '我的待办60');
  assert.deepEqual(await tabs(), ['我的待办60','与我有关41','其他0']);
  assert.equal(await page.evaluate(() => document.querySelectorAll('.analysis-items button').length), 50);
  await page.snapshot();
  await page.click('text="进展与原文"');
  await page.waitForSelector('.analysis-raw-item.event-personal-evidence');
  assert.deepEqual(await page.evaluate(() => [...document.querySelectorAll('.analysis-raw-item.event-personal-evidence')].map(n => n.textContent)), ['张三 · 原文依据个人依据：我来确认上线时间。']);
  await page.screenshot({path:'/tmp/aimychats-event-relevance-filters.png'});
  await page.click('text="下一页"');
  await page.waitForFunction(() => document.querySelector('.event-pagination')?.textContent.includes('第 2 / 2 页') && document.querySelector('.analysis-items button')?.textContent.includes('分类测试事项 51'));
  assert.equal((await lastRequest()).page, 2);
  assert.deepEqual(await tabs(), ['我的待办60','与我有关41','其他0']);
  await page.snapshot();
  await page.click('.analysis-pills button:nth-child(2)');
  await page.waitForFunction(() => document.querySelector('.analysis-items button')?.textContent.includes('已完成的相关事项'));
  assert.equal((await lastRequest()).page, 1);
  assert.equal((await lastRequest()).viewerRelevance, 'related');
  assert.equal(await page.evaluate(() => document.querySelectorAll('.analysis-items button').length), 41);
  assert.ok(await page.evaluate(() => document.querySelector('.analysis-items button').textContent.includes('已完成 · 与我有关')));
  await page.click('.analysis-pills button:nth-child(3)');
  await page.waitForSelector('.event-empty');
  assert.equal((await lastRequest()).viewerRelevance, 'other');
  assert.ok(await page.evaluate(() => document.querySelector('.event-empty').textContent.includes('当前筛选下没有记录')));
  await page.snapshot();
  await page.click('.analysis-pills button:nth-child(1)');
  await page.waitForFunction(() => document.querySelector('.analysis-items button')?.textContent.includes('确认上线窗口'));
  assert.equal((await lastRequest()).page, 1);
  assert.ok(await page.evaluate(() => [...document.querySelectorAll('.analysis-pills button')].every(n => n.scrollWidth <= n.clientWidth)), 'personal categories fit the sidebar');
  await page.screenshot({path:'/tmp/aimychats-my-todos-preview.png'});
  await page.snapshot();
  await page.click('text="关联待办"');
  await page.waitForSelector('.analysis-picker-row');
  assert.equal(await page.evaluate(() => document.querySelectorAll('.analysis-picker-row').length), 100, 'linking retains candidates from other pages and categories');
  assert.deepEqual(await lastRequest(), {kind:'events',spaceId:'e2e-space',page:1});
  assert.deepEqual(await page.evaluate(() => window.__E2E__.errors), []);
  console.log('Three personal categories, default My to-dos, server counts, paging, independent status, source highlighting and space-wide linking passed in Ego.');
  console.log(await page.snapshot());
}

export async function runEventRelevanceRecoveryE2E(task, base = 'http://localhost:1434', pageLabel = 'p1') {
  const page = task.page(pageLabel);
  await page.goto(`${base}/scripts/connectors-e2e.html?analysis`);
  await page.waitForFunction(() => Boolean(window.__E2E__));
  await page.evaluate(() => {
    window.__E2E__.analysisEvents = Array.from({length:51}, (_, i) => ({
      id:i+1,title:`分页恢复事项 ${i+1}`,content:'检查末页事项完成后的恢复。',status:'open',viewer_relevance:'my_todo',
    }));
  });
  await page.snapshot();
  await page.click('text="待办"');
  await page.waitForSelector('.event-pagination');
  await page.snapshot();
  await page.click('text="下一页"');
  await page.waitForFunction(() => document.querySelector('.analysis-items button')?.textContent.includes('分页恢复事项 51'));
  await page.snapshot();
  await page.click('text="标记完成"');
  await page.waitForFunction(() => document.querySelectorAll('.analysis-items button').length === 50 && document.querySelector('.analysis-pills')?.textContent.includes('我的待办50与我有关1'));
  assert.equal(await page.evaluate(() => window.__E2E__.calls.filter(c => c.command === 'devify_analysis_list').at(-1).args.page),1);
  assert.equal(await page.evaluate(() => document.querySelector('.event-pagination')),null);
  await page.evaluate(() => { window.__E2E__.analysisLegacy = true; });
  await page.click('text="刷新"');
  await page.waitForFunction(() => document.body.textContent.includes('暂时显示全部待办') && document.querySelectorAll('.analysis-items button').length === 50);
  assert.ok(await page.evaluate(() => [...document.querySelectorAll('.analysis-pills button')].every(n => n.disabled && n.getAttribute('aria-pressed') === 'false')));
  assert.equal(await page.evaluate(() => window.__E2E__.calls.filter(c => c.command === 'devify_analysis_list').at(-1).args.viewerRelevance),null);
  await page.evaluate(() => { window.__E2E__.analysisLegacy = false; });
  await page.click('text="刷新"');
  await page.waitForFunction(() => !document.body.textContent.includes('暂时显示全部待办') && document.querySelector('.analysis-pills button[aria-pressed="true"]')?.textContent === '我的待办50');
  assert.equal(await page.evaluate(() => window.__E2E__.calls.filter(c => c.command === 'devify_analysis_list').at(-1).args.viewerRelevance),'my_todo');
  assert.deepEqual(await page.evaluate(() => window.__E2E__.errors),[]);
  console.log('Last-page completion, legacy readability and upgrade detection passed in Ego.');
  console.log(await page.snapshot());
}

export async function runEventCompletionUiE2E(task, base = 'http://127.0.0.1:1434', pageLabel = 'p1', english = false) {
  const page = task.page(pageLabel);
  await page.goto(`${base}/scripts/connectors-e2e.html?analysis&analysis-completion${english ? '&english' : ''}`);
  await page.waitForFunction(() => Boolean(window.__E2E__));
  await page.evaluate(() => {
    const fixture = window.__E2E__;
    fixture.analysisEvents = [
      ...Array.from({ length: 51 }, (_, i) => ({
        id: i + 1, title: `Release task ${i + 1}`, content: i === 50 ? 'second-page-only' : '',
        status: i === 50 ? 'in_progress' : 'open', viewer_relevance: 'my_todo',
      })),
      { id: 52, title: 'Release notes', content: '', status: 'done', viewer_relevance: 'my_todo' },
      { id: 53, title: 'Review agenda', content: '', status: 'done', viewer_relevance: 'related' },
      { id: 54, title: 'Share minutes', content: '', status: 'open', viewer_relevance: 'other' },
    ];
  });
  await page.click(`text="${english ? 'To-dos' : '待办'}"`);
  const roles = '.analysis-side > .analysis-pills';
  const statuses = '.event-completion-filters';
  const list = '.analysis-items';
  const labels = selector => page.evaluate(selector => [...document.querySelectorAll(selector)].map(button => button.textContent), selector);
  const lastRequest = () => page.evaluate(() => window.__E2E__.calls.filter(call => call.command === 'devify_analysis_list').at(-1).args);
  await page.waitForFunction(() => document.querySelectorAll('.analysis-items button').length === 50);
  assert.deepEqual(await labels(`${roles} button`), english ? ['My tasks51', 'Involved0', 'Other1'] : ['我的待办51', '与我有关0', '其他1']);
  assert.deepEqual(await labels(`${statuses} button`), english ? ['Incomplete', 'Completed', 'All'] : ['未完成', '已完成', '全部状态']);
  assert.equal((await lastRequest()).completionStatus, 'unfinished');
  assert.ok(await page.evaluate(selectors => selectors.every(selector => {
    const group = document.querySelector(selector);
    const buttons = [...group.querySelectorAll('button')];
    return buttons.length === 3 && buttons.every(button => button.offsetTop === buttons[0].offsetTop
      && button.getBoundingClientRect().right <= group.getBoundingClientRect().right + 1);
  }), [roles, statuses]), 'each filter group fits on one line');

  if (!english) {
    await page.fill('.analysis-search input', 'second-page-only');
    await page.waitForFunction(() => document.querySelector('.analysis-items button')?.textContent.includes('Release task 51'));
    assert.deepEqual(await labels(`${roles} button`), ['我的待办51', '与我有关0', '其他1']);
    assert.equal((await lastRequest()).searchQuery, 'second-page-only');
    assert.equal((await lastRequest()).completionStatus, 'unfinished');
    assert.equal((await lastRequest()).page, 1);
    await page.fill('.analysis-search input', '');
    await page.waitForFunction(() => document.querySelectorAll('.analysis-items button').length === 50);
    await page.click('text="下一页"');
    await page.waitForFunction(() => document.querySelector('.analysis-items button')?.textContent.includes('Release task 51'));
    await page.click('text="标记完成"');
    await page.waitForFunction(() => !document.querySelector('.event-pagination') && document.querySelectorAll('.analysis-items button').length === 50);
    assert.equal((await lastRequest()).page, 1);
  }
  await page.click(`${statuses} button:nth-child(2)`);
  await page.waitForFunction(() => document.querySelector('.analysis-items')?.textContent.includes('Release notes'));
  assert.equal((await lastRequest()).completionStatus, 'done');
  assert.equal((await lastRequest()).viewerRelevance, 'my_todo');
  if (!english) {
    assert.ok((await labels(`${list} button`)).some(label => label.includes('Release task 51')));
    await page.click('text="重新打开"');
    await page.waitForFunction(() => !document.querySelector('.analysis-items')?.textContent.includes('Release task 51'));
    await page.click(`${roles} button:nth-child(2)`);
    await page.waitForFunction(() => document.querySelector('.analysis-items')?.textContent.includes('Review agenda'));
    await page.click(`${roles} button:nth-child(3)`);
    await page.waitForSelector('.event-empty');
    await page.click(`${statuses} button:nth-child(3)`);
    await page.waitForFunction(() => document.querySelector('.analysis-items')?.textContent.includes('Share minutes'));
    assert.equal((await lastRequest()).completionStatus, undefined);
  }
  assert.deepEqual(await page.evaluate(() => window.__E2E__.errors), []);
  console.log(`PASS task completion toolbar and role labels (${english ? 'English' : '中文'})`);
}

// UI + real, read-only development API through a local bridge. Other IPC remains synthetic.
export async function runEventSearchApiE2E(task, fixture, base = 'http://127.0.0.1:1434') {
 const page=task.page('p1');
 await page.goto(`${base}/scripts/connectors-e2e.html?analysis`);
 await page.waitForFunction(()=>Boolean(window.__E2E__));
 await page.evaluate(({space})=>{
  window.__E2E__.settings.devify.active_space_id=space;
  window.__E2E__.settings.devify.base_url='http://192.168.8.182:7000';
  const original=window.__TAURI_INTERNALS__.invoke;
  window.__TAURI_INTERNALS__.invoke=async(command,args={})=>{
   if(!['devify_analysis_list','devify_analysis_detail'].includes(command)||args.kind!=='events')return original(command,args);
   window.__E2E__.calls.push({command,args:structuredClone(args)});
   const params=new URLSearchParams({kind:command==='devify_analysis_list'?'list':'detail'});
   if(command==='devify_analysis_list'){
    params.set('page',String(args.page??1));params.set('page_size',String(args.pageSize??100));
    if(args.viewerRelevance)params.set('viewer_relevance',args.viewerRelevance);
    if(args.includeRelevanceCounts)params.set('include_relevance_counts','true');
    if(args.searchQuery)params.set('q',args.searchQuery);
   }else params.set('id',String(args.itemId));
   const response=await fetch('/__search173_api?'+params);const payload=await response.json();
   if(!response.ok)throw Error(payload.detail??`HTTP ${response.status}`);return payload.data??payload;
  };
 },fixture);
 await page.snapshot();await page.click('text="待办"');
 await page.waitForSelector('input[aria-label="搜索待办"]');
 await page.snapshot();await page.click('.analysis-pills button:nth-child(3)');
 await page.waitForSelector('.analysis-items button');
 const counts=await page.evaluate(()=>document.querySelector('.analysis-pills').textContent);
 await page.fill('input[aria-label="搜索待办"]',fixture.marker);
 await page.waitForFunction(()=>document.querySelector('.event-pagination')?.textContent.includes('第 1 / 2 页')&&document.querySelector('.analysis-items button')?.textContent.includes('desktop-search173-'));
 assert.equal(await page.evaluate(()=>document.querySelectorAll('.analysis-items button').length),50);
 assert.equal(await page.evaluate(()=>document.querySelector('.analysis-pills').textContent),counts);
 await page.snapshot();await page.click('text="下一页"');
 await page.waitForFunction(()=>document.querySelector('.event-pagination')?.textContent.includes('第 2 / 2 页')&&document.querySelectorAll('.analysis-items button').length===6);
 assert.equal(await page.evaluate(()=>document.querySelectorAll('.analysis-items button').length),6);
 const unique='UniqueNeedle A&B +? # 中文';
 await page.fill('input[aria-label="搜索待办"]',unique);
 await page.waitForFunction(()=>document.querySelectorAll('.analysis-items button').length===1&&document.querySelector('.analysis-doc h1')?.textContent.includes('UniqueNeedle'));
 const latest=()=>page.evaluate(()=>window.__E2E__.calls.filter(c=>c.command==='devify_analysis_list').at(-1).args);
 assert.equal((await latest()).searchQuery,unique);assert.equal((await latest()).page,1);
 await page.fill('input[aria-label="搜索待办"]',fixture.marker+' body-only-0');
 await page.waitForFunction(marker=>window.__E2E__.calls.filter(c=>c.command==='devify_analysis_list').at(-1)?.args.searchQuery===marker+' body-only-0'&&document.querySelectorAll('.analysis-items button').length===1,fixture.marker);
 assert.ok(await page.evaluate(()=>document.querySelector('.analysis-items button').textContent.includes('UniqueNeedle')),'content-only server match is retained');
 assert.equal(await page.evaluate(()=>document.querySelector('.analysis-pills').textContent),counts);
 await page.snapshot();await page.click('.analysis-pills button:nth-child(1)');
 await page.waitForSelector('.event-empty');
 assert.equal((await latest()).page,1);assert.equal((await latest()).viewerRelevance,'my_todo');
 assert.equal((await latest()).searchQuery,fixture.marker+' body-only-0');
 await page.snapshot();await page.click('text="清除搜索"');
 await page.waitForFunction(()=>document.querySelector('.event-empty')&&document.querySelector('input[aria-label="搜索待办"]')?.value===''&&!window.__E2E__.calls.filter(c=>c.command==='devify_analysis_list').at(-1)?.args.searchQuery);
 assert.equal((await latest()).page,1);
 assert.deepEqual(await page.evaluate(()=>window.__E2E__.errors),[]);
 console.log('Real development API + React UI: cross-page search, 50/6 pagination, special characters, content matches, stable badges, category/query reset and clear passed.');
 console.log(await page.snapshot());
}
