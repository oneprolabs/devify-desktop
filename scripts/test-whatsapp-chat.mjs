import * as nativeErrors from '../src/shared/i18n/errorText.ts';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';

// Render the actual workbench while account discovery is delayed. The message pane must
// wait for the resolved account; mounting it early causes a competing WeChat scan.
let current;
const react = {
  useState(initial) {
    const instance = current, index = instance.index++;
    if (!(index in instance.hooks)) instance.hooks[index] = typeof initial === 'function' ? initial() : initial;
    return [instance.hooks[index], value => { instance.hooks[index] = typeof value === 'function' ? value(instance.hooks[index]) : value; }];
  },
  useRef(initial) {
    const instance = current, index = instance.index++;
    return instance.hooks[index] ??= { current: initial };
  },
  useMemo(fn, deps) {
    const instance = current, index = instance.index++, old = instance.hooks[index];
    if (!old || !deps || deps.some((value, i) => !Object.is(value, old.deps?.[i]))) instance.hooks[index] = { deps, value: fn() };
    return instance.hooks[index].value;
  },
  useCallback(fn, deps) { return react.useMemo(() => fn, deps); },
  useEffect(fn, deps) {
    const instance = current, index = instance.index++, old = instance.hooks[index];
    if (!old || !deps || deps.some((value, i) => !Object.is(value, old.deps?.[i]))) {
      instance.effects.push(() => { old?.cleanup?.(); instance.hooks[index] = { deps, cleanup: fn() }; });
    }
  },
};
react.useLayoutEffect = react.useEffect;
const jsx = { jsx: (type, props, key) => ({ type, props, key }), jsxs: (type, props, key) => ({ type, props, key }), Fragment: Symbol('Fragment') };
const timers = [];
const intervals = new Map();
let intervalId = 0;
let chatPoll;
let syncRevision = 'fixture';
let uploadStateCalls = 0;
let resolveUploadState;
globalThis.window = { setTimeout: fn => (timers.push(fn), timers.length), clearTimeout() {}, setInterval(fn, delay) { intervals.set(++intervalId, fn); if (delay === 30_000) chatPoll = fn; return intervalId; }, clearInterval(id) { intervals.delete(id); }, addEventListener() {}, removeEventListener() {} };
globalThis.document = { hidden: true };
let resolveAccounts;
const accounts = new Promise(resolve => { resolveAccounts = resolve; });
const account = '123456789@s.whatsapp.net';
const wechat = 'wx_test';
const otherWeChat = 'wx_other';
const session = { username: 'shared', display: 'WhatsApp 测试会话', is_group: false, last_time: 1 };
const wechatSession = { username: 'shared', display: '微信测试会话', is_group: false, last_time: 2 };
let hideWhatsAppFromFirstPage = false;
let runtime = { sources: [{account_id:account,platform:'whatsapp',setup_complete:true},{account_id:wechat,platform:'wechat',setup_complete:true}] };
let emptyWhatsApp = false;
let removedWhatsApp = false;
const badgeReads = [];
let pagedSessions = false, failNextWeChatPage = false, resolvePagedBadges = false;
let longSessionPages = false;
const pageReads = [];
const sessionReads = [];
const pagedTotal = id => id === wechat ? (longSessionPages ? 1100 : 1000) : 52;
const pagedUsernames = id => Array.from({ length: pagedTotal(id) }, (_, index) =>
  longSessionPages && id !== account ? String(index) : `${id}-${index}`);
