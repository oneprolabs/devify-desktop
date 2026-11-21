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
const jsx = { jsx: (type, props, key) => ({ type, props, key }), jsxs: (type, props, key) => ({ type, props, key }), Fragment: Symbol('Fragment') };
const timers = [];
const intervals = new Map();
let intervalId = 0;
let uploadStateCalls = 0;
let resolveUploadState;
globalThis.window = { setTimeout: fn => (timers.push(fn), timers.length), clearTimeout() {}, setInterval(fn) { intervals.set(++intervalId, fn); return intervalId; }, clearInterval(id) { intervals.delete(id); }, addEventListener() {}, removeEventListener() {} };
let resolveAccounts;
const accounts = new Promise(resolve => { resolveAccounts = resolve; });
const account = '123456789@s.whatsapp.net';
const wechat = 'wx_test';
const session = { username: 'shared', display: 'WhatsApp 测试会话', is_group: false, last_time: 1 };
const wechatSession = { username: 'shared', display: '微信测试会话', is_group: false, last_time: 2 };
let hideWhatsAppFromFirstPage = false;
let runtime = { sources: [{account_id:account,platform:'whatsapp',setup_complete:true},{account_id:wechat,platform:'wechat',setup_complete:true}] };
let emptyWhatsApp = false;
let removedWhatsApp = false;
const badgeReads = [];
let pagedSessions = false, failNextWeChatPage = false;
const pageReads = [];
const sessionReads = [];
const invoke = async (command, args) => {
  if (command === 'chat_accounts') return accounts;
  if (command === 'app_settings_get') return { active_account_id: account, connectors: { whatsapp_enabled: true }, chat: { allowed_usernames: { [account]: removedWhatsApp ? [] : [session.username], [wechat]: [wechatSession.username] } } };
  if (command === 'chat_sessions') {
    sessionReads.push(args.accountId);
    if(removedWhatsApp&&args.accountId===account)return [];
    if (pagedSessions) {
      const offset=args.offset??0;
      if(offset)pageReads.push([args.accountId,offset]);
      if(offset&&args.accountId===wechat&&failNextWeChatPage){failNextWeChatPage=false;throw Error('temporary local read failure');}
      return Array.from({length:offset?2:50},(_,i)=>({username:`${args.accountId}-${offset+i}`,display:`${args.accountId} chat ${offset+i}`,last_time:1000-offset-i,is_group:false}));
    }
    return args.accountId === wechat ? [wechatSession] : hideWhatsAppFromFirstPage || emptyWhatsApp ? [] : [session];
  }
  if (command === 'wechat_upload_state') { badgeReads.push(args); uploadStateCalls++; return new Promise(resolve => { resolveUploadState = resolve; }); }
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
  if (id === './desktopInvoke') return { desktopInvoke: invoke, requestFeedback() {} };
  if (id === './ChatPlatformBar') return { ChatPlatformBar: PlatformBar };
  if (id === './taskRuntime') return { useTaskRuntime: () => runtime };
  if (id === './WeChatLocal') return { default: MessagePane };
  if (id === './entitlements') return {
    useInternalAccount: () => ({ internal: false }),
    useCapability: () => ({ allowed: false, loading: false, view: null }),
  };
  if (id === './connectorDetect') return { currentAccount: (items, remembered) => items.find(item => item.id === remembered) ?? items[0] };
  if (id === './syncFeedback') return { syncFeedback: () => 'completed' };
  if (id === './chatPresentation') return { eventViewState: () => 'empty', syncErrorMessage: String };
  if (id === './locale') { const format = (value, ...args) => value.replace(/\{(\d+)\}/g, (_, n) => String(args[Number(n)] ?? '')); return { t: value => value, cf: value => value, tf: format, ctf: format, dateLocale: () => 'zh-CN', sortLocale: () => 'zh-Hans-CN' }; }
  if (id === './toast') return { exportNotSupported() {} };
  if (id.endsWith('.svg')) return id;
  return new Proxy({}, { get: () => component });
};
const source = fs.readFileSync(new URL('../src/ChatWorkbench.tsx', import.meta.url), 'utf8');
const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2020 } }).outputText;
const loaded = { exports: {} };
new Function('require', 'module', 'exports', code)(mockedRequire, loaded, loaded.exports);
const instance = { hooks: [] };
const props = { onOpenData() {}, onOpenWhitelist() {}, onNewUpload() {}, onOpenTasks() {}, onSchedule() {}, onOpenAccount() {} };
function render() {
  current = instance; instance.index = 0; instance.effects = [];
  const tree = loaded.exports.ChatWorkbench(props);
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
resolveAccounts([{ id: account, source: 'whatsapp' }, { id: wechat, source: 'wechat' }]);
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
resolveUploadState([{username:wechatSession.username,state:'pending',new_messages:3}]);await flush();tree=render();
assert.equal(textOf(pendingFilter()),'待传 1','new messages also belong to pending');
selectPlatform('whatsapp');
assert.equal(pendingFilter().props.className,'active','platform filters are remembered separately');
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
selectPlatform('wechat');
assert.ok(textOf(tree).includes(`${wechat} chat 51`),'switching does not discard the other platform page');
scrollList().props.onScroll(scrollAt('wechat'));assert.equal(pageReads.length,3,'end of list does not request more pages');
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
console.log('Platform isolation, remembered selection/scroll/filter, pending-first-backup, stale badges and independent pagination/retry checks passed');
