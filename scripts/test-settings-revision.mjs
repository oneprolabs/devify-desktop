import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';

// Execute the real TSX with a minimal deterministic hook renderer and mocked IPC.
let current;
const react = {
  useState(initial) {
    const instance = current, index = instance.index++;
    if (!(index in instance.hooks)) instance.hooks[index] = typeof initial === 'function' ? initial() : initial;
    return [instance.hooks[index], value => { instance.hooks[index] = typeof value === 'function' ? value(instance.hooks[index]) : value; }];
  },
  useRef(value) {
    const instance = current, index = instance.index++;
    if (!(index in instance.hooks)) instance.hooks[index] = { current: value };
    return instance.hooks[index];
  },
  useEffect(effect, deps) {
    const instance = current, index = instance.index++, previous = instance.hooks[index];
    if (!previous || !deps || !previous.deps || deps.some((value, i) => !Object.is(value, previous.deps[i]))) {
      instance.effects.push(() => {
        previous?.cleanup?.();
        instance.hooks[index] = { deps, cleanup: effect() };
      });
    }
  },
  useMemo(fn, deps) {
    const instance = current, index = instance.index++, previous = instance.hooks[index];
    if (!previous || !deps || !previous.deps || deps.some((value, i) => !Object.is(value, previous.deps[i]))) {
      instance.hooks[index] = { deps, value: fn() };
    }
    return instance.hooks[index].value;
  },
  useCallback(fn, deps) { return react.useMemo(() => fn, deps); },
};
const jsx = { jsx: (type, props) => ({ type, props }), jsxs: (type, props) => ({ type, props }), Fragment: Symbol('Fragment') };
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((a, b) => { resolve = a; reject = b; });
  return { promise, resolve, reject };
};
const flush = async () => { for (let i = 0; i < 16; i++) await Promise.resolve(); };
const render = (instance, component) => {
  current = instance; instance.index = 0; instance.effects = [];
  const tree = component({});
  for (const effect of instance.effects) effect();
  current = undefined;
  return tree;
};
const elements = node => {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!node || typeof node !== 'object') return [];
  return [node, ...elements(node.props?.children)];
};
function loadModule(file) {
  const text = fs.readFileSync(new URL(file, import.meta.url), 'utf8');
  const output = ts.transpileModule(text, { compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2020 } }).outputText;
  const result = { exports: {} };
  new Function('require', 'module', 'exports', output)(mockedRequire, result, result.exports);
  return result.exports;
}
const find = (tree, test) => {
  const node = elements(tree).find(test);
  assert.ok(node, 'expected element missing');
  return node;
};
let invoke;
const installed = { detect: { state: 'installed', app_path: '/Applications/WeChat.app', version: '4.1.2', message: '' }, checking: false, usable: true, canProceed: true, recheck: async () => {} };
let connectorInstall = installed;
const mockedRequire = id => {
  if (id === 'react') return react;
  if (id === 'react/jsx-runtime') return jsx;
  if (id === '@tauri-apps/api/core') return { invoke: (...args) => invoke(...args), isTauri: () => true };
  if (id === '@tauri-apps/api/event') return { listen: async () => () => {} };
  if (id === '@tauri-apps/plugin-deep-link') return { onOpenUrl: async () => () => {} };
  if (id === './devifyDevice') return { registerDevifyDevice: async () => {} };
  if (id === './desktopInvoke') return { desktopInvoke: (...args) => invoke(...args), requestFeedback: () => {} };
  if (id === './Settings') return module.exports;
  if (id === './AccountPicker') return loadModule('../src/AccountPicker.tsx');
  if (id === './Connectors') return loadModule('../src/Connectors.tsx');
  if (id === './connectorDetect') return {
    useConnectorDetect: () => connectorInstall,
    detectTitle: { not_installed: '未检测到微信', untrusted: '检测到的微信不是官方版本' },
    detectTitleFor: (label, state) => ({ not_installed: `未检测到${label}`, untrusted: `检测到的${label}不是官方版本`, installed: `已检测到${label}` })[state],
    currentAccount: (accounts, remembered) => accounts.find(item => item.id === remembered) ?? accounts[0],
  };
  if (id === './serviceUrl.ts' || id === './serviceUrl') return serviceUrl;
  if (id === './locale') return {
    t: text => text,
    tf: (pattern, ...args) => String(pattern).replace(/\{(\d+)\}/g, (_, index) => String(args[Number(index)] ?? '')),
    ctf: (pattern, ...args) => String(pattern).replace(/\{(\d+)\}/g, (_, index) => String(args[Number(index)] ?? '')),
    dateLocale: () => 'zh-CN',
    sortLocale: () => 'zh-Hans-CN',
  };
  if (id.endsWith('.svg') || id.endsWith('.png')) return id;
  if (id.endsWith('.css')) return {};
  throw Error(`unexpected import ${id}`);
};
const serviceUrlSource = fs.readFileSync(new URL('../src/serviceUrl.ts', import.meta.url), 'utf8');
const serviceUrlCompiled = ts.transpileModule(serviceUrlSource, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
}).outputText;
const serviceUrlModule = { exports: {} };
new Function('require', 'module', 'exports', serviceUrlCompiled)(
  mockedRequire, serviceUrlModule, serviceUrlModule.exports,
);
const serviceUrl = serviceUrlModule.exports;
const source = fs.readFileSync(new URL('../src/Settings.tsx', import.meta.url), 'utf8');
const compiled = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2020 } }).outputText;
const module = { exports: {} };
new Function('require', 'module', 'exports', compiled)(mockedRequire, module, module.exports);
const { defaultSettings, mergeSavedSettings, requireSettingsRevision, settingsSaveError, StrategySettings, DevifySettings } = module.exports;