const invoke = async (command, args) => {
  if (command === 'chat_accounts') return accounts;
  if (command === 'app_settings_get') {
    return {
      active_account_id: account,
      connectors: { whatsapp_enabled: true },
      chat: {
        allowed_usernames: {
          [account]: removedWhatsApp ? [] : pagedSessions ? pagedUsernames(account) : [session.username],
          [wechat]: pagedSessions ? pagedUsernames(wechat) : [wechatSession.username],
          ...(longSessionPages ? { [otherWeChat]: pagedUsernames(otherWeChat) } : {}),
        },
      },
    };
  }
  if (command === 'chat_sessions') {
    sessionReads.push(args.accountId);
    if(removedWhatsApp&&args.accountId===account)return [];
    if (pagedSessions) {
      const offset=args.offset??0;
      if(offset)pageReads.push([args.accountId,offset]);
      if(offset&&args.accountId===wechat&&failNextWeChatPage){failNextWeChatPage=false;throw Error('temporary local read failure');}
      if (longSessionPages && args.accountId !== account) {
        const start = Math.min(offset, 1000);
        const other = args.accountId === otherWeChat;
        const count = Math.min(args.limit, pagedTotal(args.accountId) - start);
        return Array.from({ length: count }, (_, i) => {
          const index = start + i;
          if (!other && index === 50) return { username: '0', display: 'Stale duplicate', last_time: 5000, is_group: false };
          if (!other && index === 250) return { username: '1', display: 'Updated conversation', last_time: 10001, is_group: false };
          if (!other && index === 300) return { username: '1', display: 'Unconfirmed index result', last_time: 10003, is_group: false };
          return { username: String(index), display: `${args.accountId} chat ${index}`, last_time: (other ? 9975 : 10000) - index, is_group: false };
        });
      }
      const total = pagedTotal(args.accountId);
      return Array.from({ length: Math.min(args.limit, total - offset) }, (_, i) => {
        const index = offset + i;
        const row = { username: `${args.accountId}-${index}`, display: `${args.accountId} chat ${index}`, last_time: 1000 - index, is_group: false };
        if (args.accountId === account && (index === 0 || index === 50)) {
          return { ...row, community: { id: 'fixture-community', name: 'Fixture Community', is_parent: index === 50 } };
        }
        if (args.accountId === wechat && syncRevision === 'new-message' && index === 0) {
          return { ...row, preview: '自动刷新后的新消息' };
        }
        return row;
      });
    }
    return args.accountId === wechat ? [wechatSession] : hideWhatsAppFromFirstPage || emptyWhatsApp ? [] : [session];
  }
  if (command === 'wechat_upload_state') {
    badgeReads.push(args); uploadStateCalls++;
    if (resolvePagedBadges) {
      assert.ok(args.usernames.length <= 200, 'native upload-state reads are capped at 200 per request');
      const pending = longSessionPages ? '249' : `${wechat}-999`;
      return args.usernames.filter(username => username === pending).map(username => ({ username, state: 'pending', new_messages: 1 }));
    }
    return new Promise(resolve => { resolveUploadState = resolve; });
  }
  if (['wechat_avatars', 'summary_history_list'].includes(command)) return [];
  return null;
};
const MessagePane = () => null;
const PlatformBar = () => null;
const component = () => null;
const mockedRequire = id => {
  if (id === 'react') return react;
  if (id === 'react/jsx-runtime') return jsx;
  if (id === '@tauri-apps/api/core') return { isTauri: () => true };
  if (id === '../../shared/api/desktopInvoke') return { desktopInvoke: invoke, requestFeedback() {} };
  if (id === './ChatPlatformBar') return { ChatPlatformBar: PlatformBar };
  if (id === '../connectors/wechat/refresh') return { syncWechatForRefresh: async () => ({ revision: syncRevision }) };
  if (id === '../tasks/taskRuntime') return { useTaskRuntime: () => runtime };
  if (id === './local-reader/WeChatLocal') return { default: MessagePane };
  if (id === '../../entitlements') return {
    useInternalAccount: () => ({ internal: false }),
    useCapability: () => ({ allowed: false, loading: false, view: null }),
  };
  if (id === '../connectors/connectorDetect') return { currentAccount: (items, remembered) => items.find(item => item.id === remembered) ?? items[0] };
  if (id === '../backup/syncFeedback') return { syncFeedback: () => 'completed' };
  if (id === './chatPresentation') return { eventViewState: () => 'empty', syncErrorMessage: String };
  if (id === '../../shared/i18n/locale' || id === '../../../shared/i18n/locale') { const format = (value, ...args) => value.replace(/\{(\d+)\}/g, (_, n) => String(args[Number(n)] ?? '')); return { t: value => value, cf: value => value, tf: format, ctf: format, dateLocale: () => 'zh-CN', sortLocale: () => 'zh-Hans-CN' }; }
  if (id.endsWith('/i18n/errorText')) return nativeErrors;
  if (id === '../../toast') return { exportNotSupported() {} };
  if (id.endsWith('.svg')) return id;
  return new Proxy({}, { get: () => component });
};
function load(path) {
  const source = fs.readFileSync(new URL(path, import.meta.url), 'utf8');
  const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2020 } }).outputText;
  const module = { exports: {} };
  new Function('require', 'module', 'exports', code)(mockedRequire, module, module.exports);
  return module.exports;
}
const { ChatWorkbench } = load('../src/features/chat/ChatWorkbench.tsx');
const { avatarColor, messageAvatar } = load('../src/features/chat/local-reader/presentation.tsx');
const partner = { username: 'peer@s.whatsapp.net', display: '赵联系人', is_group: false };
const outgoing = { source_shard: 'whatsapp', is_self: true, sender: null, sender_name: '赵联系人', text: '第一条' };
const selfAvatar = { username: account, label: '林', color: avatarColor(account) };
assert.deepEqual(messageAvatar(outgoing, partner, account, ' 林同学 '), selfAvatar);
assert.deepEqual(messageAvatar({ ...outgoing, sender: partner.username, text: '第二条' }, partner, account, '林同学'), selfAvatar);
for (const display of [undefined, '   ', account.split('@')[0]]) {
  assert.deepEqual(messageAvatar(outgoing, partner, account, display), { ...selfAvatar, label: '我' });
}
const incoming = { ...outgoing, is_self: false };
const peerAvatar = { username: partner.username, label: '赵', color: avatarColor(partner.username) };
assert.deepEqual(messageAvatar(incoming, partner, account, '林同学'), peerAvatar);
assert.deepEqual(messageAvatar({ ...incoming, sender: 'peer@lid', sender_name: null, text: '另一条' }, partner, account), peerAvatar);
const groupMessage = { ...incoming, sender: 'member@lid', sender_name: '陈成员' };
assert.deepEqual(messageAvatar(groupMessage, { ...partner, is_group: true }, account), {
  username: groupMessage.sender, label: '陈', color: avatarColor(groupMessage.sender),
});
assert.deepEqual(messageAvatar({ ...outgoing, sender: 'group@g.us' }, { ...partner, is_group: true }, account, '林同学'), selfAvatar);
assert.deepEqual(messageAvatar({ ...incoming, sender: null, sender_name: ' 陈成员 ' }, { ...partner, is_group: true }, account), {
  username: null, label: '陈', color: avatarColor('whatsapp:unknown-member'),
});
assert.deepEqual(messageAvatar({ ...incoming, sender: 'group@g.us', sender_name: null }, { ...partner, is_group: true }, account), {
  username: null, label: '未', color: avatarColor('whatsapp:unknown-member'),
});
const wechatMessage = { ...outgoing, source_shard: 'message_0', sender: wechat, sender_name: '周同学' };
assert.deepEqual(messageAvatar(wechatMessage, wechatSession, account), {
  username: wechat, label: '周', color: avatarColor(wechat),
});
console.log('WhatsApp direct message avatars follow stable account and contact identities');
const instance = { hooks: [] };
const props = { onOpenData() {}, onOpenWhitelist() {}, onNewUpload() {}, onOpenTasks() {}, onSchedule() {}, onOpenAccount() {} };
function render() {
  current = instance; instance.index = 0; instance.effects = [];
  const tree = ChatWorkbench(props);
  for (const effect of instance.effects) effect();
  current = undefined;
  return tree;
}
function nodes(value) {
  if (Array.isArray(value)) return value.flatMap(nodes);
  if (!value || typeof value !== 'object') return [];
  return [value, ...nodes(value.props?.children)];
}
render();
if (timers.length) timers.shift()();
assert.equal(nodes(render()).filter(node => node.type === MessagePane).length, 0, 'message pane must wait until account discovery completes');
resolveAccounts([{ id: account, source: 'whatsapp', display: '林同学' }, { id: wechat, source: 'wechat' }, { id: otherWeChat, source: 'wechat' }]);
for (let i = 0; i < 20; i++) await Promise.resolve();
let initialTree = render();
initialTree = render();
const firstPane = nodes(initialTree).find(node => node.type === MessagePane);
assert.equal(firstPane?.props.accountId, wechat, 'initial entry opens the most recent chat across sources');
assert.equal(firstPane.props.focusConversation, wechatSession.username);
const textOf = value => typeof value === 'string' || typeof value === 'number' ? String(value) : Array.isArray(value) ? value.map(textOf).join('') : value && typeof value === 'object' ? textOf(value.props?.children) : '';
const flush = async () => { for (let i=0;i<20;i++) await Promise.resolve(); };
let tree = initialTree;
const bar = () => nodes(tree).find(node => node.type === PlatformBar);
const buttons = () => nodes(tree).filter(node => node.type === 'button');
const rows = () => buttons().filter(node => node.props.className?.startsWith('chat-workbench-session') && !node.props.className?.includes('label'));
const scrollList = () => nodes(tree).find(node => node.props?.className === 'chat-workbench-sessions');
const selectPlatform = source => { bar().props.onSelect(source); tree=render(); tree=render(); };
// Selection count covers images too; summary input remains text-only.
const textSelection=[1,2].map(local_id=>({username:'shared',display:'测试会话',sender:'测试联系人',time:local_id,text:'测试文字',local_id}));
firstPane.props.onSummarySelection(textSelection);
firstPane.props.onChatState({active:true,messages:4,selected:4,agentReady:false,agentName:'',agentModel:''});
tree=render();
assert.equal(typeof nodes(tree).find(node=>node.type===MessagePane).props.onSummarizeSelection,'function','the reader footer can open summary options');
let summaryPane=nodes(tree).find(node=>Array.isArray(node.props?.selection));
assert.equal(summaryPane.props.selectionTotal,4);
assert.equal(summaryPane.props.selection.length,2,'media stays out of the text-only request');
firstPane.props.onSummarySelection([]);tree=render();
summaryPane=nodes(tree).find(node=>Array.isArray(node.props?.selection));
assert.equal(summaryPane.props.selectionTotal,4);
nodes(tree).find(node=>node.type===MessagePane).props.onSummarizeSelection();tree=render();
summaryPane=nodes(tree).find(node=>Array.isArray(node.props?.selection));
assert.equal(summaryPane.props.visible,true,'the footer action opens the summary tab');
assert.equal(summaryPane.props.pickRequest,1,'the footer requests selected-message mode even for an all-media selection');
assert.equal(summaryPane.props.selectionTotal,4,'opening summary options preserves the complete selection count');
buttons().find(node=>node.props.role==='tab'&&textOf(node).startsWith('原始记录')).props.onClick();tree=render();
firstPane.props.onChatState({active:true,messages:4,selected:0,agentReady:false,agentName:'',agentModel:''});
tree=render();
assert.ok(!buttons().some(node=>textOf(node).startsWith('总结所选')),'clearing selection hides the action');
assert.equal(bar().props.selected,'wechat');
assert.equal(bar().props.platforms.length,2);
assert.ok(rows().every(node=>textOf(node).includes('微信测试会话')),'one platform is displayed at a time');
assert.equal(badgeReads[0].accountId,wechat,'badges follow the displayed account, not settings.active_account_id');
for(const tick of intervals.values()){tick();tick();}
assert.equal(uploadStateCalls,1,'slow badge scans cannot overlap');
scrollList().props.ref.current={scrollTop:120,getBoundingClientRect:()=>({top:0}),querySelectorAll:()=>[]};
selectPlatform('whatsapp');
assert.ok(rows().every(node=>textOf(node).includes('WhatsApp 测试会话')));
let pane=nodes(tree).find(node=>node.type===MessagePane);
assert.equal(pane.props.accountId,account);
assert.equal(pane.props.accountDisplay, '林同学', 'the reader receives the selected account name, not the contact name');
assert.equal(pane.key,account,'changing platforms replaces the owning message pane');
assert.equal(pane.props.focusConversation,session.username);
assert.equal(uploadStateCalls,1,'switching waits for the previous bounded badge scan');
resolveUploadState([{username:session.username,state:'pending',new_messages:99}]);
await flush();tree=render();await flush();tree=render();
assert.equal(badgeReads.at(-1).accountId,account,'the new platform refreshes as soon as the stale scan finishes');
assert.ok(!textOf(tree).includes('99 条'),'late WeChat badges cannot appear in WhatsApp');
resolveUploadState([{username:session.username,state:'local',new_messages:0}]);
await flush();tree=render();
const pendingFilter=()=>buttons().find(node=>textOf(node).startsWith('待传'));
assert.equal(textOf(pendingFilter()),'待传 1','an initial backup belongs to the pending filter');
pendingFilter().props.onClick();tree=render();
assert.equal(rows().length,1);assert.ok(textOf(tree).includes('待首次备份'));
assert.ok(!buttons().some(node=>textOf(node).includes('只在本机')));
selectPlatform('wechat');
assert.equal(scrollList().props.ref.current.scrollTop,120,'each platform restores its own scroll position');
assert.equal(nodes(tree).find(node=>node.type===MessagePane).props.accountId,wechat,'identical IDs retain their owning account');
assert.equal(nodes(tree).find(node => node.type === MessagePane).props.accountDisplay, undefined, 'switching accounts drops the WhatsApp display name');
resolveUploadState([{username:wechatSession.username,state:'pending',new_messages:3}]);await flush();tree=render();
assert.equal(textOf(pendingFilter()),'待传 1','new messages also belong to pending');
selectPlatform('whatsapp');
assert.equal(pendingFilter().props.className,'active','platform filters are remembered separately');
const searchInput = (className) => nodes(tree).find(node => node.type === 'input' && node.props.className === className);
searchInput('chat-workbench-head-search').props.onChange({ target: { value: 'message only' } });
tree = render();
assert.equal(searchInput('chat-workbench-search').props.value, '', 'conversation search must not change the sidebar query');
assert.equal(nodes(tree).find(node => node.type === MessagePane).props.searchKeyword, 'message only');
searchInput('chat-workbench-search').props.onChange({ target: { value: 'sidebar only' } });
tree = render();
assert.equal(searchInput('chat-workbench-head-search').props.value, 'message only');
const selectChatTab = label => {
  buttons().find(node => node.props.role === 'tab' && textOf(node).startsWith(label)).props.onClick();
  tree = render();
};
selectChatTab('待办');
assert.equal(searchInput('chat-workbench-head-search').props.value, '');
searchInput('chat-workbench-head-search').props.onChange({ target: { value: 'to-do only' } });
tree = render();
selectChatTab('原始记录');
assert.equal(searchInput('chat-workbench-head-search').props.value, 'message only');
selectChatTab('待办');
assert.equal(searchInput('chat-workbench-head-search').props.value, 'to-do only');
assert.equal(searchInput('chat-workbench-search').props.value, 'sidebar only');
selectChatTab('原始记录');
searchInput('chat-workbench-search').props.onChange({ target: { value: '' } });
tree = render();
// Returning from another section preserves a reader absent from the refreshed first page.
hideWhatsAppFromFirstPage=true;props.visible=false;tree=render();props.visible=true;tree=render();await flush();tree=render();
assert.equal(nodes(tree).find(node=>node.type===MessagePane).props.accountId,account);
hideWhatsAppFromFirstPage=false;
// Once a slow scan finishes after hiding, no new scan starts until the pane is visible.
props.visible=false;tree=render();assert.equal(intervals.size,0);
const hiddenReads=uploadStateCalls;resolveUploadState([]);await flush();tree=render();
assert.equal(uploadStateCalls,hiddenReads);
props.visible=true;tree=render();await flush();tree=render();
resolveUploadState([]);await flush();tree=render();
// A connected platform with no imported sessions stays reachable and cannot show another platform's reader.
emptyWhatsApp=true;
nodes(tree).find(node=>node.props?.['aria-label']==='刷新会话').props.onClick();await flush();tree=render();
selectPlatform('wechat');selectPlatform('whatsapp');
// Its remembered chat intentionally remains readable on a first-page omission.
assert.equal(nodes(tree).find(node=>node.type===MessagePane).props.accountId,account);
assert.equal(rows().length,0);
emptyWhatsApp=false;pagedSessions=true;
selectPlatform('wechat');
nodes(tree).find(node=>node.props?.['aria-label']==='刷新会话').props.onClick();await flush();tree=render();
const scrollAt=(source,near=true)=>({currentTarget:{getBoundingClientRect:()=>({top:0,bottom:500}),querySelectorAll:()=>nodes(tree).filter(node=>node.props?.['data-more-source']).map(node=>({dataset:{moreSource:node.props['data-more-source']},getBoundingClientRect:()=>({top:near&&node.props['data-more-source']===source?450:2000,bottom:near&&node.props['data-more-source']===source?460:2010})}))}});
scrollList().props.onScroll(scrollAt('wechat',false));assert.equal(pageReads.length,0);
failNextWeChatPage=true;
scrollList().props.onScroll(scrollAt('wechat'));scrollList().props.onScroll(scrollAt('wechat'));
await flush();tree=render();
assert.deepEqual(pageReads,[[wechat,50]],'only the current platform loads, without duplicate requests');
scrollList().props.onScroll(scrollAt('wechat'));assert.equal(pageReads.length,1,'failed pages require explicit retry');
buttons().find(node=>node.props.children==='重试').props.onClick();await flush();tree=render();
assert.deepEqual(pageReads,[[wechat,50],[wechat,50]],'retry preserves the failed offset');
assert.ok(textOf(tree).includes(`${wechat} chat 51`));
selectPlatform('whatsapp');
buttons().find(node=>textOf(node).startsWith('全部')).props.onClick();tree=render();
assert.ok(!textOf(tree).includes(`${wechat} chat 51`),'WeChat rows cannot leak into WhatsApp');
scrollList().props.onScroll(scrollAt('whatsapp'));await flush();tree=render();
assert.deepEqual(pageReads,[[wechat,50],[wechat,50],[account,50]],'platforms keep separate page offsets');
assert.ok(textOf(tree).includes(`${account} chat 51`));
assert.deepEqual(rows().map(row => JSON.parse(row.props['data-session-key'])[1]),
  Array.from({ length: 52 }, (_, index) => `${account}-${index}`), 'community rows stay in chronological order across pages');
