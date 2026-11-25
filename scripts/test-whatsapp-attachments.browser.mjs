import assert from 'node:assert/strict';

// Production React with synthetic IPC. No WhatsApp account, disk, or server is used.
export async function runWhatsAppAttachmentChecks(task, base, { before, screenshot } = {}) {
  const page = task.page('p1');
  await page.goto(`${base}/scripts/connectors-e2e.html?chat`);
  await page.waitForSelector('[role="tab"][data-platform="whatsapp"]');
  await page.evaluate(() => {
    const state = window.__E2E__;
    const accountId = state.accounts.find(account => account.source === 'whatsapp').id;
    const row = (id, kind, text, attachment) => ({
      local_id: id, time: 1795000000 + id, kind, text, attachment,
      sender: 'fixture@lid', sender_name: '测试成员', source_shard: 'whatsapp', is_self: id % 2 === 0,
    });
    const file = (name, status = 'available') => ({
      kind: 'file', status, name, file_type: 'pdf', size: 2048, can_open: true,
    });
    state.chatMessageOverrides[accountId] = [
      row(103, 1, '[图片] 示例图片', { kind: 'image', status: 'available' }),
      row(102, 8, '[文件] Report.pdf', file('Report.pdf')),
      row(101, 8, '[文件] Restricted.pdf', file('Restricted.pdf', 'denied')),
      row(100, 1, '[图片]', { kind: 'image', status: 'unavailable' }),
      row(99, 2, '[视频] 示例视频', { kind: 'video', status: 'available', has_thumbnail: true, can_open: true }),
      ...Array.from({ length: 98 }, (_, i) => row(98 - i, 0, '用于分页的模拟消息')),
      row(0, 8, '[文件] Older.pdf', file('Older.pdf')),
    ];
    const original = window.__TAURI_INTERNALS__.invoke;
    window.__TAURI_INTERNALS__.convertFileSrc = () => {
      throw Error('WhatsApp attachments must not become replayable asset URLs');
    };
    window.__TAURI_INTERNALS__.invoke = async (command, args = {}) => {
      if (command !== 'whatsapp_attachment_action') return original(command, args);
      state.calls.push({ command, args: structuredClone(args) });
      if (args.action === 'save') return '/Downloads/AIMyChats/WhatsApp-fixture/Report.pdf';
      if (args.action === 'open') return '';
      const width = args.action === 'preview' ? 200 : 600;
      return 'data:image/svg+xml,' + encodeURIComponent(
        `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${width * 0.6}" viewBox="0 0 200 120">
          <rect width="200" height="120" fill="#dff1fd"/><circle cx="150" cy="36" r="19" fill="#f7bf36"/>
          <path d="M0 120L67 40l55 65 38-36 40 51" fill="#407968"/>
        </svg>`,
      );
    };
  });
  await page.click('[role="tab"][data-platform="whatsapp"]', { label: '打开 WhatsApp 模拟会话' });
  await page.waitForFunction(() => document.querySelector('.wxv-messages')?.textContent.includes('Report.pdf'));
  if (before) {
    assert.equal(await page.evaluate(() => document.querySelector('.wxv-messages .wxv-attach')), null);
    if (screenshot) await page.screenshot({ path: screenshot });
    return;
  }
  const thumb = 'button[data-open-image="103"] img';
  await page.waitForFunction(selector => document.querySelector(selector)?.naturalWidth === 200, thumb);
  const transcript = await page.evaluate(() => document.querySelector('.wxv-messages').textContent);
  assert.match(transcript, /示例图片/, 'image caption stays visible');
  assert.match(transcript, /示例视频/, 'video caption stays visible');
  assert.doesNotMatch(transcript, /\[(图片|视频)\]/, 'media bubbles should not repeat the transcript labels');
  await page.waitForFunction(() => document.querySelector('.wxv-messages')?.textContent.includes('没有权限读取此附件')
    && document.querySelector('.wxv-messages')?.textContent.includes('附件在本机不可用'));
  if (screenshot) await page.screenshot({ path: screenshot });
  await page.click('button[data-open-image="103"]', { label: '预览 WhatsApp 原图' });
  await page.waitForFunction(() => document.querySelector('.wxv-image-viewer img')?.naturalWidth === 600);
  await page.click('.wxv-image-viewer-close', { label: '关闭 WhatsApp 图片' });
  await page.waitForFunction(() => document.querySelector('.wxv-attach.video img')?.naturalWidth === 200);
  await page.click('.wxv-attach.video button', { label: '打开 WhatsApp 视频' });
  await page.waitForSelector('.wxv-attach.file summary');
  assert.ok(await page.evaluate(() => {
    const file = document.querySelector('.wxv-attach.file details');
    return !file.open && file.querySelector('summary').textContent.includes('Report.pdf')
      && !/PDF|KB|打开|保存/.test(file.querySelector('summary').textContent)
      && [...file.querySelectorAll('button')].every(button => !button.checkVisibility());
  }), 'collapsed file shows only its name; actions require opening the card');
  await page.click('.wxv-attach.file summary', { label: '展开 PDF 文件操作' });
  await page.click('loc=role:button[name="打开"]', { label: '打开关联 PDF 文件' });
  await page.click('loc=role:button[name="保存文件"]', { label: '保存关联 PDF 文件' });
  await page.waitForFunction(() => document.querySelector('.wxv-messages')?.textContent.includes('文件已保存到：'));
  await page.click('.wxv-more', { label: '加载更早的附件' });
  await page.waitForFunction(() => document.querySelector('.wxv-messages')?.textContent.includes('Older.pdf'));
  const result = await page.evaluate(() => ({
    actions: window.__E2E__.calls.filter(row => row.command === 'whatsapp_attachment_action').map(row => row.args),
    reads: window.__E2E__.calls.filter(row => row.command === 'wechat_messages').map(row => row.args),
    errors: window.__E2E__.errors,
  }));
  const older = result.reads.find(args => args.before);
  assert.ok(older?.includeAttachments, 'request attachment metadata for older pages too');
  for (const [action, localId] of [['preview', 103], ['image', 103], ['open', 102], ['save', 102]]) {
    assert.deepEqual(result.actions.find(args => args.action === action && args.localId === localId), {
      accountId: older.accountId, username: older.username, localId, time: 1795000000 + localId, action,
    });
  }
  assert.ok(result.actions.some(args => args.localId === 99 && args.action === 'open'));
  assert.deepEqual(result.errors, []);
  console.log('PASS simulated WhatsApp thumbnail, original, document actions, missing/denied states and paging');
}