assert.throws(() => requireSettingsRevision({}), /版本/);
assert.throws(() => requireSettingsRevision({ user_settings_revision: -1 }), /版本/);
assert.equal(requireSettingsRevision({ user_settings_revision: 0 }).user_settings_revision, 0);
assert.match(settingsSaveError('SETTINGS_CONFLICT: stale'), /确认当前账号和空间/);
assert.doesNotMatch(settingsSaveError('SETTINGS_CONFLICT: stale'), /SETTINGS_CONFLICT/);
const before = defaultSettings();
const sent = { ...before, agent: { ...before.agent, model: 'sent' } };
const draft = { ...sent, agent: { ...sent.agent, model: 'later', command: 'later-command' } };
const response = { ...sent, user_settings_revision: 1, sync: { ...sent.sync, last_message_count: 44 }, agent: { ...sent.agent, timeout_seconds: 180 } };
const merged = mergeSavedSettings(response, sent, draft);
assert.equal(merged.agent.model, 'later');
assert.equal(merged.agent.command, 'later-command');
assert.equal(merged.sync.last_message_count, 44);
assert.equal(merged.user_settings_revision, 1);

// Explicit save must not run before load, and old response must not overwrite later input.
const load = deferred(), saves = [];
invoke = (command, args) => {
  if (command === 'app_settings_get') return load.promise;
  if (command === 'app_settings_save') { const d = deferred(); saves.push({ args, ...d }); return d.promise; }
  if (command === 'devify_daily_schedule_sync') return Promise.resolve({ message: 'ok' });
  throw Error(command);
};
const strategy = { hooks: [] };
let tree = render(strategy, StrategySettings);
assert.equal(elements(tree).filter(node => node.type === 'button').length, 0);
load.resolve({ ...defaultSettings(), user_settings_revision: 8 }); await flush();
tree = render(strategy, StrategySettings);
find(tree, node => node.type === 'input' && node.props.type === 'time').props.onChange({ target: { value: '09:00' } });
tree = render(strategy, StrategySettings);
find(tree, node => node.type === 'button').props.onClick(); await flush();
assert.equal(saves.length, 1);
assert.equal(saves[0].args.value.user_settings_revision, 8);
find(tree, node => node.type === 'input' && node.props.type === 'time').props.onChange({ target: { value: '10:00' } });
saves[0].resolve({ ...saves[0].args.value, user_settings_revision: 9 }); await flush();
tree = render(strategy, StrategySettings);
assert.equal(find(tree, node => node.type === 'input' && node.props.type === 'time').props.value, '10:00');
find(tree, node => node.type === 'button').props.onClick(); await flush();
assert.equal(saves[1].args.value.user_settings_revision, 9);
saves[1].reject('SETTINGS_CONFLICT: changed'); await flush();
tree = render(strategy, StrategySettings);
assert.equal(find(tree, node => node.type === 'input' && node.props.type === 'time').props.value, '10:00');
assert.ok(elements(tree).some(node => node.props?.role === 'alert'));