assert.ok(textOf(tree).includes('Fixture Community'), 'community identity remains visible');
assert.ok(textOf(tree).includes('已到列表末尾。'),'a short final page confirms the end');
scrollList().props.onScroll(scrollAt('whatsapp'));assert.equal(pageReads.length,3);
selectPlatform('wechat');
assert.ok(textOf(tree).includes(`${wechat} chat 51`),'switching does not discard the other platform page');
syncRevision = 'new-message';
document.hidden = false;
chatPoll();
await flush(); tree = render();
document.hidden = true;
assert.equal(rows().length, 100, 'automatic refresh preserves both loaded pages');
assert.ok(textOf(rows()[0]).includes('自动刷新后的新消息'), 'automatic refresh updates the latest preview');
selectPlatform('whatsapp');
assert.equal(rows().length, 52, 'automatic WeChat refresh preserves WhatsApp pages');
selectPlatform('wechat');
assert.ok(!textOf(tree).includes('已到列表末尾。'),'the end marker follows the platform');
const moreButton = () => buttons().find(node => textOf(node) === '加载更多会话');
assert.ok(moreButton(), 'the unfiltered list offers a manual paging fallback');
searchInput('chat-workbench-search').props.onChange({ target: { value: `${wechat} chat 999` } });tree=render();
assert.equal(rows().length,0);
assert.ok(!textOf(tree).includes('没有符合条件的会话。'),'unread pages can still match the search');
moreButton().props.onClick();await flush();tree=render();
assert.deepEqual(pageReads.at(-1),[wechat,100]);
searchInput('chat-workbench-search').props.onChange({ target: { value: '' } });tree=render();
resolvePagedBadges = true;
resolveUploadState([]);await flush();tree=render();await flush();tree=render();
for (let offset = 150; offset <= 1000; offset += 50) {
  if (offset === 200) failNextWeChatPage = true;
  scrollList().props.onScroll(scrollAt('wechat'));await flush();tree=render();
  assert.deepEqual(pageReads.at(-1),[wechat,offset]);
  if (offset === 200) {
    assert.equal(rows().length, 200, 'a failed page leaves the loaded conversations visible');
    buttons().find(node => node.props.children === '重试').props.onClick();
    await flush();
    tree=render();
    assert.deepEqual(pageReads.slice(-2), [[wechat, 200], [wechat, 200]]);
  }
  assert.equal(rows().length,Math.min(offset + 50, 1000),'later pages append rather than evict previous pages');
}
assert.deepEqual(rows().map(row => row.props['data-session-key']), Array.from({length:1000}, (_, i) => JSON.stringify([wechat, `${wechat}-${i}`])));
assert.ok(textOf(tree).includes('已到列表末尾。'),'an exact multiple ends only after the next page returns empty');
assert.equal(moreButton(),undefined);
const readsAtEnd = pageReads.length;
scrollList().props.onScroll(scrollAt('wechat'));assert.equal(pageReads.length,readsAtEnd,'end of list does not request more pages');
await flush();tree=render();
assert.ok(badgeReads.every(read => read.usernames.length <= 200),'badge requests must respect the native limit');
pendingFilter().props.onClick();tree=render();
assert.equal(rows().length,1);
assert.ok(textOf(rows()[0]).includes(`${wechat} chat 999`),'the last badge batch participates in pending filtering');
selectPlatform('whatsapp');removedWhatsApp=true;
nodes(tree).find(node=>node.props?.['aria-label']==='刷新会话').props.onClick();await flush();tree=render();
assert.equal(nodes(tree).filter(node=>node.type===MessagePane).length,0,'removing a platform selection cannot retain a stale private reader');
assert.equal(rows().length,0);assert.ok(textOf(tree).includes('该平台暂无已导入的会话。'));
assert.equal(bar().props.platforms.length,2,'an empty connected platform remains reachable');
// A draft account never enters chat navigation or starts another local read.
removedWhatsApp=false;
const readsBeforeDraft=sessionReads.length;
runtime={sources:[{account_id:account,platform:'whatsapp',setup_complete:false},{account_id:wechat,platform:'wechat',setup_complete:true}]};
tree=render();await flush();tree=render();tree=render();
assert.equal(bar().props.platforms.length,1);
assert.equal(bar().props.platforms[0].id,'wechat');
assert.ok(!sessionReads.slice(readsBeforeDraft).includes(account),'unfinished WhatsApp accounts must not load conversations');
assert.ok(!nodes(tree).some(node=>node.type===MessagePane&&node.props.accountId===account),'a cached reader is removed when setup becomes incomplete');
runtime={sources:[{account_id:account,platform:'whatsapp',setup_complete:true},{account_id:wechat,platform:'wechat',setup_complete:true}]};
tree=render();await flush();tree=render();tree=render();
assert.equal(bar().props.platforms.length,2,'finishing setup makes the platform available without restarting');

