import * as nativeErrors from '../src/shared/i18n/errorText.ts';
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
  useCallback(fn, deps) { return react.useMemo(() => fn, deps); },
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
const renderPage = (instance, page) => {
  current = instance;
  instance.index = 0;
  instance.effects = [];
  const tree = page();
  for (const effect of instance.effects) effect();
  current = undefined;
  return tree;
};
const render = (instance, kind = 'reports', focusId) =>
  renderPage(instance, () => CloudAnalysisPage({ kind, userId: null, focusId }));
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
  '../../shared/ui/SidebarResizeHandle': { SidebarResizeHandle: () => null },
  '../../shared/ui/LoadingSkeleton': { LoadingPage: () => null, LoadingSkeleton: () => null },
  '../../shared/ui/EvidenceDrawer': { EvidenceDrawer },
  './toast': { exportNotSupported: () => {} },
};
const load = path => {
  const source = fs.readFileSync(new URL(path, import.meta.url), 'utf8');
  const compiled = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2020 },
  }).outputText;
  const module = { exports: {} };
  const requireMock = id => {
    if (id === '../../shared/content/summary') return { renderAgentMarkdown: value => value };
    if (id in mocks) return mocks[id];
    if (id === '../../shared/i18n/locale' || id === '../i18n/locale') { const format = (value, ...args) => value.replace(/\{(\d+)\}/g, (_, n) => String(args[Number(n)] ?? '')); return { t: value => value, cf: value => value, tf: format, ctf: format, dateLocale: () => 'zh-CN', sortLocale: () => 'zh-Hans-CN' }; }
  if (id.endsWith('/i18n/errorText')) return nativeErrors;
    throw Error(`unexpected import: ${id}`);
  };
  new Function('require', 'module', 'exports', compiled)(requireMock, module, module.exports);
  return module.exports;
};
const { reportPresentation } = load('../src/features/analysis/reportPresentation.ts');
mocks['./reportPresentation'] = { reportPresentation };
mocks['../../shared/state/pageSnapshots'] = load('../src/shared/state/pageSnapshots.ts');
mocks['../../shared/api/eventRelevance'] = load('../src/shared/api/eventRelevance.ts');
mocks['./useEventSearch'] = load('../src/features/analysis/useEventSearch.ts');
const { CloudAnalysisPage } = load('../src/features/analysis/CloudAnalysis.tsx');

const partialWith = '报告未完全生成，以下展示已保存的内容。';
const partialWithout = '报告未完全生成，暂无已保存的正文。';
const waitingWith = '正在生成这份报告，以下展示已保存的内容。';
const waitingWithout = '正在生成这份报告，稍后刷新即可看到内容。';
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
for (const [kind, label] of [['events', '待办'], ['reports', '报告']]) {
  const loading = render({ hooks: [] }, kind);
  assert.equal(loading.type, mocks['../../shared/ui/LoadingSkeleton'].LoadingPage);
  assert.equal(loading.props.title, label);
}