// An old account's catalog must not replace the newer selection or filter its whitelist.
const catalogs = { A: deferred(), B: deferred() }, whitelistSaves = [];
invoke = (command, args) => {
  if (command === 'app_settings_get') return Promise.resolve({ ...defaultSettings(), user_settings_revision: 11, active_account_id: 'A', chat: { ...defaultSettings().chat, allowed_usernames: { A: ['a'], B: ['b'] } } });
  if (command === 'chat_accounts') return Promise.resolve([{ id: 'A', source: 'wechat' }, { id: 'B', source: 'wechat' }]);
  if (command === 'devify_upload_identity') return Promise.resolve(null);
  if (command === 'wechat_account_profile') return Promise.resolve(null);
  if (command === 'wechat_session_catalog') return catalogs[args.accountId].promise;
  if (command === 'app_settings_save') { const d = deferred(); whitelistSaves.push({ args, ...d }); return d.promise; }
  throw Error(command);
};
const whitelist = { hooks: [] };
const WhitelistSettings = module.exports.WhitelistSettings;
render(whitelist, WhitelistSettings); await flush();
tree = render(whitelist, WhitelistSettings); await flush();
// The account switcher is a nested component here, so drive its callback directly.
find(tree, node => node.type?.name === 'AccountPicker').props.onChange('B');
tree = render(whitelist, WhitelistSettings); await flush();
assert.equal(find(tree, node => node.type === 'button' && node.props.children === '保存所选会话').props.disabled, true);
const session = username => ({ username, display: username, last_time: null, is_group: false, member_count: null, preview: null, unread: null });
catalogs.B.resolve([session('b'), session('b2')]); await flush();
tree = render(whitelist, WhitelistSettings);
catalogs.A.resolve([session('a')]); await flush();
tree = render(whitelist, WhitelistSettings);
assert.ok(!elements(tree).some(node => node.props?.['aria-label'] === '导入 a'));
find(tree, node => node.props?.['aria-label'] === '导入 b2').props.onChange();
tree = render(whitelist, WhitelistSettings);
find(tree, node => node.type === 'button' && node.props.children === '保存所选会话').props.onClick(); await flush();
assert.equal(whitelistSaves.length, 1);
assert.equal(whitelistSaves[0].args.value.active_account_id, 'B');
assert.deepEqual(whitelistSaves[0].args.value.chat.allowed_usernames.B, ['b', 'b2']);
whitelistSaves[0].resolve({ ...whitelistSaves[0].args.value, user_settings_revision: 12 }); await flush();

// Blur saves serialize using confirmed revision; a rejected save prevents queued replay.
const queueSaves = [];
invoke = (command, args) => {
  if (command === 'app_settings_get') return Promise.resolve({ ...defaultSettings(), user_settings_revision: 15 });
  if (command === 'wechat_scan') return Promise.resolve({ accounts: [] });
  if (command === 'devify_auth_status') return Promise.resolve({ authenticated: false });
  if (command === 'app_settings_save') { const d = deferred(); queueSaves.push({ args, ...d }); return d.promise; }
  throw Error(command);
};
const account = { hooks: [] };
render(account, DevifySettings); await flush();
const accountInput = tree => find(tree, node => node.type === 'input' && node.props['aria-label'] === '服务端地址');
tree = render(account, DevifySettings);
accountInput(tree).props.onChange({ target: { value: 'https://first.example' } });
tree = render(account, DevifySettings); accountInput(tree).props.onBlur(); await flush();
assert.equal(queueSaves.length, 1);
accountInput(tree).props.onChange({ target: { value: 'https://second.example' } });
tree = render(account, DevifySettings); accountInput(tree).props.onBlur(); await flush();
assert.equal(queueSaves.length, 1);
queueSaves[0].resolve({ ...queueSaves[0].args.value, user_settings_revision: 16 }); await flush();
tree = render(account, DevifySettings);
assert.equal(accountInput(tree).props.value, 'https://second.example');
assert.equal(queueSaves.length, 2);
assert.equal(queueSaves[1].args.value.user_settings_revision, 16);
accountInput(tree).props.onChange({ target: { value: 'https://third.example' } });
tree = render(account, DevifySettings); accountInput(tree).props.onBlur(); await flush();
queueSaves[1].reject('SETTINGS_CONFLICT: switched'); await flush();
tree = render(account, DevifySettings);
assert.equal(queueSaves.length, 2);
assert.equal(accountInput(tree).props.value, 'https://third.example');
assert.ok(elements(tree).some(node => node.props?.role === 'alert'));

