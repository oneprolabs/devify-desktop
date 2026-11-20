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
let pagedSessions = false, failNextWeChatPage = false;
const pageReads = [];
const invoke = async (command, args) => {
  if (command === 'chat_accounts') return accounts;
  if (command === 'app_settings_get') return { active_account_id: account, connectors: { whatsapp_enabled: true }, chat: { allowed_usernames: { [account]: [session.username] } } };
  if (command === 'chat_sessions') {
    if (pagedSessions) {
      const offset=args.offset??0;
      if(offset)pageReads.push([args.accountId,offset]);
      if(offset&&args.accountId===wechat&&failNextWeChatPage){failNextWeChatPage=false;throw Error('temporary local read failure');}
      return Array.from({length:offset?2:50},(_,i)=>({username:`${args.accountId}-${offset+i}`,display:`${args.accountId} chat ${offset+i}`,last_time:1000-offset-i,is_group:false}));
    }
    return args.accountId === wechat ? [wechatSession] : hideWhatsAppFromFirstPage ? [] : [session];
  }
  if (command === 'wechat_upload_state') { uploadStateCalls++; return new Promise(resolve => { resolveUploadState = resolve; }); }
  if (['wechat_avatars', 'summary_history_list'].includes(command)) return [];
  return null;
};
const MessagePane = () => null;
const component = () => null;
const mockedRequire = id => {
  if (id === 'react') return react;
  if (id === 'react/jsx-runtime') return jsx;
  if (id === '@tauri-apps/api/core') return { isTauri: () => true };
  if (id === './desktopInvoke') return { desktopInvoke: invoke, requestFeedback() {} };
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
const textOf = value => typeof value === 'string' ? value : Array.isArray(value) ? value.map(textOf).join('') : value && typeof value === 'object' ? textOf(value.props?.children) : '';
nodes(initialTree).find(node => node.type === 'button' && textOf(node).includes('WhatsApp 测试会话')).props.onClick();
render();
const panes = nodes(render()).filter(node => node.type === MessagePane);
assert.equal(panes.length, 1);
assert.equal(panes[0].props.accountId, account);
assert.equal(panes[0].key, account, 'changing accounts must replace the previous pane');
assert.equal(panes[0].props.focusConversation, session.username);
// A first-page refresh may omit the selected chat; it must not switch the reader.
hideWhatsAppFromFirstPage = true;
props.visible = false;
render();
props.visible = true;
render();
for (let i = 0; i < 20; i++) await Promise.resolve();
assert.equal(nodes(render()).find(node => node.type === MessagePane).props.accountId, account, 'returning preserves the chosen chat even outside the refreshed first page');
hideWhatsAppFromFirstPage = false;
props.visible = false;
render();
props.visible = true;
render();
for (let i = 0; i < 20; i++) await Promise.resolve();
let tree = render();
let buttons = () => nodes(tree).filter(node => node.type === 'button');
const label = node => nodes(node.props.children).map(child => typeof child.props?.children === 'string' ? child.props.children : '').join(' ');
assert.ok(buttons().some(node => label(node).includes('WhatsApp 测试会话')));
assert.ok(buttons().some(node => label(node).includes('微信测试会话')), 'both sources must be visible together');
// Source grouping replaced the old filter row: one collapsible header per visible source.
assert.ok(!buttons().some(node => node.props['aria-label'] === '只显示微信会话'), 'the source filter row is gone');
const deepText = value => {
  if (typeof value === 'string') return value;
  if (Array.isArray(value)) return value.map(deepText).join('');
  if (value && typeof value === 'object') return deepText(value.props?.children);
  return '';
};
const groupHeaders = () => buttons().filter(node => node.props.className === 'chat-workbench-source-group');
assert.equal(groupHeaders().length, 2, 'each visible source gets one header');
assert.ok(groupHeaders().every(node => node.props['aria-expanded'] === true), 'groups start expanded');
const whatsappHeader = groupHeaders().find(node => deepText(node).includes('WhatsApp'));
assert.ok(whatsappHeader, 'the WhatsApp group header must be identifiable');
whatsappHeader.props.onClick();
tree = render();
assert.ok(buttons().some(node => label(node).includes('微信测试会话')));
assert.ok(!buttons().some(node => label(node).includes('WhatsApp 测试会话')), 'collapsing a group hides only its own conversations');
assert.equal(groupHeaders().find(node => deepText(node).includes('WhatsApp')).props['aria-expanded'], false);
groupHeaders().find(node => deepText(node).includes('WhatsApp')).props.onClick();
tree = render();
assert.ok(buttons().some(node => label(node).includes('WhatsApp 测试会话')), 'expanding brings the conversations back');
buttons().find(node => label(node).includes('微信测试会话')).props.onClick();
tree = render();
const selectedPane = nodes(tree).find(node => node.type === MessagePane);
assert.equal(selectedPane.props.accountId, wechat, 'identical conversation IDs must keep their owning account');
assert.equal(uploadStateCalls, 1);
for (const tick of intervals.values()) { tick(); tick(); }
assert.equal(uploadStateCalls, 1, 'a slow local badge read must not overlap refresh ticks');
props.visible = false;
render();
assert.equal(intervals.size, 0, 'a hidden retained workbench must stop polling');
resolveUploadState([{ username: session.username, state: 'pending', new_messages: 99 }]);
for (let i = 0; i < 10; i++) await Promise.resolve();
render();
assert.equal(uploadStateCalls, 1, 'hiding the pane must not issue a new read');
props.visible = true;
render();
assert.equal(uploadStateCalls, 2, 'returning to the pane resumes a bounded refresh');
resolveUploadState([]);
for (let i = 0; i < 10; i++) await Promise.resolve();
pagedSessions=true;
tree=render();nodes(tree).find(node=>node.props?.['aria-label']==='刷新会话').props.onClick();
for(let i=0;i<20;i++)await Promise.resolve();
tree=render();
const scrollList=()=>nodes(tree).find(node=>node.props?.className==='chat-workbench-sessions');
const scrollAt=(source,near=true)=>({currentTarget:{getBoundingClientRect:()=>({top:0,bottom:500}),querySelectorAll:()=>nodes(tree).filter(node=>node.props?.['data-more-source']).map(node=>({dataset:{moreSource:node.props['data-more-source']},getBoundingClientRect:()=>({top:near&&node.props['data-more-source']===source?450:2000,bottom:near&&node.props['data-more-source']===source?460:2010})}))}});
scrollList().props.onScroll(scrollAt('wechat',false));
assert.equal(pageReads.length,0,'scrolling far from the bottom does not prefetch pages');
groupHeaders().find(node=>deepText(node).includes('微信')).props.onClick();tree=render();
scrollList().props.onScroll(scrollAt('wechat'));
assert.equal(pageReads.length,0,'collapsed sources cannot auto-load');
groupHeaders().find(node=>deepText(node).includes('微信')).props.onClick();tree=render();
failNextWeChatPage=true;
const bottom=scrollAt('wechat');
scrollList().props.onScroll(bottom);scrollList().props.onScroll(bottom);
for(let i=0;i<20;i++)await Promise.resolve();tree=render();
assert.deepEqual(pageReads,[[wechat,50]],'only the visible platform loads, with no overlapping page reads');
scrollList().props.onScroll(scrollAt('wechat'));assert.equal(pageReads.length,1,'a failed page waits for explicit retry');
nodes(tree).find(node=>node.type==='button'&&node.props.children==='重试').props.onClick();
for(let i=0;i<20;i++)await Promise.resolve();tree=render();
assert.deepEqual(pageReads,[[wechat,50],[wechat,50]],'retry keeps the failed platform offset');
scrollList().props.onScroll(scrollAt('whatsapp'));
for(let i=0;i<20;i++)await Promise.resolve();tree=render();
assert.deepEqual(pageReads,[[wechat,50],[wechat,50],[account,50]],'WhatsApp keeps its independent offset');
assert.ok(textOf(tree).includes(`${account} chat 51`));
assert.ok(textOf(tree).includes(`${wechat} chat 51`));
scrollList().props.onScroll(scrollAt('whatsapp'));assert.equal(pageReads.length,3,'end of list does not issue more reads');
console.log('Chat selection, grouping, hidden polling and bounded scroll pagination/retry checks passed');
