import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';

// Execute only transpiled local source; all IPC and child components are fakes.
let current;
const react = {
  useState(initial) {
    const instance = current, index = instance.index++;
    if (!(index in instance.hooks)) instance.hooks[index] = typeof initial === 'function' ? initial() : initial;
    return [instance.hooks[index], value => {
      instance.hooks[index] = typeof value === 'function' ? value(instance.hooks[index]) : value;
    }];
  },
  useRef(value) {
    const instance = current, index = instance.index++;
    if (!(index in instance.hooks)) instance.hooks[index] = { current: value };
    return instance.hooks[index];
  },
  useEffect(effect, deps) {
    const instance = current, index = instance.index++, previous = instance.hooks[index];
    if (!previous || !deps || !previous.deps || deps.length !== previous.deps.length || deps.some((value, i) => !Object.is(value, previous.deps[i]))) {
      instance.effects.push(() => {
        previous?.cleanup?.();
        instance.hooks[index] = { deps, cleanup: effect() };
      });
    }
  },
  useMemo(fn, deps) {
    const instance = current, index = instance.index++, previous = instance.hooks[index];
    if (!previous || !deps || !previous.deps || deps.length !== previous.deps.length || deps.some((value, i) => !Object.is(value, previous.deps[i]))) {
      instance.hooks[index] = { deps, value: fn() };
    }
    return instance.hooks[index].value;
  },
};
const jsx = { jsx: (type, props) => ({ type, props }), jsxs: (type, props) => ({ type, props }), Fragment: Symbol('Fragment') };
const flush = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };
const render = (instance, kind = 'reports') => {
  current = instance;
  instance.index = 0;
  instance.effects = [];
  const tree = CloudAnalysisPage({ kind });
  for (const effect of instance.effects) effect();
  current = undefined;
  return tree;
};
const elements = node => {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!node || typeof node !== 'object') return [];
  return [node, ...elements(node.props?.children)];
};
const text = node => {
  if (Array.isArray(node)) return node.map(text).join('');
  if (node == null || typeof node === 'boolean') return '';
  if (typeof node !== 'object') return String(node);
  return text(node.props?.children);
};
const find = (tree, predicate, message = 'expected element missing') => {
  const node = elements(tree).find(predicate);
  assert.ok(node, message);
  return node;
};
const byClass = (tree, className) => find(tree, node => node.props?.className === className, `missing .${className}`);

let invoke = () => { throw Error('unexpected IPC before setup'); };
function EvidenceDrawer() { throw Error('child component should not execute'); }
const mocks = {
  react,
  'react/jsx-runtime': jsx,
  '@tauri-apps/api/core': { invoke: (...args) => invoke(...args), isTauri: () => true },
  './SidebarResizeHandle': { SidebarResizeHandle: () => null },
  './EvidenceDrawer': { EvidenceDrawer },
  './toast': { exportNotSupported: () => {} },
};
const load = path => {
  const source = fs.readFileSync(new URL(path, import.meta.url), 'utf8');
  const compiled = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2020 },
  }).outputText;
  const module = { exports: {} };
  const requireMock = id => {
    if (id in mocks) return mocks[id];
    throw Error(`unexpected import: ${id}`);
  };
  new Function('require', 'module', 'exports', compiled)(requireMock, module, module.exports);
  return module.exports;
};
const { reportPresentation } = load('../src/reportPresentation.ts');
mocks['./reportPresentation'] = { reportPresentation };
const { CloudAnalysisPage } = load('../src/CloudAnalysis.tsx');

const partialWith = '报告未完全生成，以下展示已保存的内容。';
const partialWithout = '报告未完全生成，暂无已保存的正文。';
const waitingWith = '服务端正在生成这份报告，以下展示已保存的内容。';
const waitingWithout = '服务端正在生成这份报告，稍后刷新即可看到内容。';
const failedWith = '报告生成失败，以下展示已保存的内容。';
const failedWithout = '报告生成失败，暂无已保存的正文。';
const body = '  第一行\n第二行  \n';
const content = '  后备正文\n末行  ';
const metadata = { metadata: '不应出现的元数据', count: 3 };
const metadataJSON = JSON.stringify(metadata, null, 2);