// Exercise the actual WeChat callbacks with controlled save completion.
const wxSource = fs.readFileSync(new URL('../src/WeChatLocal.tsx', import.meta.url), 'utf8');
const start = wxSource.indexOf('  const rememberAccount = useCallback(');
const end = wxSource.indexOf('  /** Fetch the avatars', start);
const switchStart = wxSource.indexOf('  const switchAccount = useCallback(');
const switchEnd = wxSource.indexOf('  const applyImportResult', switchStart);
assert.ok(start >= 0 && end > start && switchStart >= 0 && switchEnd > switchStart);
const wxBody = wxSource.slice(start, end) + wxSource.slice(switchStart, switchEnd) + '\nreturn { switchAccount };';
const wxCode = ts.transpileModule(wxBody, { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText;
const wxContext = { settingsRef: { current: { ...defaultSettings(), active_account_id: 'old', user_settings_revision: 4 } }, accountSwitchLock: { current: false }, accountId: 'old', avatarAsked: { current: new Set() } };
for (const name of ['setAccountSwitching','setError','setAccountId','setSessions','setSessionsLoaded','setActive','setMessages','setOutcomes','setSelectedMessages','setSelectionAnchor','setWorkGroups','setWorkInsights','setKeyImport','setAvatars']) wxContext[name] = value => { wxContext[name + 'Value'] = value; };
wxContext.loadSessions = async () => [];
wxContext.requireSettingsRevision = requireSettingsRevision;
wxContext.settingsSaveError = settingsSaveError;
wxContext.isSettingsConflict = module.exports.isSettingsConflict;
wxContext.tf = (pattern, ...args) => String(pattern).replace(/\{(\d+)\}/g, (_, index) => String(args[Number(index)] ?? ''));
wxContext.useCallback = fn => fn;
let wxSave = deferred();
wxContext.invoke = (_command, args) => { wxContext.sent = args.value; return wxSave.promise; };
const wx = new Function(...Object.keys(wxContext), wxCode)(...Object.values(wxContext));
let switching = wx.switchAccount('new');
assert.equal(wxContext.setAccountIdValue, undefined);
assert.equal(wxContext.sent.user_settings_revision, 4);
wxSave.reject('SETTINGS_CONFLICT: switched'); await switching;
assert.equal(wxContext.setAccountIdValue, undefined);
assert.match(wxContext.setErrorValue, /账号未切换/);
assert.equal(wxContext.settingsRef.current, null);
// Reopening the page establishes a new context before retrying.
wxContext.settingsRef.current = { ...defaultSettings(), active_account_id: 'old', user_settings_revision: 4 };
wxSave = deferred();
switching = wx.switchAccount('new');
wxSave.resolve({ ...wxContext.sent, user_settings_revision: 5 }); await switching;
assert.equal(wxContext.setAccountIdValue, 'new');
assert.equal(wxContext.settingsRef.current.user_settings_revision, 5);
// Waitlist writes are ordered and distinguish unsaved drafts after a conflict.
const { Connectors } = loadModule('../src/Connectors.tsx');
const connectorLoad = deferred(), connectorSaves = [];
invoke = (command, args) => {
  if (command === 'app_settings_get') return connectorLoad.promise;
  if (command === 'wechat_scan') return Promise.resolve({ accounts: [] });
  if (command === 'whatsapp_status') return Promise.resolve({ installed: true, app_path: null, version: null, data_dir: null, account: null, name: null, sessions: 0, messages: 0, last_message_at: null, error: null });
  if (command === 'wechat_key_status') return Promise.resolve({ keys: [] });
  if (command === 'app_settings_save') { const d = deferred(); connectorSaves.push({ args, ...d }); return d.promise; }
  throw Error(command);
};
const connectors = { hooks: [] };
tree = render(connectors, () => Connectors({ onOpenWechat() {}, onOpenWhatsApp() {} }));
const notify = tree => elements(tree).filter(node => node.type === 'button' && node.props.className?.startsWith('connector-notify'));
assert.ok(notify(tree).every(node => node.props.disabled));
connectorLoad.resolve({ ...defaultSettings(), user_settings_revision: 20 }); await flush();
tree = render(connectors, () => Connectors({ onOpenWechat() {}, onOpenWhatsApp() {} }));
notify(tree)[0].props.onClick(); tree = render(connectors, () => Connectors({ onOpenWechat() {}, onOpenWhatsApp() {} }));
notify(tree)[1].props.onClick(); await flush();
assert.equal(connectorSaves.length, 1);
connectorSaves[0].resolve({ ...connectorSaves[0].args.value, user_settings_revision: 21 }); await flush();
assert.equal(connectorSaves.length, 2);
assert.equal(connectorSaves[1].args.value.user_settings_revision, 21);
assert.deepEqual(connectorSaves[1].args.value.connectors.waitlist, ['邮件', 'Telegram']);
connectorSaves[1].reject('SETTINGS_CONFLICT: changed'); await flush();
tree = render(connectors, () => Connectors({ onOpenWechat() {}, onOpenWhatsApp() {} }));
assert.match(notify(tree)[1].props.children, /未保存/);
assert.ok(elements(tree).some(node => node.props?.role === 'alert'));
// A missing WeChat blocks the wizard and offers the download; an untrusted bundle remains
// usable for read-only import and is only blocked by the invasive capture action.
const wizardButton = tree => find(tree, node => node.type === 'button' && node.props.className === 'connector-primary');
assert.equal(wizardButton(tree).props.disabled, false);
connectorInstall = { detect: { state: 'not_installed', app_path: null, version: null, message: '请先安装微信。' }, checking: false, usable: false, canProceed: false, recheck: async () => {} };
tree = render(connectors, () => Connectors({ onOpenWechat() {}, onOpenWhatsApp() {} }));
assert.equal(wizardButton(tree).props.disabled, true);
assert.ok(elements(tree).some(node => node.type === 'a' && node.props.href === 'https://pc.weixin.qq.com/'));
connectorInstall = { ...connectorInstall, detect: { ...connectorInstall.detect, state: 'untrusted', app_path: '/Applications/WeChat.app' }, canProceed: true };
tree = render(connectors, () => Connectors({ onOpenWechat() {}, onOpenWhatsApp() {} }));
assert.equal(wizardButton(tree).props.disabled, false);
assert.ok(!elements(tree).some(node => node.type === 'a' && node.props.href === 'https://pc.weixin.qq.com/'));
connectorInstall = installed;

// Login gate saves from its loaded context and does not overwrite later typing.
const { LoginGate } = loadModule('../src/LoginGate.tsx');
const gateLoad = deferred(), gateSaves = [];
invoke = (command, args) => {
  if (command === 'app_settings_get') return gateLoad.promise;
  if (command === 'app_settings_save') { const d = deferred(); gateSaves.push({ args, ...d }); return d.promise; }
  throw Error(command);
};
const gate = { hooks: [] };
tree = render(gate, () => LoginGate({ onAuthenticated() {} }));
const gateInput = tree => find(tree, node => node.type === 'input' && node.props.type === 'url');
assert.equal(gateInput(tree).props.disabled, true);
gateLoad.resolve({ ...defaultSettings(), user_settings_revision: 30, devify: { ...defaultSettings().devify, base_url: '' } }); await flush();
tree = render(gate, () => LoginGate({ onAuthenticated() {} }));
gateInput(tree).props.onChange({ target: { value: 'https://first.example' } });
tree = render(gate, () => LoginGate({ onAuthenticated() {} }));
find(tree, node => node.type === 'button' && node.props.children === '保存').props.onClick(); await flush();
assert.equal(gateSaves[0].args.value.user_settings_revision, 30);
gateInput(tree).props.onChange({ target: { value: 'https://later.example' } });
gateSaves[0].resolve({ ...gateSaves[0].args.value, user_settings_revision: 31 }); await flush();
tree = render(gate, () => LoginGate({ onAuthenticated() {} }));
assert.equal(gateInput(tree).props.value, 'https://later.example');
assert.ok(elements(tree).some(node => node.props?.role === 'status' && String(node.props.children).includes('尚未保存')));
console.log('settings revision and draft checks passed');