longSessionPages = true;
runtime = { sources: [...runtime.sources, { account_id: otherWeChat, platform: 'wechat', setup_complete: true }] };
render(); await flush(); tree = render();
selectPlatform('wechat');
buttons().find(node => textOf(node).startsWith('全部')).props.onClick(); tree = render();
await flush(); tree = render();
const loadWeChatPage = async () => {
  scrollList().props.onScroll(scrollAt('wechat'));
  await flush(); tree = render(); await flush(); tree = render();
};
const rowKeys = () => rows().map(row => row.props['data-session-key']);
const key = (id, username) => JSON.stringify([id, String(username)]);
await loadWeChatPage();
assert.ok(rowKeys().indexOf(key(wechat, 51)) < rowKeys().indexOf(key(otherWeChat, 49)), 'a later page can precede older rows from another account');
assert.equal(rows()[0].props['data-session-key'], key(wechat, 0), 'an older duplicate cannot move the newest row');
assert.ok(!textOf(tree).includes('Stale duplicate'));
assert.ok(rowKeys().includes(key(otherWeChat, 0)), 'the same username in another account is a separate row');

for (let offset = 100; offset < 250; offset += 50) await loadWeChatPage();
assert.equal(rowKeys().length, 249 + 52, 'loading beyond 200 keeps the first page and deduplicates the overlap');
assert.equal(rowKeys()[0], key(wechat, 0));
assert.equal(rowKeys().at(-1), key(wechat, 249));
assert.equal(new Set(rowKeys()).size, rowKeys().length);
assert.ok(badgeReads.some(read => read.accountId === wechat && read.usernames.includes('249') && read.usernames.length === 49), 'the second upload-state batch includes newly loaded older rows');
pendingFilter().props.onClick(); tree = render();
assert.deepEqual(rowKeys(), [key(wechat, 249)], 'badges from later batches still feed the pending filter');
buttons().find(node => textOf(node).startsWith('全部')).props.onClick(); tree = render();