// Table-driven helper contract: each known status with and without saved text,
// both body and content fields, whitespace, objects, and legacy statuses.
for (const [status, withText, withoutText] of [
  ['partial', partialWith, partialWithout],
  ['queued', waitingWith, waitingWithout],
  ['processing', waitingWith, waitingWithout],
  ['failed', failedWith, failedWithout],
  ['ready', null, null],
  ['legacy-status', null, null],
  [undefined, null, null],
]) {
  const check = (input, expected) => assert.deepEqual(reportPresentation({ status, ...input }), expected, `${status}: ${JSON.stringify(input)}`);
  check({}, { notice: withoutText, content: null });
  check({ body: '', content: '' }, { notice: withoutText, content: null });
  check({ body: ' \n\t', content: '  \n' }, { notice: withoutText, content: null });
  check({ body }, { notice: withText, content: body });
  check({ content }, { notice: withText, content });
  check({ body, content }, { notice: withText, content: body });
  check({ body: ' \n', content }, { notice: withText, content });
  check({ body, content: metadata }, { notice: withText, content: body });
  const objectIsBody = !['partial', 'queued', 'processing'].includes(status);
  check({ body: '', content: metadata }, {
    notice: objectIsBody ? withText : withoutText,
    content: objectIsBody ? metadataJSON : null,
  });
}
assert.deepEqual(reportPresentation({ status: 'ready', content: ['甲', '乙'] }), { notice: null, content: JSON.stringify(['甲', '乙'], null, 2) });
assert.deepEqual(reportPresentation({ status: 'failed', body: null, content: null }), { notice: failedWithout, content: null });

// Entirely synthetic report and space ids. Mock IPC records exactly which calls
// the page makes; it never reaches a running app, server, or local user data.
const sections = [
  { id: 1, title: '虚构分类甲', position: 1, items: [{ id: 11, title: '虚构事项甲', summary: '虚构摘要甲', label: '标签甲', sources: [{ note_id: 701 }, { event_id: 601 }] }] },
  { id: 2, title: '虚构分类乙', position: 2, items: [{ id: 12, title: '虚构事项乙', summary: '虚构摘要乙', sources: [{ note_id: 702 }] }] },
  { id: 3, title: '虚构分类丙', position: 3, items: [{ id: 13, title: '虚构事项丙', summary: '虚构摘要丙', sources: [] }] },
];
const fixture = overrides => ({ id: 101, title: '虚构日报', status: 'partial', sections, ...overrides });

// Loading text is shared by the events and reports pages and must use generic wording.
invoke = command => {
  if (command === 'app_settings_get') return new Promise(() => {});
  throw Error(`unexpected IPC: ${command}`);
};
for (const [kind, label] of [['events', '事件'], ['reports', '报告']]) {
  const loading = render({ hooks: [] }, kind);
  assert.ok(text(loading).includes(`正在同步${label}…`));
  assert.ok(text(loading).includes('正在从服务端读取当前空间的最新结果。'));
  assert.ok(!text(loading).includes('Devify'));
}

async function mount(report, failDetail = false) {
  const calls = [];
  invoke = (command, args) => {
    calls.push({ command, args });
    if (command === 'app_settings_get') return Promise.resolve({ devify: { active_space_id: 999 } });
    if (command === 'devify_analysis_list') {
      assert.deepEqual(args, { kind: 'reports', spaceId: 999, page: 1 });
      // List payload intentionally omits the body and sections: assertions below
      // must exercise the separate detail response, not merely a list row.
      return Promise.resolve({ list: [{ id: report.id, title: report.title, status: report.status }], pagination: { total: 1 } });
    }
    if (command === 'devify_analysis_detail') {
      assert.deepEqual(args, { kind: 'reports', spaceId: 999, itemId: report.id });
      return failDetail ? Promise.reject('虚构详情错误') : Promise.resolve(report);
    }
    throw Error(`unexpected IPC: ${command}`);
  };
  const instance = { hooks: [] };
  render(instance); await flush();
  render(instance); await flush();
  const tree = render(instance);
  assert.deepEqual(calls.map(call => call.command), ['app_settings_get', 'devify_analysis_list', 'devify_analysis_detail']);
  return { instance, tree, calls };
}
const summaryOf = tree => byClass(tree, 'report-summary');
const assertNotice = (tree, role, expected) => {
  const summary = summaryOf(tree);
  const notices = elements(summary).filter(node => node.props?.role === role);
  assert.equal(notices.length, 1, `expected one ${role} notice`);
  assert.equal(text(notices[0]), expected);
};