async function mount(report, failDetail = false) {
  const calls = [];
  invoke = (command, args) => {
    calls.push({ command, args });
    if (command === 'app_settings_get') return Promise.resolve({ devify: { active_space_id: "999" } });
    if (command === 'devify_analysis_list') {
      assert.deepEqual(args, { kind: 'reports', spaceId: "999", page: 1 });
      // List payload intentionally omits the body and sections: assertions below
      // must exercise the separate detail response, not merely a list row.
      return Promise.resolve({ list: [{ id: report.id, title: report.title, status: report.status }], pagination: { total: 1 } });
    }
    if (command === 'devify_analysis_detail') {
      assert.deepEqual(args, { kind: 'reports', spaceId: "999", itemId: report.id });
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
const weeklyEnglish = await mount(fixture({ title: 'Weekly · week 48', body: 'Report body' }));
assert.ok(text(weeklyEnglish.tree).includes('周报 ·'), 'English-generated reports keep their cadence in the Chinese interface');
const weeklyFilter = find(weeklyEnglish.tree, node => node.type === 'button' && text(node) === '周报1');
weeklyFilter.props.onClick();
assert.match(text(byClass(render(weeklyEnglish.instance), 'analysis-items')), /Weekly · week 48/);
const summaryOf = tree => byClass(tree, 'report-summary');

{
  const { instance, tree } = await mount(fixture({ title: '主题 · 虚构客户反馈日报' }));
  assert.equal(tree.props.className, 'analysis-page report-workspace');
  assert.ok(!elements(tree).some(node => node.props?.className === 'analysis-side-row'));
  find(tree, node => node.type === mocks['../../shared/ui/SidebarResizeHandle'].SidebarResizeHandle);
  const topicFilter = find(tree, node => node.type === 'button' && text(node) === '主题1');
  topicFilter.props.onClick();
  assert.equal(elements(byClass(render(instance), 'analysis-items')).filter(node => node.type === 'button').length, 1);
  find(tree, node => node.type === 'input' && node.props?.['aria-label'] === '搜索报告').props.onChange({ target: { value: '不存在的标题' } });
  const noMatch = render(instance);
  assert.match(text(byClass(noMatch, 'analysis-list-empty')), /当前筛选下没有记录。/);
  assert.ok(!text(noMatch).includes('当前空间还没有报告。'));
  find(noMatch, node => node.type === 'button' && text(node) === '清除搜索').props.onClick();
  assert.match(text(byClass(render(instance), 'analysis-items')), /虚构客户反馈/);
}

for (const failed of [false, true]) {
  invoke = command => {
    if (command === 'app_settings_get') return Promise.resolve({ devify: { active_space_id: '999' } });
    if (command === 'devify_analysis_list') return failed ? Promise.reject('ACCOUNT_UPLOAD_BUSY: 虚构同步错误') : Promise.resolve({ list: [] });
    throw Error(`unexpected IPC: ${command}`);
  };
  const instance = { hooks: [] };
  render(instance); await flush();
  const tree = render(instance);
  const state = byClass(tree, failed ? 'analysis-empty report-empty error' : 'analysis-empty report-empty');
  assert.equal(state.props.role, failed ? 'alert' : undefined);
  assert.match(text(state), failed ? /同步失败虚构同步错误重试/ : /当前空间还没有报告。.*生成报告/);
  assert.deepEqual(
    elements(byClass(tree, 'analysis-pills')).filter(node => node.type === 'small').map(text),
    Array(5).fill(failed ? '—' : '0'),
  );
  assert.ok(!elements(tree).some(node => node.props?.className === 'analysis-list-empty'));
}

const assertNotice = (tree, role, expected) => {
  const summary = summaryOf(tree);
  const notices = elements(summary).filter(node => node.props?.role === role);
  assert.equal(notices.length, 1, `expected one ${role} notice`);
  assert.equal(text(notices[0]), expected);
};

const refreshedReport = fixture({ body: '刷新后的报告正文' });
const refreshPage = await mount(fixture({ body }));
let resolveRefreshList, resolveRefreshDetail;
invoke = (command, args) => {
  if (command === 'app_settings_get') return Promise.resolve({ devify: { active_space_id: '999' } });
  if (command === 'devify_analysis_list') return new Promise(resolve => { resolveRefreshList = resolve; });
  if (command === 'devify_analysis_detail') {
    assert.equal(args.itemId, refreshedReport.id);
    return new Promise(resolve => { resolveRefreshDetail = resolve; });
  }
  throw Error(`unexpected IPC: ${command}`);
};
const refreshButton = view => find(view, node => node.type === 'button' && text(node) === '刷新');
refreshButton(refreshPage.tree).props.onClick();
render(refreshPage.instance); await flush();
let refreshingReport = render(refreshPage.instance);
assert.equal(refreshButton(refreshingReport).props['aria-busy'], true);
assert.ok(text(summaryOf(refreshingReport)).includes(body), 'List refresh keeps the report visible');
resolveRefreshList({ list: [{ id: refreshedReport.id, title: refreshedReport.title }] });
await flush();
render(refreshPage.instance); await flush();
refreshingReport = render(refreshPage.instance);
assert.ok(text(summaryOf(refreshingReport)).includes(body), 'Detail refresh keeps the report visible');
resolveRefreshDetail(refreshedReport);
await flush();
assert.ok(text(summaryOf(render(refreshPage.instance))).includes(refreshedReport.body));

invoke = (command, args) => {
  if (command === 'app_settings_get') return Promise.resolve({ devify: { active_space_id: '1000' } });
  if (command === 'devify_analysis_list') {
    assert.equal(args.spaceId, '1000');
    return new Promise(() => {});
  }
  throw Error(`unexpected IPC: ${command}`);
};
refreshButton(render(refreshPage.instance)).props.onClick();
render(refreshPage.instance); await flush();
const newSpacePage = render(refreshPage.instance);
assert.equal(newSpacePage.type, mocks['../../shared/ui/LoadingSkeleton'].LoadingPage);
assert.ok(!text(newSpacePage).includes(refreshedReport.body), 'A space change clears the old report');

const sameIdReport = await mount(fixture({ body }));
invoke = async (command, args) => {
  if (command === 'app_settings_get') {
    return { devify: { base_url: 'https://reports.example.invalid', active_space_id: '999' } };
  }
  if (command === 'devify_analysis_list') return { list: [{ id: refreshedReport.id, title: refreshedReport.title }] };
  if (command === 'devify_analysis_detail') {
    assert.deepEqual(args, { kind: 'reports', spaceId: '999', itemId: refreshedReport.id });
    return refreshedReport;
  }
  throw Error(command);
};
refreshButton(sameIdReport.tree).props.onClick();
render(sameIdReport.instance); await flush();
const switchingServer = render(sameIdReport.instance);
assert.ok(!text(switchingServer).includes(body), 'A server change must not reuse the same report ID from the previous server');
await flush();
assert.ok(text(summaryOf(render(sameIdReport.instance))).includes(refreshedReport.body));

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
assert.equal(drawer.props.spaceId, "999");
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
  assert.ok(!text(summaryOf(noBody)).includes('尚未返回正文。'));
  const { tree: withBody } = await mount(fixture({ status, content }));
  assertNotice(withBody, 'status', status === 'partial' ? partialWith : waitingWith);
  assert.ok(text(summaryOf(withBody)).includes(content));
}
for (const [overrides, role, notice, expectedContent] of [
  [{ status: 'failed', content: metadata }, 'alert', failedWith, metadataJSON],
  [{ status: 'failed', content: null }, 'alert', failedWithout, null],
  [{ status: 'ready', content: metadata }, null, null, metadataJSON],
  [{ status: 'ready', content: null }, null, null, '尚未返回正文。'],
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

const failedReport = fixture({ status: 'ready' });
const { instance: retryInstance, tree: errorTree } = await mount(failedReport, true);
const errorAlert = find(errorTree, node => node.props?.role === 'alert');
assert.ok(text(errorAlert).includes('虚构详情错误'), 'IPC failure must remain visible');
assert.ok(text(byClass(errorTree, 'analysis-items')).includes(failedReport.title), 'Detail failure must keep the list visible');
const retryButton = find(errorAlert, node => node.type === 'button' && text(node) === '重试');
invoke = async command => {
  if (command === 'app_settings_get') return { devify: { active_space_id: '999' } };
  if (command === 'devify_analysis_list') return { list: [{ id: failedReport.id, title: failedReport.title }] };
  if (command === 'devify_analysis_detail') return { ...failedReport, body: '重试后的报告正文' };
  throw Error(command);
};
retryButton.props.onClick();
render(retryInstance); await flush();
render(retryInstance); await flush();
assert.ok(text(summaryOf(render(retryInstance))).includes('重试后的报告正文'));
console.log('report presentation and CloudAnalysis rendering checks passed');

// Personal classification and shared completion are independent server fields.
const completedEvent={id:601,title:'虚构已完成待办',status:'done',content:'虚构摘要',viewer_relevance:'related',viewer_evidence_message_ids:[99],evidence:[
  {id:99,message_id:8,content:'不匹配的原文'}, {id:8,message_id:99,content:'匹配的原文'},
]};
const myEvent={id:602,title:'默认个人待办',status:'open',content:'个人待办摘要',viewer_relevance:'my_todo'};
const eventCounts={my_todo:1,related:6,other:4};
const eventCalls=[],detailResolvers=[];
invoke=(command,args)=>{
  eventCalls.push({command,args});
  if(command==='app_settings_get')return Promise.resolve({devify:{active_space_id:'event-fixture'}});
  if(command==='devify_analysis_list')return Promise.resolve({list:args.viewerRelevance==='my_todo'?[myEvent]:args.viewerRelevance==='other'?[]:[completedEvent],pagination:{total:args.viewerRelevance==='my_todo'?1:args.viewerRelevance==='other'?4:6},relevance_counts:eventCounts});
  if(command==='devify_event_notes'||command==='devify_event_relations')return Promise.resolve([]);
  if(command==='devify_analysis_detail')return new Promise(resolve=>detailResolvers.push(resolve));
  throw Error(`unexpected event IPC: ${command}`);
};
const eventInstance={hooks:[]};
const renderEvents=()=>render(eventInstance,'events',null);
const settleEvents=async()=>{for(let i=0;i<4;i++){renderEvents();await flush();}return renderEvents();};
let eventTree=await settleEvents();
const hasDetailLoading=tree=>elements(tree).some(node=>node.type===mocks['../../shared/ui/LoadingSkeleton'].LoadingSkeleton&&node.props.label==='正在加载详情');
const selectFilter=(tree,label)=>find(byClass(tree,'analysis-pills'),node=>node.type==='button'&&text(node).startsWith(label)).props.onClick();
assert.equal(find(byClass(eventTree,'analysis-pills'),node=>node.type==='button'&&text(node).startsWith('我的待办')).props['aria-pressed'],true);
assert.deepEqual(eventCalls.find(c=>c.command==='devify_analysis_list').args,{kind:'events',spaceId:'event-fixture',page:1,pageSize:50,viewerRelevance:'my_todo',includeRelevanceCounts:false});
assert.equal(elements(byClass(eventTree,'analysis-pills')).filter(n=>n.type==='button').length,3);
assert.match(text(byClass(eventTree,'analysis-pills')),/我的待办1与我有关6其他4/);
selectFilter(eventTree,'与我有关');eventTree=await settleEvents();
assert.equal(detailResolvers.length,2);
detailResolvers[1](completedEvent);eventTree=await settleEvents();
detailResolvers[0](myEvent);eventTree=await settleEvents();
assert.ok(text(byClass(eventTree,'analysis-doc')).includes(completedEvent.title),'Late detail cannot replace the current category');
assert.match(text(byClass(eventTree,'analysis-items')),/已完成 · 与我有关/);
find(eventTree,node=>node.type==='button'&&text(node)==='查看全部原文').props.onClick();
eventTree=renderEvents();
const highlights=elements(eventTree).filter(node=>String(node.props?.className).includes('event-personal-evidence'));
assert.equal(highlights.length,1);
assert.ok(text(highlights[0]).includes('匹配的原文')&&!text(highlights[0]).includes('不匹配的原文'),'Highlight matches evidence.message_id, never evidence.id');
find(eventTree,node=>node.type==='input'&&node.props['aria-label']==='搜索当前页待办').props.onChange({target:{value:'no matches'}});
eventTree=await settleEvents();
find(eventTree,node=>node.type==='button'&&text(node)==='清除搜索').props.onClick();
eventTree=await settleEvents();
assert.equal(find(eventTree,node=>node.type==='input'&&node.props['aria-label']==='搜索当前页待办').props.value,'');
selectFilter(eventTree,'其他');eventTree=await settleEvents();
assert.equal(hasDetailLoading(eventTree),false);
assert.ok(text(eventTree).includes('当前筛选下没有记录'));
assert.ok(!text(eventTree).includes('查看全部待办'),'Hidden All category must not reappear through an empty-state action');
// Existing links to an event still fetch that event even outside the first page.
const focusedInstance={hooks:[]};
for(let i=0;i<4;i++){render(focusedInstance,'events',601);await flush();}
assert.deepEqual(eventCalls.filter(c=>c.command==='devify_analysis_list').at(-1).args,{kind:'events',spaceId:'event-fixture',page:1,pageSize:50,viewerRelevance:null,includeRelevanceCounts:true});
console.log('Default personal category, three server counts, independent status, evidence IDs and cancelled details passed');

// Paging is bounded; switching categories starts at page one and rejects late replies.
const pageCounts={my_todo:60,related:41,other:0};
const eventPages=[],pendingCategories=[];
let raceAccount=1;
const raceRow=(id,title,viewer_relevance='my_todo')=>({id,title,content:title,status:'open',viewer_relevance});
invoke=(command,args)=>{
  if(command==='app_settings_get')return Promise.resolve({devify:{base_url:'https://events.invalid',active_space_id:'race-space'}});
  if(command==='devify_analysis_list'){
    eventPages.push(args);
    if(raceAccount===2)return Promise.resolve({list:[],pagination:{total:0,next:null},relevance_counts:{my_todo:0,related:0,other:0}});
    if(args.viewerRelevance!=='my_todo')return new Promise(resolve=>pendingCategories.push({args,resolve}));
    return Promise.resolve({list:[raceRow(args.page,args.page===1?'我的第一页':'我的第二页')],pagination:{total:60,next:args.page===1?'page=2':null},relevance_counts:pageCounts});
  }
  if(command==='devify_analysis_detail')return Promise.resolve(raceRow(args.itemId,'详情'+args.itemId));
  if(command==='devify_event_notes'||command==='devify_event_relations')return Promise.resolve([]);
  throw Error(command);
};
const raceInstance={hooks:[]};
const renderRace=()=>renderPage(raceInstance,()=>CloudAnalysisPage({kind:'events',userId:raceAccount}));
const settleRace=async()=>{for(let i=0;i<4;i++){renderRace();await flush();}return renderRace();};
let raceTree=await settleRace();
find(raceTree,node=>node.type==='button'&&text(node)==='下一页').props.onClick();
raceTree=await settleRace();
assert.equal(eventPages.at(-1).page,2);
assert.match(text(byClass(raceTree,'analysis-pills')),/我的待办60与我有关41/,'Counts do not shrink to the current page');
selectFilter(raceTree,'与我有关');raceTree=await settleRace();
assert.equal(pendingCategories.at(-1).args.page,1);
selectFilter(raceTree,'其他');raceTree=await settleRace();
assert.equal(pendingCategories.at(-1).args.page,1);
pendingCategories[1].resolve({list:[],pagination:{total:0,next:null},relevance_counts:pageCounts});
raceTree=await settleRace();
pendingCategories[0].resolve({list:[raceRow(4,'迟到的相关事项','related')],pagination:{total:41,next:null},relevance_counts:{my_todo:999,related:0,other:0}});
raceTree=await settleRace();
assert.ok(!text(raceTree).includes('迟到的相关事项')&&!text(byClass(raceTree,'analysis-pills')).includes('999'));
raceAccount=2;
assert.equal(renderRace().type,mocks['../../shared/ui/LoadingSkeleton'].LoadingPage,'An account change hides the former personal classification immediately');
raceTree=await settleRace();
assert.equal(eventPages.at(-1).viewerRelevance,'my_todo');
assert.equal(eventPages.at(-1).page,1);
assert.match(text(byClass(raceTree,'analysis-pills')),/我的待办0与我有关0其他0/);
console.log('Event paging, page-one category changes, global counts, late replies and account isolation passed');

// Older servers remain readable, but their unfiltered rows are never labelled as personal.
let upgradedServer=false;
const legacyCalls=[];
invoke=(command,args)=>{
  legacyCalls.push({command,args});
  if(command==='app_settings_get')return Promise.resolve({devify:{active_space_id:'legacy-event-space'}});
  if(command==='devify_analysis_list')return Promise.resolve({list:[raceRow(70,'旧服务端未筛选的事项')],pagination:{total:70},...(upgradedServer?{relevance_counts:pageCounts}:{})});
  if(command==='devify_analysis_detail')return Promise.resolve(raceRow(70,'旧服务端详情'));
  if(command==='devify_event_relations'||command==='devify_event_notes')return Promise.resolve([]);
  throw Error(command);
};
const legacyInstance={hooks:[]};
const settleLegacy=async()=>{for(let i=0;i<4;i++){render(legacyInstance,'events');await flush();}return render(legacyInstance,'events');};
let legacyTree=await settleLegacy();
assert.ok(text(legacyTree).includes('暂时显示全部待办'));
assert.ok(text(byClass(legacyTree,'analysis-items')).includes('旧服务端未筛选的事项'));
assert.ok(elements(byClass(legacyTree,'analysis-pills')).filter(n=>n.type==='button').every(n=>n.props.disabled&&!n.props['aria-pressed']));
assert.equal(legacyCalls.filter(c=>c.command==='devify_analysis_list').at(-1).args.viewerRelevance,null);
upgradedServer=true;
find(legacyTree,n=>n.type==='button'&&text(n)==='刷新').props.onClick();legacyTree=await settleLegacy();
assert.ok(!text(legacyTree).includes('暂时显示全部待办'));
assert.equal(legacyCalls.filter(c=>c.command==='devify_analysis_list').at(-1).args.viewerRelevance,'my_todo');
assert.ok(elements(byClass(legacyTree,'analysis-pills')).filter(n=>n.type==='button').every(n=>!n.props.disabled));
console.log('Legacy server stays readable without personal mislabelling; refresh detects an upgraded server');

// Completing the only item on page two must not re-request an invalid page.
const boundaryRows=Array.from({length:51},(_,i)=>raceRow(10000+i,'分页边界事项 '+i));
let boundaryDone=false;const boundaryPages=[];
invoke=(command,args)=>{
  if(command==='app_settings_get')return Promise.resolve({devify:{active_space_id:'boundary-space'}});
  if(command==='devify_analysis_list'){
    boundaryPages.push(args.page);
    if(args.page===2&&boundaryDone)return Promise.reject('无效页码（404）');
    const rows=boundaryDone?boundaryRows.slice(0,50):boundaryRows;
    return Promise.resolve({list:rows.slice((args.page-1)*50,args.page*50),pagination:{total:rows.length,next:args.page===1&&rows.length>50?'next':null},relevance_counts:{my_todo:rows.length,related:boundaryDone?1:0,other:0}});
  }
  if(command==='devify_analysis_detail')return Promise.resolve(boundaryRows.find(row=>row.id===args.itemId));
  if(command==='devify_event_notes'||command==='devify_event_relations')return Promise.resolve([]);
  if(command==='devify_event_update'){boundaryDone=true;return Promise.resolve({...boundaryRows.at(-1),status:'done',viewer_relevance:'related'});}
  throw Error(command);
};
const boundaryInstance={hooks:[]};
const settleBoundary=async()=>{for(let i=0;i<6;i++){render(boundaryInstance,'events');await flush();}return render(boundaryInstance,'events');};
let boundaryTree=await settleBoundary();
find(boundaryTree,n=>n.type==='button'&&text(n)==='下一页').props.onClick();boundaryTree=await settleBoundary();
assert.equal(boundaryPages.at(-1),2);
find(boundaryTree,n=>n.type==='button'&&text(n)==='标记完成').props.onClick();boundaryTree=await settleBoundary();
assert.equal(boundaryPages.at(-1),1);
assert.ok(!text(boundaryTree).includes('无效页码'));
assert.match(text(byClass(boundaryTree,'analysis-pills')),/我的待办50与我有关1/);
assert.equal(elements(byClass(boundaryTree,'analysis-items')).filter(n=>n.type==='button').length,50);
console.log('Completing the last item on the last page reloads page one and refreshed category counts');

// Server search debounces, resets paging, preserves global badges and rejects late results.
const searchCalls=[],searchPending=[];
let searchOwner=31,searchAvailable=true,searchService='https://search.invalid';
invoke=(command,args)=>{
 if(command==='app_settings_get')return Promise.resolve({devify:{base_url:searchService,active_space_id:'search-fixture'}});
 if(command==='devify_analysis_list'){
  searchCalls.push({...args,service:searchService});
  if(args.searchQuery&&searchAvailable)return new Promise(resolve=>searchPending.push({args,resolve}));
  return Promise.resolve({list:[raceRow(701,'未筛选第一页')],search_supported:searchAvailable,pagination:{total:120,next:'page=2'},relevance_counts:pageCounts});
 }
 if(command==='devify_analysis_detail')return Promise.resolve(raceRow(args.itemId,'搜索详情'));
 if(command==='devify_event_notes'||command==='devify_event_relations')return Promise.resolve([]);
 throw Error(command);
};
const searchInstance={hooks:[]};
const renderSearch=()=>renderPage(searchInstance,()=>CloudAnalysisPage({kind:'events',userId:searchOwner}));
const settleSearch=async()=>{for(let i=0;i<5;i++){renderSearch();await flush();}return renderSearch();};
let searchTree=await settleSearch();
find(searchTree,n=>n.type==='button'&&text(n)==='下一页').props.onClick();searchTree=await settleSearch();
const inputSearch=(value)=>find(searchTree,n=>n.type==='input').props.onChange({target:{value}});
const waitSearch=async()=>{await new Promise(resolve=>setTimeout(resolve,380));return settleSearch();};
const beforeTyping=searchCalls.length;
inputSearch('a');searchTree=await settleSearch();inputSearch('alpha');searchTree=await settleSearch();
assert.equal(searchCalls.length,beforeTyping,'Typing does not request the old keyword/page');
searchTree=await waitSearch();
assert.equal(searchPending.at(-1).args.page,1);assert.equal(searchPending.at(-1).args.searchQuery,'alpha');
inputSearch('beta');searchTree=await settleSearch();searchTree=await waitSearch();
searchPending.at(-1).resolve({list:[raceRow(702,'服务端跨页命中')],search_supported:true,pagination:{total:1,next:null},relevance_counts:pageCounts});searchTree=await settleSearch();
searchPending[0].resolve({list:[raceRow(703,'迟到搜索结果')],search_supported:true,pagination:{total:999,next:null},relevance_counts:{my_todo:999,related:0,other:0}});searchTree=await settleSearch();
assert.ok(text(byClass(searchTree,'analysis-items')).includes('服务端跨页命中'),'Server matches are not re-filtered using a different local search algorithm');
assert.ok(!text(searchTree).includes('迟到搜索结果'));
assert.match(text(byClass(searchTree,'analysis-pills')),/我的待办60与我有关41/);
selectFilter(searchTree,'与我有关');searchTree=await settleSearch();
assert.equal(searchPending.at(-1).args.viewerRelevance,'related');assert.equal(searchPending.at(-1).args.searchQuery,'beta');assert.equal(searchPending.at(-1).args.page,1);
searchPending.at(-1).resolve({list:[],search_supported:true,pagination:{total:0,next:null},relevance_counts:pageCounts});searchTree=await settleSearch();
find(searchTree,n=>n.type==='button'&&text(n)==='清除搜索').props.onClick();searchTree=await settleSearch();searchTree=await waitSearch();
assert.equal(searchCalls.at(-1).searchQuery,undefined);assert.equal(searchCalls.at(-1).page,1);
const beforeTooLong=searchCalls.length;inputSearch('中'.repeat(201));searchTree=await settleSearch();searchTree=await waitSearch();
assert.equal(searchCalls.length,beforeTooLong);assert.ok(text(searchTree).includes('最多输入 200 个字符'));
inputSearch('downgrade');searchTree=await settleSearch();searchTree=await waitSearch();
searchAvailable=false;searchPending.at(-1).resolve({list:[raceRow(704,'不应冒充全局搜索')],pagination:{total:1,next:null},relevance_counts:pageCounts});searchTree=await settleSearch();
assert.equal(find(searchTree,n=>n.type==='input').props['aria-label'],'搜索当前页待办');
assert.ok(!text(searchTree).includes('不应冒充全局搜索'));
searchOwner=32;searchTree=await settleSearch();
assert.equal(find(searchTree,n=>n.type==='input').props.value,'');
assert.equal(searchCalls.at(-1).searchQuery,undefined);
searchOwner=33;searchAvailable=true;searchTree=await settleSearch();
inputSearch('private-keyword');searchTree=await settleSearch();searchTree=await waitSearch();
searchPending.at(-1).resolve({list:[raceRow(705,'旧服务搜索结果')],search_supported:true,pagination:{total:1,next:null},relevance_counts:pageCounts});searchTree=await settleSearch();
searchService='https://new-search.invalid';
find(searchTree,n=>n.type==='button'&&text(n)==='刷新').props.onClick();searchTree=await settleSearch();
assert.equal(find(searchTree,n=>n.type==='input').props.value,'');
assert.ok(searchCalls.filter(call=>call.service===searchService).every(call=>!call.searchQuery),'An old keyword is never sent to a new service before capability discovery');
console.log('Debounced remote search, category/query paging, global counts, stale results, Unicode limit and downgrade/account isolation passed');

// Opening the linking dialog preserves the original space-wide candidate scope.
invoke=(command,args)=>{
  if(command==='app_settings_get')return Promise.resolve({devify:{active_space_id:'link-fixture'}});
  if(command==='devify_analysis_list')return Promise.resolve({list:[myEvent,...(!args.pageSize?[completedEvent]:[])],pagination:{total:1},relevance_counts:eventCounts});
  if(command==='devify_analysis_detail')return Promise.resolve(myEvent);
  if(command==='devify_event_notes'||command==='devify_event_relations')return Promise.resolve([]);
  throw Error(command);
};
const linkInstance={hooks:[]};
const settleLinks=async()=>{for(let i=0;i<4;i++){render(linkInstance,'events');await flush();}return render(linkInstance,'events');};
let linkTree=await settleLinks();
find(linkTree,node=>node.type==='button'&&text(node)==='关联待办').props.onClick();
linkTree=await settleLinks();
assert.ok(text(byClass(linkTree,'analysis-picker-list')).includes(completedEvent.title),'Linking includes other pages and personal categories');

// A true empty space directs users to analysis settings; a filtered-empty list
// continues to offer View All, without implying that event analysis failed.
invoke=command=>{
  if(command==='app_settings_get')return Promise.resolve({devify:{active_space_id:'empty-fixture'}});
  if(command==='devify_analysis_list')return Promise.resolve({list:[],relevance_counts:{my_todo:0,related:0,other:0}});
  if(command==='devify_event_relations')return Promise.resolve([]);
  throw Error(command);
};
const emptyInstance={hooks:[]};
for(let i=0;i<4;i++){render(emptyInstance,'events');await flush();}
const emptyTree=render(emptyInstance,'events');
assert.ok(text(emptyTree).includes('大约每小时分析一次'));
assert.ok(text(emptyTree).includes('智能分析 → 聊天待办分析'));
assert.ok(!text(emptyTree).includes('查看全部待办'));
console.log('Event empty-space analysis cadence and settings guidance passed');

mocks['./chatPresentation'] = load('../src/features/chat/chatPresentation.ts');
const { ChatEventsPane } = load('../src/features/chat/ChatEventsPane.tsx');
const chatEvent = { id: 1, title: '测试待办' };
const renderChatEvents = (events, query = '') => renderPage({ hooks: [] }, () => ChatEventsPane({
  active: true,
  loading: true,
  error: '',
  events,
  sessionCount: 1,
  query,
  filter: 'all',
  onRetry: () => {},
  onSchedule: () => {},
  onOpenEvidence: () => {},
}));
mocks['./loading-skeleton.css'] = {};
const { LoadingSkeleton, LoadingPage } = load('../src/shared/ui/LoadingSkeleton.tsx');
const skeleton = mocks['../../shared/ui/LoadingSkeleton'].LoadingSkeleton;
for (const kind of ['list', 'messages', 'detail']) {
  const indicator = LoadingSkeleton({ label: '正在加载', kind });
  assert.equal(indicator.props.role, 'status');
  assert.equal(text(indicator), '正在加载');
  assert.ok(byClass(indicator, 'loading-skeleton-spinner'));
  assert.equal(elements(indicator).length, 3, 'The indicator contains only the label and spinner');
}
assert.ok(!elements(LoadingPage({ title: '待办' })).some(node => node.props?.className === 'loading-page-search'));
const firstChatLoad = renderChatEvents([]);
assert.equal(find(firstChatLoad, node => node.type === skeleton).props.label, '正在加载待办');
for (const query of ['', '没有匹配']) {
  const tree = renderChatEvents([chatEvent], query);
  const status = byClass(tree, 'chat-events-status');
  assert.ok(byClass(status, 'loading-skeleton-spinner'));
  assert.ok(text(status).includes('正在同步待办'));
  assert.ok(!elements(tree).some(node => node.type === skeleton), 'Refresh does not insert an indicator into the feed');
  if (!query) assert.ok(text(tree).includes(chatEvent.title));
}

// The current sidebar range must not rewrite the scope shown on an old answer.
mocks['./analysisSettingsText'] = load('../src/features/analysis/analysisSettingsText.ts');
mocks['./askAnswer'] = load('../src/features/analysis/askAnswer.ts');
const { AskPage } = load('../src/features/analysis/Ask.tsx');
const savedStart = '2026-10-08T10:15:00Z';
const savedEnd = '2026-10-09T10:15:00Z';
const question = {
  id: 701, question: '有历史回答的问题', answer: '原有回答', citations: [],
  status: 'completed', created_at: new Date().toISOString(),
  scope: { since: savedStart, until: savedEnd },
};
invoke = async command => {
  if (command === 'app_settings_get') return { devify: { active_space_id: 'ask-fixture' } };
  if (command === 'devify_questions') return [question];
  throw Error(command);
};
const askInstance = { hooks: [] };
const renderAsk = () => renderPage(askInstance, () => AskPage({}));
assert.equal(renderAsk().type, mocks['../../shared/ui/LoadingSkeleton'].LoadingPage);
await flush();
const askTree = renderAsk();
const answerFooter = tree => text(byClass(tree, 'ask-foot'));
const shortDate = value => new Date(value).toLocaleString('zh-CN', {
  month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit',
});
assert.ok(answerFooter(askTree).includes(shortDate(savedStart) + ' – ' + shortDate(savedEnd)));
assert.ok(answerFooter(askTree).includes('个待办'));
find(askTree, node => node.type === 'button' && text(node) === '今天').props.onClick();
assert.equal(answerFooter(renderAsk()), answerFooter(askTree));
console.log('Saved question scope remains visible after changing the sidebar range');

invoke = command => {
  if (command === 'devify_question_ask') return new Promise(() => {});
  throw Error(command);
};
find(renderAsk(), node => node.type === 'textarea').props.onChange({ target: { value: '继续提问' } });
find(renderAsk(), node => node.props?.['aria-label'] === '发送').props.onClick();
const pendingAsk = renderAsk();
assert.ok(text(pendingAsk).includes(question.answer), 'A pending answer must not hide earlier turns');
assert.ok(byClass(byClass(pendingAsk, 'ask-pending'), 'ask-spinner'));

const { TopicsPage } = load('../src/features/analysis/Topics.tsx');
const topics = [
  { id: 801, title: '项目甲', instruction: '甲详情', enabled: true, period: 'day' },
  { id: 802, title: '项目乙', instruction: '乙详情', enabled: true, period: 'day' },
];
const topicRequests = [];
invoke = async (command, args) => {
  if (command === 'app_settings_get') return { devify: { active_space_id: 'topic-fixture' } };
  if (command === 'devify_topics') return topics;
  if (command === 'devify_topic_detail') {
    topicRequests.push(args.topicId);
    return { ...topics.find(topic => topic.id === args.topicId), matched_event_ids: [37] };
  }
  throw Error(command);
};
const topicsInstance = { hooks: [] };
const renderTopics = () => renderPage(topicsInstance, () => TopicsPage({ userId: null }));
const settleTopics = async () => {
  renderTopics();
  await flush();
  return renderTopics();
};
await settleTopics();
let topicTree = await settleTopics();
assert.ok(text(byClass(topicTree, 'topic-detail-wrap')).includes('甲详情'));
const searchTopic = (tree, value) => byClass(tree, 'topic-side-search').props.onChange({ target: { value } });
searchTopic(topicTree, '项目乙');
topicTree = await settleTopics();
assert.ok(text(byClass(topicTree, 'topic-detail-wrap')).includes('乙详情'));
assert.ok(text(byClass(topicTree, 'topic-detail-wrap')).includes('待办 #37'));
assert.deepEqual(topicRequests, [801, 802]);
searchTopic(topicTree, 'nothing matches');
topicTree = await settleTopics();
assert.ok(!elements(topicTree).some(node => node.props?.className === 'topic-detail-wrap'));
find(topicTree, node => node.type === 'button' && text(node) === '清除搜索').props.onClick();
topicTree = await settleTopics();
assert.ok(text(byClass(topicTree, 'topic-detail-wrap')).includes('甲详情'));
console.log('Topic detail follows visible search results and clears when nothing matches');

const topicSpace = 'cached-topic-space';
const topicUser = 901;
const snapshotKey = JSON.stringify([undefined, topicUser, topicSpace, 'topics']);
mocks['../../shared/state/pageSnapshots'].savePageSnapshot(snapshotKey, topics);
let resolveTopicList;
invoke = (command, args) => {
  if (command === 'app_settings_get') return Promise.resolve({ devify: { active_space_id: topicSpace } });
  if (command === 'devify_topics') return new Promise(resolve => { resolveTopicList = resolve; });
  if (command === 'devify_topic_detail') return Promise.resolve(topics.find(topic => topic.id === args.topicId));
  throw Error(command);
};
const cachedTopicsInstance = { hooks: [] };
const renderCachedTopics = (userId = topicUser) => renderPage(cachedTopicsInstance, () => TopicsPage({ userId }));
assert.equal(renderCachedTopics().type, mocks['../../shared/ui/LoadingSkeleton'].LoadingPage);
await flush();
let cachedTopicsTree = renderCachedTopics();
await flush();
cachedTopicsTree = renderCachedTopics();
assert.ok(text(byClass(cachedTopicsTree, 'topic-side-list')).includes(topics[0].title));
assert.ok(text(byClass(cachedTopicsTree, 'topic-detail-wrap')).includes(topics[0].instruction));
assert.ok(byClass(byClass(cachedTopicsTree, 'topic-loading-status'), 'loading-skeleton-spinner'));
resolveTopicList(topics);
await flush();
assert.ok(!elements(renderCachedTopics()).some(node => node.props?.className === 'topic-loading-status'));
assert.equal(renderCachedTopics(topicUser + 1).type, mocks['../../shared/ui/LoadingSkeleton'].LoadingPage);
await flush();
assert.ok(!text(renderCachedTopics(topicUser + 1)).includes(topics[0].title));
console.log('Topic refresh keeps cached content in place and clears it on account change');