await loadWeChatPage();
assert.equal(rowKeys()[0], key(wechat, 1), 'a newer duplicate updates and reorders the existing row');
assert.ok(textOf(rows()[0]).includes('Updated conversation'));
rows()[0].props.onClick();
tree = render();
nodes(tree).find(node => node.type === MessagePane).props.onLatestMessage(wechat, '1', { text: 'Preview from readable messages', time: 10002 });
tree = render();
for (let offset = 300; offset <= 1000; offset += 50) await loadWeChatPage();
assert.equal(rowKeys()[0], key(wechat, 1), 'the newest conversation stays at the top after scrolling through later pages');
assert.ok(textOf(rows()[0]).includes('Preview from readable messages'), 'a later index page cannot replace the message-backed preview');
assert.ok(!textOf(tree).includes('Unconfirmed index result'));
assert.deepEqual(pageReads.at(-1), [wechat, 1000]);
assert.equal(rowKeys().at(-1), key(wechat, 1049), 'the last supported full page is retained');
const limitNotice = '已达到会话读取上限，可能还有更早的会话。';
assert.ok(textOf(tree).includes(limitNotice));
assert.ok(!textOf(tree).includes('已到列表末尾。'), 'a full page at the API cap does not prove exhaustion');
assert.equal(moreButton(), undefined);
const lastPageReadCount = pageReads.length;
await loadWeChatPage();
assert.equal(pageReads.length, lastPageReadCount, 'do not request an offset the native API would clamp to the same page');
selectPlatform('whatsapp');
assert.ok(moreButton(), 'a capped WeChat account does not stop WhatsApp pagination');
assert.ok(!textOf(tree).includes(limitNotice));