const partial = await mount(fixture({ body, content: metadata }));
let tree = partial.tree;
assertNotice(tree, 'status', partialWith);
assert.ok(text(summaryOf(tree)).includes(body), 'partial notice and saved body must both render');
assert.ok(!text(summaryOf(tree)).includes(metadata.metadata));
assert.match(text(summaryOf(tree)), /3 个分类 · 3 项/);
const sectionNodes = elements(tree).filter(node => node.type === 'section' && node.props?.className === 'report-section');
assert.equal(sectionNodes.length, 3);
for (let i = 0; i < sections.length; i++) {
  assert.ok(text(sectionNodes[i]).includes(sections[i].title));
  assert.ok(text(sectionNodes[i]).includes(sections[i].items[0].title));
  assert.ok(text(sectionNodes[i]).includes(sections[i].items[0].summary));
}
const buttons = elements(tree).filter(node => node.type === 'button' && node.props?.className === 'report-item');
assert.equal(buttons.length, 3);
assert.equal(buttons[0].props.disabled, false);
assert.equal(buttons[1].props.disabled, false);
assert.equal(buttons[2].props.disabled, true);
assert.ok(text(buttons[0]).includes('标签甲'));
buttons[0].props.onClick();
tree = render(partial.instance);
let drawer = find(tree, node => node.type === EvidenceDrawer, 'event source did not open evidence');
assert.deepEqual(drawer.props.target, { kind: 'event', id: 601 }, 'event source takes priority over note source');
assert.equal(drawer.props.spaceId, 999);
drawer.props.onClose();
tree = render(partial.instance);
assert.ok(!elements(tree).some(node => node.type === EvidenceDrawer), 'evidence should close');
find(tree, node => node.type === 'button' && node.props?.className === 'report-item' && text(node).includes('虚构事项乙')).props.onClick();
tree = render(partial.instance);
drawer = find(tree, node => node.type === EvidenceDrawer, 'note source did not open evidence');
assert.deepEqual(drawer.props.target, { kind: 'note', id: 702 });
drawer.props.onClose();
tree = render(partial.instance);
// Even a directly invoked handler for an unavailable source must not open evidence.
find(tree, node => node.type === 'button' && node.props?.className === 'report-item' && text(node).includes('虚构事项丙')).props.onClick();
tree = render(partial.instance);
assert.ok(!elements(tree).some(node => node.type === EvidenceDrawer));
assert.equal(partial.calls.length, 3, 'source controls must not call real or unexpected IPC');

for (const status of ['partial', 'queued', 'processing']) {
  const { tree: noBody } = await mount(fixture({ status, body: '  \n', content: metadata }));
  assertNotice(noBody, 'status', status === 'partial' ? partialWithout : waitingWithout);
  assert.ok(!text(summaryOf(noBody)).includes(metadata.metadata), `${status} metadata rendered as body`);
  assert.ok(!text(summaryOf(noBody)).includes('服务端尚未返回正文。'));
  const { tree: withBody } = await mount(fixture({ status, content }));
  assertNotice(withBody, 'status', status === 'partial' ? partialWith : waitingWith);
  assert.ok(text(summaryOf(withBody)).includes(content));
}
for (const [overrides, role, notice, expectedContent] of [
  [{ status: 'failed', content: metadata }, 'alert', failedWith, metadataJSON],
  [{ status: 'failed', content: null }, 'alert', failedWithout, null],
  [{ status: 'ready', content: metadata }, null, null, metadataJSON],
  [{ status: 'ready', content: null }, null, null, '服务端尚未返回正文。'],
  [{ status: 'legacy-status', content }, null, null, content],
]) {
  const { tree: rendered } = await mount(fixture(overrides));
  const summary = summaryOf(rendered);
  if (role) assertNotice(rendered, role, notice);
  else assert.ok(!elements(summary).some(node => ['status', 'alert'].includes(node.props?.role)));
  if (expectedContent) assert.ok(text(summary).includes(expectedContent));
}
// The reported case has three sections and hundreds of entries. Render that size
// using synthetic content, without reading any real report or message data.
const largeSections = [335, 400, 111].map((count, sectionIndex) => ({
  id: 200 + sectionIndex,
  title: `虚构大分类 ${sectionIndex}`,
  position: sectionIndex,
  items: Array.from({ length: count }, (_, itemIndex) => ({
    id: sectionIndex * 1000 + itemIndex + 1,
    title: `虚构事项 ${sectionIndex}-${itemIndex}`,
    summary: '虚构摘要',
    sources: [],
  })),
}));
const { tree: largeTree } = await mount(fixture({ body: '虚构的已保存正文', sections: largeSections }));
assertNotice(largeTree, 'status', partialWith);
assert.match(text(summaryOf(largeTree)), /3 个分类 · 846 项/);
assert.ok(text(summaryOf(largeTree)).includes('虚构的已保存正文'));
assert.equal(elements(largeTree).filter(node => node.type === 'button' && node.props?.className === 'report-item').length, 846);

const { tree: errorTree } = await mount(fixture({ status: 'ready' }), true);
const errorAlert = find(errorTree, node => node.props?.role === 'alert');
assert.ok(text(errorAlert).includes('虚构详情错误'), 'IPC failure must remain visible');
console.log('report presentation and CloudAnalysis rendering checks passed');
