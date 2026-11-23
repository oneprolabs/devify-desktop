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
let wizardActionModule;
const installed = { detect: { state: 'installed', app_path: '/Applications/WeChat.app', version: '4.1.2', message: '' }, checking: false, usable: true, canProceed: true, recheck: async () => {} };
let connectorInstall = installed;
let backupRuntime = { paused: false };
const mockedRequire = id => {
  if (id === 'react') return react;
  if (id === 'react/jsx-runtime') return jsx;
  if (id === '@tauri-apps/api/core') return { invoke: (...args) => invoke(...args), isTauri: () => true };
  if (id === '@tauri-apps/api/event') return { listen: async () => () => {} };
  if (id === '@tauri-apps/plugin-deep-link') return { onOpenUrl: async () => () => {} };
  if (id === '../../shared/api/devifyDevice') return { registerDevifyDevice: async () => {} };
  if (['../../../shared/api/desktopInvoke', '../../shared/api/desktopInvoke'].includes(id)) return { desktopInvoke: (...args) => invoke(...args), requestFeedback: () => {} };
  if (['./settingsModel', '../../settings/settingsModel', '../settings/settingsModel'].includes(id)) return settingsModel;
  if (id === '../settings/NetworkSettings') return loadModule('../src/features/settings/NetworkSettings.tsx');
  if (['../../settings/Settings', '../settings/Settings'].includes(id)) return module.exports;
  if (id === './wechatSetupState') return loadModule('../src/features/connectors/wizard/wechatSetupState.ts');
  if (['../../connectors/Permission', '../Permission'].includes(id)) return loadModule('../src/features/connectors/Permission.tsx');
  if (['../../../shared/ui/WizardActionButton', '../../shared/ui/WizardActionButton'].includes(id)) return wizardActionModule ??= loadModule('../src/shared/ui/WizardActionButton.tsx');
  if (id === '../connectors/wizard/WizardSessionPicker') return loadModule('../src/features/connectors/wizard/WizardSessionPicker.tsx');
  if (id === '../connectors/wizard/WizardImportScope') return loadModule('../src/features/connectors/wizard/WizardImportScope.tsx');
  if (id === '../connectors/wizard/ImportHistoryRange') return loadModule('../src/features/connectors/wizard/ImportHistoryRange.tsx');
  if (id === '../../backup/BackupSchedule') return loadModule('../src/features/backup/BackupSchedule.tsx');
  if (id === './WeChatWizardComplete') return loadModule('../src/features/connectors/wizard/WeChatWizardComplete.tsx');
  if (id === '../../shared/state/settingsSaveQueue') return loadModule('../src/shared/state/settingsSaveQueue.ts');
  if (id === '../tasks/taskRuntime') return { useTaskRuntime: () => backupRuntime, refreshTaskRuntime: async () => {} };
  if (id === './BackupRuntimeSwitch') return loadModule('../src/features/backup/BackupRuntimeSwitch.tsx');
  if (['../../../shared/ui/icons', '../../shared/ui/icons'].includes(id)) return { Icons: { check: '✓', clock: '◷' } };
  if (id === '../connectors/AccountPicker') return loadModule('../src/features/connectors/AccountPicker.tsx');
  if (id === '../connectors/Connectors') return loadModule('../src/features/connectors/Connectors.tsx');
  if (['../connectorDetect', '../connectors/connectorDetect', './connectorDetect'].includes(id)) return {
    useConnectorDetect: () => connectorInstall,
    detectTitle: { not_installed: '未检测到微信', untrusted: '检测到的微信不是官方版本' },
    detectTitleFor: (label, state) => ({ not_installed: `未检测到${label}`, untrusted: `检测到的${label}不是官方版本`, installed: `已检测到${label}` })[state],
    currentAccount: (accounts, remembered) => accounts.find(item => item.id === remembered) ?? accounts[0],
  };
  if (id === '../../shared/api/serviceUrl.ts' || id === './serviceUrl') return serviceUrl;
  if (['../../../shared/i18n/locale', '../../shared/i18n/locale', '../i18n/locale'].includes(id)) return {
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
const serviceUrlSource = fs.readFileSync(new URL('../src/shared/api/serviceUrl.ts', import.meta.url), 'utf8');
const serviceUrlCompiled = ts.transpileModule(serviceUrlSource, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
}).outputText;
const serviceUrlModule = { exports: {} };
new Function('require', 'module', 'exports', serviceUrlCompiled)(
  mockedRequire, serviceUrlModule, serviceUrlModule.exports,
);
const serviceUrl = serviceUrlModule.exports;
const settingsModel = loadModule('../src/features/settings/settingsModel.ts');
const source = fs.readFileSync(new URL('../src/features/settings/Settings.tsx', import.meta.url), 'utf8');
const compiled = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2020 } }).outputText;
const module = { exports: {} };
new Function('require', 'module', 'exports', compiled)(mockedRequire, module, module.exports);
const { defaultSettings, mergeSavedSettings, requireSettingsRevision, settingsSaveError } = settingsModel;
const { StrategySettings, DevifySettings } = module.exports;

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
assert.ok(!find(tree, node => node.type?.name === 'WizardSessionPicker').props.items.some(item => item.username === 'a'));
find(tree, node => node.type?.name === 'WizardSessionPicker').props.onToggle('b2');
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
const wxSource = fs.readFileSync(new URL('../src/features/chat/local-reader/WeChatLocal.tsx', import.meta.url), 'utf8');
const start = wxSource.indexOf('  const rememberAccount = useCallback(');
const end = wxSource.indexOf('  /** Fetch the avatars', start);
const switchStart = wxSource.indexOf('  const switchAccount = useCallback(');
const switchEnd = wxSource.indexOf('  const openSession', switchStart);
assert.ok(start >= 0 && end > start && switchStart >= 0 && switchEnd > switchStart);
const wxBody = wxSource.slice(start, end) + wxSource.slice(switchStart, switchEnd) + '\nreturn { switchAccount };';
const wxCode = ts.transpileModule(wxBody, { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText;
const wxContext = { settingsRef: { current: { ...defaultSettings(), active_account_id: 'old', user_settings_revision: 4 } }, accountSwitchLock: { current: false }, accountId: 'old', avatarAsked: { current: new Set() } };
for (const name of ['setAccountSwitching','setError','setAccountId','setSessions','setSessionsLoaded','setActive','setMessages','setSelectedMessages','setSelectionAnchor','setWorkGroups','setWorkInsights','setAvatars']) wxContext[name] = value => { wxContext[name + 'Value'] = value; };
wxContext.resetAccountResult = () => { wxContext.connectionResultsReset = true; };
wxContext.loadSessions = async () => [];
wxContext.requireSettingsRevision = requireSettingsRevision;
wxContext.settingsSaveError = settingsSaveError;
wxContext.isSettingsConflict = settingsModel.isSettingsConflict;
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
assert.equal(wxContext.connectionResultsReset, undefined);
assert.match(wxContext.setErrorValue, /账号未切换/);
assert.equal(wxContext.settingsRef.current, null);
// Reopening the page establishes a new context before retrying.
wxContext.settingsRef.current = { ...defaultSettings(), active_account_id: 'old', user_settings_revision: 4 };
wxSave = deferred();
switching = wx.switchAccount('new');
wxSave.resolve({ ...wxContext.sent, user_settings_revision: 5 }); await switching;
assert.equal(wxContext.setAccountIdValue, 'new');
assert.equal(wxContext.connectionResultsReset, true);
assert.equal(wxContext.settingsRef.current.user_settings_revision, 5);
// Waitlist writes are ordered and distinguish unsaved drafts after a conflict.
const { Connectors } = loadModule('../src/features/connectors/Connectors.tsx');
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
backupRuntime = { ...backupRuntime, sources: [{ account_id: 'A', platform: 'wechat', setup_complete: false, backup_enabled: true }] };
tree = render(connectors, () => Connectors({ onOpenWechat() {}, onOpenWhatsApp() {} }));
assert.ok(elements(tree).some(node => node.props?.role === 'status' && node.props.children === '配置未完成，备份已暂停。'));
assert.ok(elements(tree).some(node => node.type === 'button' && node.props.children === '继续配置'));
backupRuntime = { ...backupRuntime, sources: [{ account_id: 'A', platform: 'wechat', setup_complete: true, backup_enabled: true }] };
tree = render(connectors, () => Connectors({ onOpenWechat() {}, onOpenWhatsApp() {} }));
assert.ok(!elements(tree).some(node => node.props?.children === '配置未完成，备份已暂停。'), 'live completion clears the connector notice');
notify(tree)[0].props.onClick(); tree = render(connectors, () => Connectors({ onOpenWechat() {}, onOpenWhatsApp() {} }));
notify(tree)[1].props.onClick(); await flush();
assert.equal(connectorSaves.length, 1);
connectorSaves[0].resolve({ ...connectorSaves[0].args.value, user_settings_revision: 21 }); await flush();
assert.equal(connectorSaves.length, 2);
assert.equal(connectorSaves[1].args.value.user_settings_revision, 21);
assert.deepEqual(connectorSaves[1].args.value.connectors.waitlist, ['Telegram', 'Slack']);
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
const { LoginGate } = loadModule('../src/features/auth/LoginGate.tsx');
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
const { ChatEntry } = loadModule('../src/features/chat/ChatLanding.tsx');
backupRuntime = { paused: false, sources: [{ account_id: 'locked-account', platform: 'wechat', setup_complete: true }] };
let entryChecks = 0, unlocks = 0;
let entryAccounts = deferred();
invoke = (command) => {
  if (command === 'chat_accounts') { entryChecks += 1; return entryAccounts.promise; }
  if (command === 'app_settings_get') return Promise.resolve(defaultSettings());
  if (command === 'wechat_key_status') return Promise.resolve({ keys: [] });
  if (command === 'chat_sessions') return Promise.reject('未找到解密后的数据库');
  throw Error(command);
};
const entry = { hooks: [] };
const entryProps = { onConnect() {}, onConnectWhatsApp() {}, onImport() {}, onBackup() {}, onSchedule() {}, onReady() {}, onUnlock() { unlocks += 1; } };
render(entry, () => ChatEntry({ ...entryProps, visible: true }));
assert.equal(entryChecks, 1);
render(entry, () => ChatEntry({ ...entryProps, visible: false }));
entryAccounts.resolve([{ id: 'locked-account', source: 'wechat', encrypted_count: 2 }]); await flush();
assert.equal(unlocks, 0, 'a pending chat probe must not redirect after leaving the chat page');
render(entry, () => ChatEntry({ ...entryProps, visible: false, onUnlock() { unlocks += 1; } }));
assert.equal(entryChecks, 1, 'hidden chat pages must not start a new probe on parent renders');
entryAccounts = deferred();
render(entry, () => ChatEntry({ ...entryProps, visible: true }));
entryAccounts.resolve([{ id: 'locked-account', source: 'wechat', encrypted_count: 2 }]); await flush();
assert.equal(unlocks, 1, 'returning to chat still opens setup when the account is locked');
render(entry, () => ChatEntry({ ...entryProps, visible: true })); await flush();
assert.equal(entryChecks, 2, 'stable callbacks must not restart a completed probe');
assert.equal(unlocks, 1);
const unfinishedEntry = { hooks: [] };
let unfinishedReads = 0, unfinishedReady = 0;
backupRuntime = { paused: false, sources: [{ account_id: 'draft@s.whatsapp.net', platform: 'whatsapp', setup_complete: false }] };
invoke = (command) => {
  if (command === 'chat_accounts') return Promise.resolve([{ id: 'draft@s.whatsapp.net', source: 'whatsapp' }]);
  if (command === 'app_settings_get') return Promise.resolve(defaultSettings());
  if (command === 'wechat_key_status') return Promise.resolve({ keys: [] });
  if (command === 'chat_sessions') { unfinishedReads += 1; return Promise.resolve([{ username: 'chat' }]); }
  throw Error(command);
};
const unfinishedProps = { ...entryProps, visible: true, onReady() { unfinishedReady += 1; } };
render(unfinishedEntry, () => ChatEntry(unfinishedProps)); await flush();
assert.equal(unfinishedReads, 0, 'a draft account is not probed for chat records');
assert.equal(unfinishedReady, 0, 'local readability must not skip unfinished setup');
backupRuntime = { ...backupRuntime, sources: [{ account_id: 'draft@s.whatsapp.net', platform: 'whatsapp', setup_complete: true }] };
render(unfinishedEntry, () => ChatEntry(unfinishedProps)); await flush();
assert.equal(unfinishedReady, 1, 'completed setup can enter chat');
const { DiskAccessCard } = loadModule('../src/features/connectors/Permission.tsx');
const { WizardActionButton } = mockedRequire('../../../shared/ui/WizardActionButton');
const previousWindow = globalThis.window, previousDocument = globalThis.document;
globalThis.window = new EventTarget();
globalThis.document = Object.assign(new EventTarget(), { visibilityState: 'visible' });
window.setInterval = () => { throw Error('permission checks must not poll'); };
let permissionChecks = 0, permissionGranted = false, reportedPermission;
invoke = command => {
  if (command === 'wechat_platform_capabilities') return Promise.resolve({ platform: 'macos' });
  if (command === 'perm_status') { permissionChecks += 1; return Promise.resolve({ granted: permissionGranted, state: permissionGranted ? 'granted' : 'denied', location: 'applications' }); }
  throw Error(command);
};
const permission = { hooks: [] };
const permissionProps = { compact: true, onCheck(value) { reportedPermission = value; } };
render(permission, () => DiskAccessCard(permissionProps)); await flush();
assert.equal(permissionChecks, 1);
render(permission, () => DiskAccessCard(permissionProps)); await flush();
assert.equal(permissionChecks, 1, 'parent renders must not recheck permission');
window.dispatchEvent(new Event('focus')); await flush();
assert.equal(permissionChecks, 1, 'returning to the app must not recheck permission');
document.visibilityState = 'hidden';
document.dispatchEvent(new Event('visibilitychange')); await flush();
assert.equal(permissionChecks, 1, 'hiding the app must not trigger a permission read');
document.visibilityState = 'visible';
document.dispatchEvent(new Event('visibilitychange')); await flush();
assert.equal(permissionChecks, 1, 'visibility changes must not recheck permission');
render(permission, () => DiskAccessCard({ ...permissionProps, checkOnEntry: false })); await flush();
assert.equal(permissionChecks, 1, 'leaving the permission step must not recheck');
render(permission, () => DiskAccessCard(permissionProps)); await flush();
assert.equal(permissionChecks, 2, 'reentering the permission step checks once');
tree = render(permission, () => DiskAccessCard(permissionProps));
permissionGranted = true;
find(tree, node => node.type === WizardActionButton && node.props.children === '重新检测').props.onAction(); await flush();
assert.equal(permissionChecks, 3, 'manual recheck remains available');
assert.equal(render(permission, () => DiskAccessCard(permissionProps)), null);
assert.equal(reportedPermission.granted, true);
window.dispatchEvent(new Event('focus'));
document.dispatchEvent(new Event('visibilitychange')); await flush();
assert.equal(permissionChecks, 3, 'focus and visibility must never trigger checks');
for (const hook of permission.hooks) hook?.cleanup?.();
// Returning from System Settings must not toggle the wizard's installation row
// back to "checking". Catalog/chat views keep their existing focus refresh.
const { useConnectorDetect } = loadModule('../src/features/connectors/connectorDetect.ts');
let installationChecks = 0;
let installationResult = deferred();
invoke = command => {
  if (command === 'connector_detect') { installationChecks += 1; return installationResult.promise; }
  throw Error(command);
};
const installation = { hooks: [] };
const detectInWizard = () => useConnectorDetect('wechat', { refreshOnFocus: false });
render(installation, detectInWizard);
assert.equal(installationChecks, 1, 'installation is checked on mount');
installationResult.resolve(installed.detect); await flush();
let installationView = render(installation, detectInWizard);
assert.equal(installationView.checking, false);
window.dispatchEvent(new Event('focus')); await flush();
installationView = render(installation, detectInWizard);
assert.equal(installationChecks, 1, 'returning from System Settings must not check installation in the wizard');
assert.equal(installationView.checking, false, 'the passed installation row must remain stable');
installationResult = deferred();
const recheckInstallation = installationView.recheck();
assert.equal(render(installation, detectInWizard).checking, true);
assert.equal(installationChecks, 2, 'explicit installation checks remain available');
installationResult.resolve(installed.detect); await recheckInstallation;
assert.equal(render(installation, detectInWizard).checking, false);
render(installation, () => useConnectorDetect('wechat'));
assert.equal(installationChecks, 2, 'enabling focus refresh must not itself trigger a check');
window.dispatchEvent(new Event('focus')); await flush();
assert.equal(installationChecks, 3, 'catalog/chat views retain focus refresh');
render(installation, detectInWizard);
window.dispatchEvent(new Event('focus')); await flush();
assert.equal(installationChecks, 3, 'entering the wizard removes the existing focus listener');
for (const hook of installation.hooks) hook?.cleanup?.();
// WhatsApp gets its own wizard, without any WeChat key/capture commands.
const { WhatsAppWizard } = loadModule('../src/features/connectors/wizard/WhatsAppWizard.tsx');
const { WeChatSetupChecklist } = loadModule('../src/features/connectors/wizard/WeChatSetupChecklist.tsx');
const recoveryView = { hooks: [] };
let acceptedOfficial = 0, restores = 0;
const recoveryProps = {
  accountId: '', accounts: [], scanLoaded: true, busy: null, captureBusy: null, windowsBusy: false, switching: false, finishing: false,
  keyedCount: 0, keyStatusLoaded: true, savedKey: null, freshKey: false, decrypted: false,
  capture: { pending: true, stage: 'external_install_conflict', needs_cleanup: true, recovery_error_code: 'external_install_conflict', wechat_official: true },
  install: { ready: true, message: '' }, error: 'scan permission denied', manual: false, permissionPhase: false,
  onAcceptOfficial() { acceptedOfficial += 1; }, onRestore() { restores += 1; },
  onCheckStep: async () => ({ kind: 'ready' }),
};
tree = render(recoveryView, () => WeChatSetupChecklist(recoveryProps));
const acceptButton = find(tree, node => node.type === WizardActionButton && node.props.children === '使用当前微信继续');
assert.equal(acceptButton.props.disabled, false);
assert.equal(acceptButton.props.className, 'wxv-btn primary');
acceptButton.props.onAction(); assert.equal(acceptedOfficial, 1); assert.equal(restores, 0);
assert.ok(!elements(tree).some(node => node.type === WizardActionButton && node.props.children === '重试恢复微信'), 'an official update must not lead to an endless restore retry');
assert.ok(!elements(tree).some(node => node.props?.role === 'alert' && String(node.props.children).startsWith('校验未完成')), 'a scan failure must not be reported as a failed recovery');
tree = render(recoveryView, () => WeChatSetupChecklist({ ...recoveryProps, captureBusy: 'accept-official' }));
assert.equal(find(tree, node => node.type === WizardActionButton && node.props.children === '使用当前微信继续').props.loading, true);
tree = render(recoveryView, () => WeChatSetupChecklist({ ...recoveryProps, recoveryError: 'restore failed', capture: { ...recoveryProps.capture, wechat_official: false } }));
find(tree, node => node.type === WizardActionButton && node.props.children === '重试恢复微信').props.onAction();
assert.equal(restores, 1, 'an unknown or modified signature must keep the guarded restore path');
for (const hook of recoveryView.hooks) hook?.cleanup?.();
const wa = { hooks: [] }, waCalls = [];
let waStatus = { installed: true, version: '2.0', account: 'wa@s.whatsapp.net', name: 'Test', sessions: 2, error: null };
let activation = deferred();
invoke = (command, args) => {
  waCalls.push({ command, args });
  if (command === 'whatsapp_status') return Promise.resolve(waStatus);
  if (command === 'app_settings_get') return Promise.resolve({ ...defaultSettings(), user_settings_revision: 20 });
  if (command === 'chat_account_activate') return activation.promise;
  if (command === 'connector_setup_state') return Promise.resolve({});
  throw Error(`unexpected WhatsApp command ${command}`);
};
let chatsOpened = 0;
const renderWa = () => render(wa, () => WhatsAppWizard({ onBack() {}, onOpenChat() { chatsOpened += 1; } }));
tree = renderWa(); await flush();
assert.equal(waCalls.length, 0, 'opening the introduction must not activate an account or read messages');
find(tree, node => node.type === 'button' && node.props.children === '下一步：系统检查').props.onClick();
tree = renderWa(); await flush(); tree = renderWa();
assert.equal(waCalls.filter(call => call.command === 'whatsapp_status').length, 1);
window.dispatchEvent(new Event('focus')); await flush(); renderWa();
assert.equal(waCalls.length, 1, 'focus must not recheck WhatsApp or advance the wizard');
waStatus = { ...waStatus, error: 'Operation not permitted (os error 1)' };
find(tree, node => node.type === WizardActionButton && node.props.children === '重新检查').props.onAction(); await flush();
tree = renderWa();
assert.equal(find(tree, node => node.type === WizardActionButton && node.props.children === '下一步：选择会话').props.disabled, true);
assert.ok(elements(tree).some(node => node.props?.children === '打开系统设置'));
waStatus = { ...waStatus, error: null };
find(tree, node => node.type === WizardActionButton && node.props.children === '重新检查').props.onAction(); await flush(); tree = renderWa();
find(tree, node => node.type === WizardActionButton && node.props.children === '下一步：选择会话').props.onAction(); await flush();
assert.ok(!elements(renderWa()).some(node => node.type === WhitelistSettings), 'activation must finish before selecting sessions');
activation.reject('account unavailable'); await flush();
assert.ok(elements(renderWa()).some(node => node.props?.role === 'alert'));
assert.equal(chatsOpened, 0, 'activation failure must not open chat');
activation = deferred(); tree = renderWa();
find(tree, node => node.type === WizardActionButton && node.props.children === '下一步：选择会话').props.onAction(); await flush();
activation.resolve(); await flush(); tree = renderWa();
const waSelection = find(tree, node => node.type === WhitelistSettings);
assert.equal(waSelection.props.wizardSource, 'whatsapp', 'the wizard must select only WhatsApp accounts');
waSelection.props.onSaved(0);
assert.ok(elements(renderWa()).some(node => node.type === WhitelistSettings), 'empty selection must not mark setup complete');
await waSelection.props.onSaved(2); tree = renderWa();
const scheduleStep = find(tree, node => node.type?.name === 'BackupSchedule');
await scheduleStep.props.onContinue(); tree = renderWa();
find(tree, node => node.type === 'button' && node.props.children === '进入聊天').props.onClick();
assert.equal(chatsOpened, 1);
assert.ok(waCalls.every(call => !call.command.startsWith('wechat_')), 'WhatsApp must not run WeChat checks or key capture');
for (const hook of wa.hooks) hook?.cleanup?.();

// Resume validates the same local account once, then returns to the saved stage.
const resumeCalls = [];
let resumeStatus = { ...waStatus };
let resumeSettings = { ...defaultSettings(), chat: { ...defaultSettings().chat, allowed_usernames: { [waStatus.account]: ['chat'] } }, user_settings_revision: 30 };
invoke = async (command, args) => {
  resumeCalls.push({ command, args });
  if (command === 'whatsapp_status') return resumeStatus;
  if (command === 'app_settings_get') return resumeSettings;
  if (command === 'chat_account_activate') return null;
  throw Error(command);
};
const resumed = { hooks: [] }, resumeProps = { resume: { accountId: waStatus.account, step: 'backup' }, onBack() {}, onOpenChat() {} };
render(resumed, () => WhatsAppWizard(resumeProps)); await flush();
tree = render(resumed, () => WhatsAppWizard(resumeProps));
assert.ok(elements(tree).some(node => node.type?.name === 'BackupSchedule'), 'continue returns to the saved schedule step');
assert.ok(!elements(tree).some(node => node.props?.children === '开始前，请先确认'));
assert.equal(resumeCalls.filter(call => call.command === 'whatsapp_status').length, 1);
assert.ok(!resumeCalls.some(call => call.command === 'app_settings_save' || call.command === 'connector_setup_state'), 'resuming does not rewrite configuration');
for (const hook of resumed.hooks) hook?.cleanup?.();
resumeSettings = { ...resumeSettings, chat: { ...resumeSettings.chat, allowed_usernames: {} } };
const missingSelection = { hooks: [] };
render(missingSelection, () => WhatsAppWizard(resumeProps)); await flush();
tree = render(missingSelection, () => WhatsAppWizard(resumeProps));
assert.ok(elements(tree).some(node => node.type === WhitelistSettings), 'missing saved selections return to selection instead of skipping it');
for (const hook of missingSelection.hooks) hook?.cleanup?.();
resumeStatus = { ...waStatus, account: 'other@s.whatsapp.net' };
resumeCalls.length = 0;
const changedAccount = { hooks: [] };
render(changedAccount, () => WhatsAppWizard(resumeProps)); await flush();
tree = render(changedAccount, () => WhatsAppWizard(resumeProps)); await flush();
assert.ok(!resumeCalls.some(call => call.command === 'chat_account_activate'), 'resume never activates a different WhatsApp account');
assert.ok(elements(tree).some(node => node.props?.role === 'alert'));
for (const hook of changedAccount.hooks) hook?.cleanup?.();

// Mixed-source account lists must never save a WeChat whitelist from the WA wizard.
const waWhitelist = { hooks: [] }, waCatalogAccounts = [];
invoke = (command, args) => {
  if (command === 'app_settings_get') return Promise.resolve({ ...defaultSettings(), active_account_id: 'wx', user_settings_revision: 22 });
  if (command === 'chat_accounts') return Promise.resolve([{ id: 'wx', source: 'wechat' }, { id: 'wa@s.whatsapp.net', source: 'whatsapp' }]);
  if (command === 'devify_upload_identity' || command === 'wechat_account_profile') return Promise.resolve(null);
  if (command === 'wechat_session_catalog') { waCatalogAccounts.push(args.accountId); return Promise.resolve([session('wa-chat')]); }
  throw Error(command);
};
const renderWaWhitelist = () => render(waWhitelist, () => WhitelistSettings({ wizard: true, wizardSource: 'whatsapp' }));
renderWaWhitelist(); await flush(); renderWaWhitelist(); await flush(); tree = renderWaWhitelist();
assert.deepEqual(waCatalogAccounts, ['wa@s.whatsapp.net']);
assert.equal(find(tree, node => node.type?.name === 'WizardSessionPicker').props.selected.has('wa-chat'), false);
assert.equal(find(tree, node => node.type?.name === 'WizardSessionPicker').props.sourceName, 'WhatsApp');
for (const hook of waWhitelist.hooks) hook?.cleanup?.();
// Busy feedback must paint before IPC; clicks before React rerenders still invoke once.
const frames = [];
window.requestAnimationFrame = callback => { frames.push(callback); return frames.length; };
window.setTimeout = () => 1;
window.clearTimeout = () => {};
const paint = async () => { frames.splice(0).forEach(callback => callback()); await flush(); frames.splice(0).forEach(callback => callback()); await flush(); };
const action = { hooks: [] };
let actionCalls = 0, task = deferred();
const actionProps = { children: '准备微信', className: 'wxv-btn primary', busyLabel: '正在准备微信…', onAction() { actionCalls += 1; return task.promise; } };
const renderAction = () => render(action, () => WizardActionButton(actionProps));
tree = renderAction();
const clickAction = find(tree, node => node.type === 'button').props.onClick;
const domButton = { disabled: false };
const firstClick = clickAction({ currentTarget: domButton });
clickAction({ currentTarget: domButton });
assert.equal(domButton.disabled, true, 'disable the DOM button immediately');
assert.equal(actionCalls, 0, 'do not start native IPC before loading feedback can paint');
tree = renderAction();
assert.equal(find(tree, node => node.type === 'button').props['aria-busy'], true);
assert.ok(elements(tree).some(node => node.type === 'svg' && node.props.className === 'wizard-button-spinner'));
await paint(); assert.equal(actionCalls, 1, 'rapid repeated clicks start only one task');
task.reject('test failure'); await firstClick;
tree = renderAction();
assert.equal(find(tree, node => node.type === 'button').props['aria-busy'], false);
assert.ok(elements(tree).some(node => node.props?.role === 'alert'), 'a rejected action reports an error and releases the button');
task = deferred();
const retry = find(tree, node => node.type === 'button').props.onClick({ currentTarget: { disabled: false } });
await paint(); assert.equal(actionCalls, 2, 'retry is available after failure');
task.resolve(); await retry;
tree = renderAction();
const abandoned = find(tree, node => node.type === 'button').props.onClick({ currentTarget: { disabled: false } });
for (const hook of action.hooks) hook?.cleanup?.();
await paint(); await abandoned;
assert.equal(actionCalls, 2, 'navigating away before IPC starts cancels the queued action');

const progress = { hooks: [] };
let restarts = 0;
const progressProps = { ...recoveryProps, error: '', accountId: 'A', accounts: [{ id: 'A' }], capture: { ...recoveryProps.capture, pending: false, needs_cleanup: false, recovery_error_code: null, stage: 'idle' }, captureBusy: 'start', onRestartConnection() { restarts += 1; } };
tree = render(progress, () => WeChatSetupChecklist(progressProps));
assert.ok(elements(tree).some(node => node.props?.role === 'status' && String(node.props.children).startsWith('正在处理连接结果')));
tree = render(progress, () => WeChatSetupChecklist({ ...progressProps, captureBusy: null }));
assert.ok(elements(tree).some(node => node.type === WizardActionButton && node.props.children === '重新检查连接结果'), 'missing results after completion must offer an explicit check rather than spin forever');
tree = render(progress, () => WeChatSetupChecklist({ ...progressProps, captureBusy: null, error: 'capture failed' }));
assert.ok(elements(tree).some(node => node.props?.role === 'alert' && Array.isArray(node.props.children) && node.props.children[0] === '本次连接未完成。'));
await find(tree, node => node.type === WizardActionButton && node.props.children === '重新连接').props.onAction();
assert.equal(restarts, 1, 'failure stays in place until the user explicitly restarts');
for (const hook of progress.hooks) hook?.cleanup?.();

// The real checklist must stop before IPC, preserve its visible step on a failed
// probe, and wait for an explicit return/retry rather than reset progress.
const guardView = { hooks: [] };
let opens = 0, guardCalls = 0, stepProbe = deferred();
const guardedProps = {
  ...progressProps, captureBusy: null, onOpen() { opens += 1; },
  capture: { ...progressProps.capture, pending: true, needs_cleanup: true, stage: 'awaiting_manual_launch', transaction_id: 'one' },
  onCheckStep: () => { guardCalls += 1; return stepProbe.promise; },
};
tree = render(guardView, () => WeChatSetupChecklist(guardedProps));
find(tree, node => node.type?.name === 'DiskAccessCard').props.onCheck({ granted: true, platform: 'macos', error: '' });
tree = render(guardView, () => WeChatSetupChecklist(guardedProps));
const opening = find(tree, node => node.type === WizardActionButton && node.props.children === '打开微信').props.onAction();
await find(tree, node => node.type === WizardActionButton && node.props.children === '打开微信').props.onAction();
assert.equal(guardCalls, 1, 'one shared guard lock also rejects clicks before React rerenders');
assert.equal(opens, 0, 'no native action starts while its prerequisites are being read');
tree = render(guardView, () => WeChatSetupChecklist(guardedProps));
assert.equal(find(tree, node => node.type === WizardActionButton && node.props.children === '打开微信').props.busyLabel, '正在检查前置条件…');
stepProbe.resolve({ kind: 'missing', message: '准备结果缺失', target: 'prepare' });
await opening;
const vanishedProps = { ...guardedProps, capture: progressProps.capture };
tree = render(guardView, () => WeChatSetupChecklist(vanishedProps));
assert.ok(elements(tree).some(node => node.props?.role === 'alert' && node.props.children === '准备结果缺失'));
const activeRow = find(tree, node => node.props?.['aria-current'] === 'step');
assert.ok(elements(activeRow).some(node => node.type === 'strong' && node.props.children === '登录微信'), 'a missing result must not silently jump back');
assert.equal(opens, 0);
find(tree, node => node.type === 'button' && Array.isArray(node.props.children) && node.props.children.join('') === '返回准备微信').props.onClick();
tree = render(guardView, () => WeChatSetupChecklist(vanishedProps));
assert.ok(elements(find(tree, node => node.props?.['aria-current'] === 'step')).some(node => node.type === 'strong' && node.props.children === '准备微信'));
assert.equal(opens, 0, 'returning to an earlier step never replays its action');
for (const hook of guardView.hooks) hook?.cleanup?.();

const retryView = { hooks: [] };
let probeResult = { kind: 'check-failed', message: '检查失败，进度保留' };
const retryProps = { ...guardedProps, onCheckStep: async () => probeResult };
tree = render(retryView, () => WeChatSetupChecklist(retryProps));
find(tree, node => node.type?.name === 'DiskAccessCard').props.onCheck({ granted: true, platform: 'macos', error: '' });
tree = render(retryView, () => WeChatSetupChecklist(retryProps));
await find(tree, node => node.type === WizardActionButton && node.props.children === '打开微信').props.onAction();
tree = render(retryView, () => WeChatSetupChecklist(retryProps));
assert.ok(!elements(tree).some(node => node.type === 'button' && String(node.props.children).includes('返回准备微信')), 'an unavailable probe does not prove preparation was lost');
probeResult = { kind: 'ready' };
await find(tree, node => node.type === WizardActionButton && node.props.children === '重新检查').props.onAction();
tree = render(retryView, () => WeChatSetupChecklist(retryProps));
assert.equal(opens, 0, 'rechecking alone must not launch WeChat');
await find(tree, node => node.type === WizardActionButton && node.props.children === '打开微信').props.onAction();
assert.equal(opens, 1, 'the user can run the original action after a successful recheck');
let confirms = 0;
tree = render(retryView, () => WeChatSetupChecklist({ ...retryProps, onConfirmLogin: () => {
  confirms += 1;
  return { kind: 'missing', message: '底层确认准备结果已失效', target: 'prepare' };
} }));
await find(tree, node => node.type === WizardActionButton && node.props.children === '我已登录，继续').props.onAction();
tree = render(retryView, () => WeChatSetupChecklist(retryProps));
assert.equal(confirms, 1);
assert.ok(elements(tree).some(node => node.props?.role === 'alert' && node.props.children === '底层确认准备结果已失效'), 'native validation after a successful UI check uses the same explicit recovery guidance');
for (const hook of retryView.hooks) hook?.cleanup?.();
for (const abandon of ['account-change', 'unmount']) {
  const view = { hooks: [] }, pendingProbe = deferred();
  const props = { ...guardedProps, onCheckStep: () => pendingProbe.promise };
  tree = render(view, () => WeChatSetupChecklist(props));
  find(tree, node => node.type?.name === 'DiskAccessCard').props.onCheck({ granted: true, platform: 'macos', error: '' });
  tree = render(view, () => WeChatSetupChecklist(props));
  const pending = find(tree, node => node.type === WizardActionButton && node.props.children === '打开微信').props.onAction();
  if (abandon === 'account-change') render(view, () => WeChatSetupChecklist({ ...props, accountId: 'B', accounts: [{ id: 'B' }] }));
  else for (const hook of view.hooks) hook?.cleanup?.();
  pendingProbe.resolve({ kind: 'ready' }); await pending;
  assert.equal(opens, 1, `${abandon} while checking must prevent the old native action from starting`);
  if (abandon === 'account-change') for (const hook of view.hooks) hook?.cleanup?.();
}
const reusedView = { hooks: [] };
const reusedProps = { ...progressProps, captureBusy: null, keyedCount: 2, decrypted: true, error: '' };
tree = render(reusedView, () => WeChatSetupChecklist(reusedProps));
find(tree, node => node.type?.name === 'DiskAccessCard').props.onCheck({ granted: true, platform: 'macos', error: '' });
tree = render(reusedView, () => WeChatSetupChecklist(reusedProps));
const stepsWithSavedKeys = find(tree, node => node.type === 'ol' && node.props['aria-label'] === '微信操作步骤');
for (const title of ['准备微信', '登录微信', '授权连接', '退出微信账号', '重新登录微信', '校验连接', '读取聊天记录']) {
  assert.ok(elements(stepsWithSavedKeys).some(node => node.type === 'strong' && node.props.children === title), `saved results must preserve the ${title} step`);
}
assert.equal(elements(stepsWithSavedKeys).filter(node => node.props?.className === 'wx-setup-status' && node.props.children.at(-1) === '已完成').length, 7, 'validated saved results use the same completed status as a new connection');
tree = render(reusedView, () => WeChatSetupChecklist({ ...reusedProps, keyedCount: 0, freshKey: true, savedKey: { exists: true, secure: true, modified_at: 1 }, error: '等待文件校验' }));
assert.ok(elements(tree).some(node => node.props?.role === 'status' && node.props.children === '检测到已保存的连接文件，将先校验是否适用于当前账号。'));
const unverifiedSteps = find(tree, node => node.type === 'ol' && node.props['aria-label'] === '微信操作步骤');
assert.equal(elements(unverifiedSteps).filter(node => node.props?.className === 'wx-setup-status' && node.props.children.at(-1) === '未完成').length, 7, 'a saved file alone leaves the connection steps unfinished until validation');
for (const hook of reusedView.hooks) hook?.cleanup?.();
// Completion only opens a verified restored official app, and retries do not
// bypass a pending recovery or launch failure.
const { WeChatWizardComplete } = loadModule('../src/features/connectors/wizard/WeChatWizardComplete.tsx');
const complete = { hooks: [] };
let officialOpens = 0, officialState = { pending: true, needs_cleanup: true, recovery_error_code: null, wechat_official: true }, launchWorks = false;
invoke = async command => {
  if (command === 'app_settings_get') return { ...defaultSettings(), active_account_id: 'A', chat: { ...defaultSettings().chat, allowed_usernames: { A: ['a', 'b'] } } };
  if (command === 'app_info') return { platform: 'macos' };
  if (command === 'wechat_capture_status') return officialState;
  if (command === 'wechat_capture_open') { officialOpens += 1; return { launched: launchWorks }; }
  throw Error(command);
};
const completeProps = { edit: false, onFinish() {}, onBack() {} };
render(complete, () => WeChatWizardComplete(completeProps)); await flush();
tree = render(complete, () => WeChatWizardComplete(completeProps));
assert.ok(elements(tree).some(node => node.type === 'li' && Array.isArray(node.props.children) && node.props.children.join('') === '已选择 2 个会话导入'));
await find(tree, node => node.type === WizardActionButton && node.props.children === '打开微信').props.onAction();
assert.equal(officialOpens, 0, 'a pending recovery cannot launch a temporary app from completion');
officialState = { ...officialState, pending: false, needs_cleanup: false, wechat_official: false };
tree = render(complete, () => WeChatWizardComplete(completeProps));
await find(tree, node => node.type === WizardActionButton && node.props.children === '重新打开微信').props.onAction();
assert.equal(officialOpens, 0, 'an unverified signature is not accepted as official');
officialState.wechat_official = true;
tree = render(complete, () => WeChatWizardComplete(completeProps));
await find(tree, node => node.type === WizardActionButton && node.props.children === '重新打开微信').props.onAction();
tree = render(complete, () => WeChatWizardComplete(completeProps));
assert.ok(elements(tree).some(node => node.type === 'p' && String(node.props.children).startsWith('微信未能打开')));
launchWorks = true;
await find(tree, node => node.type === WizardActionButton && node.props.children === '重新打开微信').props.onAction();
tree = render(complete, () => WeChatWizardComplete(completeProps));
assert.equal(officialOpens, 2);
assert.ok(elements(tree).some(node => node.type === 'p' && node.props.children === '已请求打开官方微信，请在微信中确认登录。'));
for (const hook of complete.hooks) hook?.cleanup?.();
const { BackupSchedule } = loadModule('../src/features/backup/BackupSchedule.tsx');
const backupView = { hooks: [] }, backupWrites = [];
let backupNext = 0;
const backupInitial = { ...defaultSettings(), active_account_id: 'A', user_settings_revision: 100, sync: { ...defaultSettings().sync, enabled: true } };
invoke = (command, args) => {
  if (command === 'app_settings_get') return Promise.resolve(backupInitial);
  if (command === 'chat_accounts') return Promise.resolve([{ id: 'A', source: 'wechat' }]);
  if (command === 'app_settings_save') { const pending = deferred(); backupWrites.push({ args, ...pending }); return pending.promise; }
  throw Error(command);
};
const backupProps = { wizard: true, onContinue() { backupNext += 1; }, onBack() {} };
render(backupView, () => BackupSchedule(backupProps)); await flush();
tree = render(backupView, () => BackupSchedule(backupProps));
assert.ok(!elements(tree).some(node => node.type?.name === 'Pills'), 'wizard reuses the shared plan until editing is explicitly requested');
assert.equal(backupWrites.length, 0, 'viewing the shared plan must not write defaults');
await find(tree, node => node.type === WizardActionButton && node.props.children === '沿用当前设置并继续').props.onAction();
assert.equal(backupNext, 1);
assert.equal(backupWrites.length, 0, 'continuing with the existing plan must not rewrite shared settings');
backupNext = 0;
find(tree, node => node.type === 'button' && node.props.children === '修改统一设置').props.onClick();
tree = render(backupView, () => BackupSchedule(backupProps));
assert.equal(backupWrites.length, 0, 'opening the shared editor must not save');
assert.ok(elements(tree).some(node => node.props.children === '以下修改会立即保存，并应用于所有已连接的平台。'));
let intervals = find(tree, node => node.type?.name === 'Pills' && node.props.label === '常用间隔');
assert.equal(intervals.props.options[0].id, 300);
assert.equal(intervals.props.options.at(-1).id, 'custom');
assert.ok(!elements(tree).some(node => node.type?.name === 'Row' && node.props.label === '自定义间隔'));
const writesBeforeCustom = backupWrites.length;
intervals.props.onChange('custom');
tree = render(backupView, () => BackupSchedule(backupProps));
assert.ok(elements(tree).some(node => node.type?.name === 'Row' && node.props.label === '自定义间隔'));
assert.equal(backupWrites.length, writesBeforeCustom, 'opening the custom editor must not save a changed interval');
find(tree, node => node.type?.name === 'Pills' && node.props.label === '常用间隔').props.onChange(3600); await flush();
tree = render(backupView, () => BackupSchedule(backupProps));
assert.ok(!elements(tree).some(node => node.type?.name === 'Row' && node.props.label === '自定义间隔'));
let continueBackup = find(tree, node => node.type === WizardActionButton && node.props.children === '下一步');
assert.equal(continueBackup.props.disabled, true);
assert.equal(continueBackup.props.loading, true, 'next remains busy until the save is acknowledged');
assert.equal(backupWrites[0].args.value.user_settings_revision, 100);
backupWrites[0].resolve({ ...backupWrites[0].args.value, user_settings_revision: 101 }); await flush();
tree = render(backupView, () => BackupSchedule(backupProps));
continueBackup = find(tree, node => node.type === WizardActionButton && node.props.children === '下一步');
assert.equal(continueBackup.props.disabled, false);
find(tree, node => node.type?.name === 'Pills' && node.props.label === '常用间隔').props.onChange(300); await flush();
backupWrites[1].reject('SETTINGS_CONFLICT: another editor'); await flush();
tree = render(backupView, () => BackupSchedule(backupProps));
assert.equal(find(tree, node => node.type === WizardActionButton && node.props.children === '下一步').props.disabled, true, 'an unconfirmed/conflicting save cannot enter completion');
assert.equal(backupNext, 0);
assert.ok(elements(tree).some(node => node.props?.role === 'alert'));
for (const hook of backupView.hooks) hook?.cleanup?.();
const scheduleView = { hooks: [] }, scheduleCalls = [];
let finishPause, failPause = false;
invoke = (command, args) => {
  scheduleCalls.push({ command, args });
  if (command === 'app_settings_get') return Promise.resolve({ ...backupInitial, chat: { ...backupInitial.chat, sync_interval_seconds: 60 } });
  if (command === 'chat_accounts') return Promise.resolve([{ id: 'A', source: 'wechat' }]);
  if (command === 'task_runtime_pause') return new Promise((resolve, reject) => { finishPause = () => { if (failPause) reject('pause failed'); else { backupRuntime = { ...backupRuntime, paused: args.paused }; resolve(); } }; });
  throw Error(command);
};
render(scheduleView, () => BackupSchedule({})); await flush();
tree = render(scheduleView, () => BackupSchedule({}));
assert.ok(!elements(tree).some(node => node.props?.['aria-expanded'] !== undefined), 'the schedule has no collapse action');
assert.ok(!elements(tree).some(node => node.type?.name === 'Row' && node.props.label === '下次备份'));
assert.ok(elements(tree).some(node => node.type?.name === 'Row' && node.props.label === '自定义间隔'), 'existing non-preset intervals remain visible and unchanged');
const backupControl = find(tree, node => node.type?.name === 'BackupRuntimeSwitch');
const controlView = { hooks: [] };
const renderControl = () => render(controlView, () => backupControl.type(backupControl.props));
let controlTree = renderControl();
let backupSwitch = find(controlTree, node => node.props?.role === 'switch');
assert.equal(backupSwitch.props['aria-checked'], true);
backupSwitch.props.onClick(); backupSwitch.props.onClick(); await flush();
assert.deepEqual(scheduleCalls.filter(call => call.command === 'task_runtime_pause'), [{ command: 'task_runtime_pause', args: { paused: true } }]);
controlTree = renderControl();
assert.equal(find(controlTree, node => node.props?.role === 'switch').props['aria-busy'], true);
assert.equal(find(controlTree, node => node.props?.role === 'switch').props.disabled, true);
assert.ok(!elements(controlTree).some(node => node.props?.children === '正在保存…'), 'a fast toggle does not flash loading copy');
finishPause(); await flush();
controlTree = renderControl();
assert.equal(find(controlTree, node => node.props?.role === 'switch').props['aria-checked'], false);
tree = render(scheduleView, () => BackupSchedule({}));
assert.ok(!elements(tree).some(node => node.type === 'fieldset' && node.props.disabled), 'pausing must leave schedule settings editable');
backupRuntime = { paused: false };
controlTree = renderControl();
assert.equal(find(controlTree, node => node.props?.role === 'switch').props['aria-checked'], true, 'the task page runtime state is reflected without a separate settings toggle');
failPause = true;
find(controlTree, node => node.props?.role === 'switch').props.onClick(); finishPause(); await flush();
controlTree = renderControl();
assert.equal(find(controlTree, node => node.props?.role === 'switch').props['aria-checked'], true, 'failure must retain the confirmed state');
assert.equal(find(controlTree, node => node.props?.role === 'switch').props.disabled, false);
tree = render(scheduleView, () => BackupSchedule({}));
assert.ok(elements(tree).some(node => String(node.props?.children).includes('pause failed')));
assert.ok(!scheduleCalls.some(call => call.command === 'app_settings_save'), 'pause uses the task action without rewriting backup configuration');
for (const hook of controlView.hooks) hook?.cleanup?.();
for (const hook of scheduleView.hooks) hook?.cleanup?.();
const { WizardSessionPicker } = loadModule('../src/features/connectors/wizard/WizardSessionPicker.tsx');
const picker = { hooks: [] }, bulkSelections = [];
const pickerProps = { items: Array.from({ length: 63 }, (_, i) => ({ username: `s${i}`, display: `会话 ${i}`, is_group: false, last_time: null })), total: 63, groupCount: 0, selected: new Set(), selectedCount: 0, kind: 'all', onKind() {}, search: '', onSearch() {}, loading: false, disabled: false, accountId: 'A', onToggle() {}, onBulk(mode, ids) { bulkSelections.push({ mode, ids }); }, focused: '', onFocus() {}, activity: () => '无记录', historyEditor: null };
tree = render(picker, () => WizardSessionPicker(pickerProps));
find(tree, node => node.props?.['aria-label'] === '选择本页全部会话').props.onChange();
assert.deepEqual(bulkSelections[0].ids, pickerProps.items.slice(0, 20).map(item => item.username));
find(tree, node => node.props?.['aria-label'] === '下一页').props.onClick();
tree = render(picker, () => WizardSessionPicker(pickerProps));
find(tree, node => node.props?.['aria-label'] === '选择本页全部会话').props.onChange();
assert.deepEqual(bulkSelections[1].ids, pickerProps.items.slice(20, 40).map(item => item.username), 'page selection must not touch off-page conversations');
for (const hook of picker.hooks) hook?.cleanup?.();

// The default V7 screen is the scope form. The table opens only on an explicit choice.
const { WizardImportScope } = loadModule('../src/features/connectors/wizard/WizardImportScope.tsx');
const scopeView = { hooks: [] }, scopeChanges = [];
let clearedScopeIssue = 0;
const scopeProps = { catalog: [{ username: 'd', is_group: false }, { username: 'g', is_group: true }], selected: new Set(['d', 'g']), loading: false, disabled: false, accountId: 'A', onScope: (...args) => scopeChanges.push(args), selectionIssue: null, onClearIssue: () => clearedScopeIssue++, children: jsx.jsx('table', { 'aria-label': 'custom table' }) };
tree = render(scopeView, () => WizardImportScope(scopeProps));
assert.ok(!elements(tree).some(node => node.type === 'table'), 'the default screen must not expand the custom table');
assert.equal(find(tree, node => node.type === 'button' && node.props.children === '私聊').props['aria-pressed'], true);
assert.equal(find(tree, node => node.type === 'button' && node.props.children === '自动导入').props.disabled, true, 'unsupported future-conversation rules cannot appear functional');
find(tree, node => node.type === 'button' && node.props.children === '自定义').props.onClick();
tree = render(scopeView, () => WizardImportScope(scopeProps));
assert.ok(elements(tree).some(node => node.type === 'table'));
assert.ok(elements(tree).some(node => node.props?.['aria-label'] === '新增会话的导入方式'), 'custom selection must retain the new-conversation options');
assert.ok(elements(tree).some(node => node.props?.role === 'status'), 'the import estimate stays visible while editing custom selections');
find(tree, node => node.type === 'button' && node.props.children === '群聊').props.onClick();
tree = render(scopeView, () => WizardImportScope(scopeProps));
assert.deepEqual(scopeChanges, [[true, false]]);
assert.ok(!elements(tree).some(node => node.type === 'table'));
scopeProps.selectionIssue = { attempted: 1001 };
tree = render(scopeView, () => WizardImportScope(scopeProps));
assert.equal(elements(tree).filter(node => node.props?.role === 'alert').length, 1, 'one contextual limit notice, without a second page-level error');
const inlineIssue = find(tree, node => node.props?.role === 'alert');
assert.ok(elements(inlineIssue).some(node => node.type === 'strong' && node.props.children === '本次选择未生效'));
const clearBefore = clearedScopeIssue;
find(inlineIssue, node => node.type === 'button' && node.props.children === '自定义选择').props.onClick();
tree = render(scopeView, () => WizardImportScope(scopeProps));
assert.ok(elements(tree).some(node => node.type === 'table'), 'the notice action opens the existing custom picker');
assert.equal(clearedScopeIssue, clearBefore + 1);
for (const hook of scopeView.hooks) hook?.cleanup?.();

// Scope controls change the real allowlist and preserve saved choices. Never write on entry.
let initialAllowlist = undefined, scopeCatalog = [session('d'), { ...session('g'), is_group: true }], scopeSaved = [];
invoke = async (command, args) => {
  if (command === 'app_settings_get') return { ...defaultSettings(), user_settings_revision: 120, active_account_id: 'A', chat: { ...defaultSettings().chat, allowed_usernames: initialAllowlist === undefined ? {} : { A: initialAllowlist } } };
  if (command === 'chat_accounts') return [{ id: 'A', source: 'wechat' }];
  if (command === 'devify_upload_identity' || command === 'wechat_account_profile') return null;
  if (command === 'wechat_session_catalog') return scopeCatalog;
  if (command === 'app_settings_save') { scopeSaved.push(args.value); return { ...args.value, user_settings_revision: 121 }; }
  throw Error(command);
};
const makeScopeView = async () => {
  const view = { hooks: [] }, draw = () => render(view, () => WhitelistSettings({ wizard: true, wizardSource: 'wechat' }));
  for (let i = 0; i < 4; i++) { draw(); await flush(); }
  return { view, draw };
};
let scoped = await makeScopeView();
let scopeControl = find(scoped.draw(), node => node.type?.name === 'WizardImportScope');
assert.deepEqual([...scopeControl.props.selected], ['d', 'g']);
assert.equal(scopeSaved.length, 0, 'default choices must not save until Next');
scopeControl.props.onScope(true, false);
tree = scoped.draw();
assert.deepEqual([...find(tree, node => node.type?.name === 'WizardImportScope').props.selected], ['d']);
find(tree, node => node.type === 'button' && node.props.children === '下一步').props.onClick(); await flush();
assert.deepEqual(scopeSaved[0].chat.allowed_usernames.A, ['d']);
for (const hook of scoped.view.hooks) hook?.cleanup?.();
initialAllowlist = ['g'];
scoped = await makeScopeView();
assert.deepEqual([...find(scoped.draw(), node => node.type?.name === 'WizardImportScope').props.selected], ['g'], 'revisiting must preserve the saved subset');
for (const hook of scoped.view.hooks) hook?.cleanup?.();
initialAllowlist = undefined;
scopeCatalog = Array.from({ length: 1001 }, (_, i) => session(`s${i}`));
scoped = await makeScopeView();
scopeControl = find(scoped.draw(), node => node.type?.name === 'WizardImportScope');
assert.equal(scopeControl.props.selected.size, 0, 'never silently truncate a larger catalog to the cap');
scopeControl.props.onScope(false, true);
tree = scoped.draw();
assert.equal(find(tree, node => node.type?.name === 'WizardImportScope').props.selected.size, 0);
assert.deepEqual(find(tree, node => node.type?.name === 'WizardImportScope').props.selectionIssue, { attempted: 1001 });
assert.ok(!elements(tree).some(node => node.props?.className === 'error'), 'selection limits must not use the page-level save-failure banner');
find(tree, node => node.type?.name === 'WizardImportScope').props.onClearIssue();
tree = scoped.draw();
assert.equal(find(tree, node => node.type?.name === 'WizardImportScope').props.selectionIssue, null);
assert.equal(find(tree, node => node.type?.name === 'WizardImportScope').props.selected.size, 0, 'opening the picker must preserve the prior selection');
for (const hook of scoped.view.hooks) hook?.cleanup?.();

// Import management shares both editors, scopes accounts by platform, and keeps drafts
// when changing platforms. Opening a page or changing platforms must never auto-save.
const management = { hooks: [] }, managementWrites = [];
invoke = async (command, args) => {
  if (command === 'app_settings_get') return { ...defaultSettings(), user_settings_revision: 130, active_account_id: 'A', chat: { ...defaultSettings().chat, allowed_usernames: { A: ['d'], WA: ['w'] } } };
  if (command === 'chat_accounts') return [{ id: 'A', source: 'wechat' }, { id: 'WA', source: 'whatsapp' }];
  if (command === 'devify_upload_identity' || command === 'wechat_account_profile') return null;
  if (command === 'wechat_session_catalog') return args.accountId === 'WA' ? [session('w')] : [session('d'), session('x')];
  if (command === 'app_settings_save') { managementWrites.push(args.value); return { ...args.value, user_settings_revision: 131 }; }
  throw Error(command);
};
const drawManagement = () => render(management, WhitelistSettings);
for (let i = 0; i < 4; i++) { drawManagement(); await flush(); }
tree = drawManagement();
assert.ok(elements(tree).some(node => node.type?.name === 'WizardImportScope'));
assert.ok(elements(tree).some(node => node.type?.name === 'WizardSessionPicker'));
find(tree, node => node.type?.name === 'WizardImportScope').props.onScope(false, true);
tree = drawManagement();
find(tree, node => node.type?.name === 'WizardSessionPicker').props.onSearch('x');
tree = drawManagement();
find(tree, node => node.props?.className === 'imports-platform-head' && elements(node).some(child => child.type === 'strong' && child.props.children === 'WhatsApp')).props.onClick();
for (let i = 0; i < 3; i++) { drawManagement(); await flush(); }
tree = drawManagement();
let managementPicker = find(tree, node => node.type?.name === 'WizardSessionPicker');
assert.equal(managementPicker.props.sourceName, 'WhatsApp');
assert.equal(managementPicker.props.search, '', 'platform switches clear filters from the previous platform');
assert.deepEqual(managementPicker.props.items.map(item => item.username), ['w']);
assert.equal(managementWrites.length, 0);
find(tree, node => node.type?.name === 'WizardImportScope').props.onScope(false, false);
tree = drawManagement();
const saveManagement = find(tree, node => node.type === 'button' && node.props.children === '保存所选会话');
assert.equal(saveManagement.props.disabled, false, 'management can explicitly stop importing all sessions, unlike initial setup');
saveManagement.props.onClick(); await flush();
assert.deepEqual(managementWrites[0].chat.allowed_usernames.A, ['d', 'x']);
assert.deepEqual(managementWrites[0].chat.allowed_usernames.WA, []);
assert.equal(managementWrites[0].active_account_id, 'WA');
tree = drawManagement();
assert.ok(elements(tree).some(node => node.props?.className === 'imports-save-status' && node.props.role === 'status'), 'save feedback stays in the action row');
assert.ok(!elements(tree).some(node => node.props?.className === 'notice'), 'no full-width saved banner');
find(tree, node => node.type?.name === 'WizardImportScope').props.onScope(false, true);
tree = drawManagement();
assert.ok(!elements(tree).some(node => node.props?.className === 'imports-save-status'), 'a new edit clears the previous saved confirmation');
for (const hook of management.hooks) hook?.cleanup?.();

// Initial history ranges share the same editor across quick scopes and the custom
// table. Defaults are seven local calendar days; explicit saved ranges stay intact.
const historyModule = loadModule('../src/features/connectors/wizard/ImportHistoryRange.tsx');
const { ImportHistoryRange, presetHistoryRange, historyChoiceForRange, historyDateText, historyDateSeconds } = historyModule;
const historyNow = new Date(2026, 0, 3, 15, 30);
assert.equal(historyDateText(presetHistoryRange('7', historyNow).since), '2025-12-28');
assert.equal(historyDateText(presetHistoryRange('30', historyNow).since), '2025-12-05');
assert.equal(historyDateText(presetHistoryRange('90', historyNow).since), '2025-10-06');
assert.deepEqual(presetHistoryRange('all'), { since: null, until: null });
assert.equal(historyChoiceForRange(presetHistoryRange('7', historyNow), historyNow), '7');
assert.equal(historyDateText(historyDateSeconds('2026-01-03', true), true), '2026-01-03');
assert.equal(historyDateText(historyDateSeconds('2026-01-03', true)), '2026-01-04', 'UI end date is inclusive, persisted end is exclusive');
const historyChanges = [];
const historyProps = { selected: [session('a'), session('b'), session('done')], focused: session('b'), synced: new Set(['done']), scope: 'selected', ranges: {}, choices: {}, disabled: false, onScope() {}, onFocus() {}, onChange(...args) { historyChanges.push(args); }, dateDetails: {}, dateLoading: '', onLoadDates() {} };
let historyTree = ImportHistoryRange(historyProps);
find(historyTree, node => node.type === 'button' && node.props.children === '最近 30 天').props.onClick();
assert.deepEqual(historyChanges.at(-1)[0], ['a', 'b'], 'bulk excludes already-synced conversations');
assert.deepEqual(elements(historyTree).filter(node => node.type === 'div' && node.props['aria-label'] === '首次备份时间选项')[0].props.children.map(node => node.props.children), ['全部历史', '最近 7 天', '最近 30 天', '最近 90 天', '自定义']);
historyTree = ImportHistoryRange({ ...historyProps, scope: 'single' });
find(historyTree, node => node.type === 'button' && node.props.children === '最近 90 天').props.onClick();
assert.deepEqual(historyChanges.at(-1)[0], ['b']);

const historyView = { hooks: [] }, historyWrites = [];
const historyCatalog = Array.from({ length: 63 }, (_, i) => ({ ...session(`h${i}`), is_group: i % 2 === 0 }));
const historySelected = historyCatalog.slice(0, 25).map(item => item.username);
invoke = async (command, args) => {
  if (command === 'app_settings_get') return { ...defaultSettings(), user_settings_revision: 150, active_account_id: 'A', chat: { ...defaultSettings().chat, allowed_usernames: { A: historySelected }, initial_ranges: { A: { h0: { since: null, until: null } } } } };
  if (command === 'chat_accounts') return [{ id: 'A', source: 'wechat' }, { id: 'WA', source: 'whatsapp' }];
  if (command === 'devify_upload_identity' || command === 'wechat_account_profile') return null;
  if (command === 'wechat_session_catalog') return historyCatalog;
  if (command === 'app_settings_save') { historyWrites.push(structuredClone(args.value)); return { ...args.value, user_settings_revision: args.value.user_settings_revision + 1 }; }
  throw Error(command);
};
const drawHistory = () => render(historyView, WhitelistSettings);
for (let i = 0; i < 4; i++) { drawHistory(); await flush(); }
tree = drawHistory();
let editor = find(tree, node => node.type?.name === 'ImportHistoryRange');
assert.equal(editor.props.scope, 'selected');
assert.equal(editor.props.selected.length, 25, 'editor includes selections beyond the first page');
assert.deepEqual(editor.props.ranges.h0, { since: null, until: null }, 'preserve an explicit saved all-history choice');
assert.deepEqual(editor.props.ranges.h24, presetHistoryRange('7'), 'missing ranges default to seven days');
assert.equal(historyWrites.length, 0, 'opening the page never persists defaults');
historyTree = ImportHistoryRange(editor.props);
find(historyTree, node => node.type === 'button' && node.props.children === '最近 30 天').props.onClick();
tree = drawHistory();
find(tree, node => node.type === 'button' && node.props.children === '保存所选会话').props.onClick(); await flush();
assert.equal(historyWrites.length, 1);
for (const id of historySelected) assert.deepEqual(historyWrites[0].chat.initial_ranges.A[id], presetHistoryRange('30'));
find(drawHistory(), node => node.type?.name === 'WizardSessionPicker').props.onFocus('h24');
tree = drawHistory();
editor = find(tree, node => node.type?.name === 'ImportHistoryRange');
assert.equal(editor.props.focused.username, 'h24');
historyTree = ImportHistoryRange(editor.props);
find(historyTree, node => node.type === 'button' && node.props.children === '最近 90 天').props.onClick();
tree = drawHistory();
assert.deepEqual(find(tree, node => node.type?.name === 'ImportHistoryRange').props.ranges.h0, presetHistoryRange('30'), 'single edit leaves other ranges untouched');
assert.deepEqual(find(tree, node => node.type?.name === 'ImportHistoryRange').props.ranges.h24, presetHistoryRange('90'));
let scopeForHistory = find(tree, node => node.type?.name === 'WizardImportScope');
scopeForHistory.props.onScope(false, true);
tree = drawHistory();
assert.equal(find(tree, node => node.type?.name === 'ImportHistoryRange').props.scope, 'selected', 'quick selections return to the shared range editor');
scopeForHistory = find(tree, node => node.type?.name === 'WizardImportScope');
scopeForHistory.props.onScope(true, true);
tree = drawHistory();
assert.equal(find(tree, node => node.type?.name === 'WizardImportScope').props.selected.size, 63, 'private and group shortcuts are additive');
assert.deepEqual(find(tree, node => node.type?.name === 'ImportHistoryRange').props.ranges.h62, presetHistoryRange('7'), 'newly selected groups also get seven-day defaults');
editor = find(tree, node => node.type?.name === 'ImportHistoryRange');
editor.props.onChange(['h24'], { since: null, until: null }, 'custom');
find(drawHistory(), node => node.type === 'button' && node.props.children === '保存所选会话').props.onClick(); await flush();
assert.equal(historyWrites.length, 1, 'incomplete custom dates block saving');
assert.ok(elements(drawHistory()).some(node => node.props?.role === 'alert' && String(node.props.children).includes('自定义范围')));
editor = find(drawHistory(), node => node.type?.name === 'ImportHistoryRange');
editor.props.onChange(['h24'], { since: historyDateSeconds('2026-01-03'), until: historyDateSeconds('2026-01-03', true) }, 'custom');
find(drawHistory(), node => node.type === 'button' && node.props.children === '保存所选会话').props.onClick(); await flush();
assert.equal(historyWrites.length, 2);
assert.deepEqual(historyWrites[1].chat.initial_ranges.A.h24, { since: historyDateSeconds('2026-01-03'), until: historyDateSeconds('2026-01-04') });
// A save on another platform still submits the first account's draft.
editor = find(drawHistory(), node => node.type?.name === 'ImportHistoryRange');
editor.props.onChange(['h24'], { since: null, until: null }, 'custom');
find(drawHistory(), node => node.props?.className === 'imports-platform-head' && elements(node).some(child => child.type === 'strong' && child.props.children === 'WhatsApp')).props.onClick();
for (let i = 0; i < 3; i++) { drawHistory(); await flush(); }
find(drawHistory(), node => node.type?.name === 'WizardImportScope').props.onScope(false, true);
find(drawHistory(), node => node.type === 'button' && node.props.children === '保存所选会话').props.onClick(); await flush();
assert.equal(historyWrites.length, 2, 'cannot serialize an incomplete custom range through a different account');
for (const hook of historyView.hooks) hook?.cleanup?.();

// Recover existing credentials without starting another login; the successful gate
// unmount must not invalidate the newly recovered session through auth_cancel.
const savedSessionGate = { hooks: [] }, savedSessionCheck = deferred();
let savedSessionReads = 0, recoveredSessions = 0, cancelledSessions = 0;
invoke = (command) => {
  if (command === 'app_settings_get') return Promise.resolve({ ...defaultSettings(), user_settings_revision: 140, devify: { ...defaultSettings().devify, base_url: 'https://example.invalid' } });
  if (command === 'devify_auth_status') { savedSessionReads++; return savedSessionCheck.promise; }
  if (command === 'devify_auth_cancel') { cancelledSessions++; return Promise.resolve(); }
  throw Error(command);
};
const drawSavedSession = () => render(savedSessionGate, () => LoginGate({ onAuthenticated() { recoveredSessions++; } }));
drawSavedSession(); await flush();
tree = drawSavedSession();
const retrySession = find(tree, node => node.type === 'button' && node.props.children === '重新检查已有登录').props.onClick;
retrySession(); retrySession(); await flush();
assert.equal(savedSessionReads, 1, 'rapid repeated recovery clicks send one check');
tree = drawSavedSession();
assert.equal(find(tree, node => node.type === 'button' && node.props.children === '正在检查…').props.disabled, true);
savedSessionCheck.resolve({ authenticated: true }); await flush();
assert.equal(recoveredSessions, 1);
for (const hook of savedSessionGate.hooks) hook?.cleanup?.();
assert.equal(cancelledSessions, 0, 'finishing recovery cannot invalidate its credential generation');
// Enter and a click in the same render must share one password action, including
// the asynchronous server validation before the native login command starts.
const passwordGate = { hooks: [] }, passwordResult = deferred();
let passwordRequests = 0, passwordCompletions = 0;
invoke = command => {
  if (command === 'app_settings_get') return Promise.resolve({ ...defaultSettings(), user_settings_revision: 141, devify: { ...defaultSettings().devify, base_url: 'https://example.invalid' } });
  if (command === 'devify_login_password') { passwordRequests++; return passwordResult.promise; }
  if (command === 'devify_auth_cancel') return Promise.resolve();
  throw Error(command);
};
const drawPassword = () => render(passwordGate, () => LoginGate({ onAuthenticated() { passwordCompletions++; } }));
drawPassword(); await flush(); tree = drawPassword();
find(tree, node => node.type === 'button' && node.props.children === '登录…').props.onClick();
tree = drawPassword();
find(tree, node => node.type === 'input' && node.props.name === 'username').props.onChange({ target: { value: 'test-user' } });
find(tree, node => node.type === 'input' && node.props.name === 'password').props.onChange({ target: { value: 'synthetic-password' } });
tree = drawPassword();
const passwordSubmit = find(tree, node => node.type === 'form').props.onSubmit;
passwordSubmit({ preventDefault() {} }); passwordSubmit({ preventDefault() {} }); await flush();
assert.equal(passwordRequests, 1, 'rapid password submits cannot start competing login generations');
tree = drawPassword();
assert.equal(find(tree, node => node.type === 'button' && node.props.type === 'submit').props.disabled, true);
passwordResult.resolve(); await flush();
assert.equal(passwordCompletions, 1);
for (const hook of passwordGate.hooks) hook?.cleanup?.();
if (previousWindow === undefined) delete globalThis.window; else globalThis.window = previousWindow;
if (previousDocument === undefined) delete globalThis.document; else globalThis.document = previousDocument;
console.log('settings revision, navigation, explicit checks and WhatsApp wizard checks passed');