selectPlatform('wechat');
const loadedKeys = rowKeys().sort();
const beforeRefresh = pageReads.length;
syncRevision = 'large-window-update';
document.hidden = false;
chatPoll();
await flush(); tree = render(); await flush(); tree = render();
document.hidden = true;
assert.deepEqual(rowKeys().sort(), loadedKeys, 'a background refresh keeps the loaded session window');
assert.equal(rowKeys()[0], key(wechat, 1));
assert.ok(textOf(rows()[0]).includes('Preview from readable messages'), 'background refresh keeps the open chat preview until the message reader commits');
const backgroundOffsets = pageReads.slice(beforeRefresh).map(([, offset]) => offset);

failNextWeChatPage = true;
syncRevision = 'failed-window-update';
document.hidden = false;
chatPoll();
await flush(); tree = render(); await flush(); tree = render();
document.hidden = true;
assert.deepEqual(rowKeys().sort(), loadedKeys, 'an incomplete background refresh keeps the prior window');
const refreshAlert = nodes(tree).find(node => node.props?.role === 'alert' && textOf(node).includes('temporary local read failure'));
const retryRefresh = nodes(refreshAlert).find(node => node.type === 'button' && textOf(node) === '重试');
assert.ok(retryRefresh, 'a failed refresh can be retried');
retryRefresh.props.onClick();
await flush(); tree = render(); await flush(); tree = render();
assert.deepEqual(rowKeys().sort(), loadedKeys, 'retry refreshes the already-loaded window without returning to page one');
assert.ok(!textOf(tree).includes('temporary local read failure'), 'successful retry dismisses the error');
assert.deepEqual(backgroundOffsets, Array.from({ length: 10 }, (_, index) => (index + 1) * 100),
  'background refresh reuses the native maximum page size without reading beyond the loaded window');
console.log('Platform isolation, remembered selection/scroll/filter, stale badges, and session pagination/order/deduplication checks passed');
