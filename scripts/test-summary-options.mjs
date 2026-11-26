import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
import { createElement } from 'react';
import { jsx, jsxs, Fragment } from 'react/jsx-runtime';
import { renderToStaticMarkup } from 'react-dom/server';
import { summaryOptionFields, summaryOutputLanguage } from '../src/features/chat/summary/summaryOptions.ts';
import { validateCloudSummaryInput } from '../src/features/chat/summary/summaryInput.ts';

const config={schema_version:1,version:'v7',default_scenario:'three_part',max_instructions_chars:4,scenarios:[{id:'summary',sha256:'s'},{id:'three_part',sha256:'t'},{id:'custom',sha256:'c',requires_instructions:true}]};
const view={config,language:'English',ui_language:'中文',instructions:'saved'};
assert.equal(summaryOptionFields(view,{scenario:'three_part',language:'',instructions:''}).scenario,'three_part');
assert.equal(summaryOptionFields(view,{scenario:'summary',language:'',instructions:''}).language,'English');
assert.equal(summaryOptionFields(view,{scenario:'summary',language:'跟随界面',instructions:''}).language,'中文');
assert.equal(summaryOptionFields(view,{scenario:'summary',language:'按原文语言',instructions:''}).language,'按原文语言');
const languageDraft = {scenario:'summary',language:'',instructions:''};
for (const language of ['中文','English','按原文语言']) {
  const saved = {...view,language};
  assert.equal(summaryOutputLanguage(saved,languageDraft),language,'show the saved language directly');
  assert.equal(summaryOptionFields(saved,languageDraft).language,language);
}
assert.equal(summaryOutputLanguage({...view,language:'跟随界面'},languageDraft),'中文','resolve the setting without adding an interface-language option');
assert.equal(summaryOutputLanguage(view,{...languageDraft,language:'中文'}),'中文');
assert.equal(view.language,'English','per-request choices do not change the saved default');
assert.throws(()=>summaryOptionFields(view,{scenario:'custom',language:'',instructions:'  '}),/填写/);
assert.throws(()=>summaryOptionFields(view,{scenario:'custom',language:'',instructions:'😀'.repeat(5)}),/过长/);
assert.equal(summaryOptionFields(view,{scenario:'custom',language:'es',instructions:'  😀😀  '}).instructions,'😀😀');
assert.throws(()=>summaryOptionFields(null,{scenario:'',language:'',instructions:''}),/没有可用的总结配置/);
const input={title:'Group',range_label:'Today',language:'English',messages:[{username:'g',display:'Group',sender:'Member',time:1,text:'x'.repeat(3100)}]};
assert.doesNotThrow(()=>validateCloudSummaryInput(input,{max_content_bytes:4000}),'catalog budget replaces the old fixed character ceiling');
assert.throws(()=>validateCloudSummaryInput({...input,instructions:'"'.repeat(600)},{max_content_bytes:4000}),/超过/,'escaped instructions consume the same compact input budget');

// Execute the actual cache hook with controlled IPC completion order.
let current,invoke;
const equal=(a,b)=>a&&b&&a.length===b.length&&a.every((x,i)=>Object.is(x,b[i]));
const react={
 useState(initial){const owner=current,i=owner.i++;if(!(i in owner.hooks))owner.hooks[i]=typeof initial==='function'?initial():initial;return[owner.hooks[i],value=>owner.hooks[i]=typeof value==='function'?value(owner.hooks[i]):value];},
 useRef(value){const i=current.i++;return current.hooks[i]??={current:value};},
 useCallback(fn,deps){const i=current.i++;if(!equal(current.hooks[i]?.deps,deps))current.hooks[i]={deps,fn};return current.hooks[i].fn;},
 useEffect(fn,deps){const owner=current,i=owner.i++,old=owner.hooks[i];if(!equal(old?.deps,deps))owner.effects.push(()=>{old?.cleanup?.();owner.hooks[i]={deps,cleanup:fn()};});},
};
const output=ts.transpileModule(fs.readFileSync(new URL('../src/features/chat/summary/useSummaryOptions.ts',import.meta.url),'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
const module={exports:{}};
new Function('require','module','exports',output)(name=>name==='react'?react:{desktopInvoke:(...args)=>invoke(...args)},module,module.exports);
const {useSummaryOptions}=module.exports;
const owner={hooks:[],i:0,effects:[]};
function render(scope='first',open=true){current=owner;owner.i=0;owner.effects=[];const result=useSummaryOptions(scope,open);owner.effects.forEach(fn=>fn());return result;}
const pending=[],calls=[];
invoke=command=>{calls.push(command);return new Promise((resolve,reject)=>pending.push({resolve,reject}));};
const flush=async()=>{for(let i=0;i<30;i++)await Promise.resolve();};
render('first');render('second');
pending[1].resolve(view);await flush();let model=render('second');assert.equal(model.draft.scenario,'three_part','default is not the first array item');assert.equal(model.draft.instructions,'saved');
pending[0].resolve({...view,language:'old-user'});await flush();model=render('second');assert.equal(model.view.language,'English','late old-account responses are discarded');
model.setDraft(old=>({...old,scenario:'custom',instructions:'mine',language:'按原文语言'}));model=render('second');
const reload=model.reload();pending[2].resolve({...view,config:{...config,version:'v8'}});await reload;model=render('second');assert.equal(model.draft.instructions,'mine');assert.equal(model.draft.language,'按原文语言');
const count=calls.length;render('second');await flush();assert.equal(calls.length,count,'rerenders do not poll');assert.ok(calls.every(command=>command==='summary_config_get'));
const switched=render('third');
assert.equal(switched.view,null,'never render the previous scope’s options, even before effects');
const failure={message:'无法连接总结服务，请检查网络。',retry_at:Date.now()/1000+60};
pending[3].reject(failure);await flush();model=render('third');
assert.deepEqual(model.error,failure,'show the native reason and retry time, not a generic cache error');
const retry=model.reload();await model.reload();assert.equal(pending.length,5,'concurrent retries share a single read');
pending[4].resolve(view);await retry;model=render('third');
assert.equal(model.view.config.version,'v7','retry recovers without closing the form');
const detach=model.reload();owner.hooks.forEach(h=>h?.cleanup?.());pending[5].resolve({...view,language:'detached'});await detach;
assert.equal(owner.hooks.filter(h=>h?.language==='detached').length,0);

const optionsJs = ts.transpileModule(fs.readFileSync(new URL('../src/features/chat/summary/SummaryRequestOptions.tsx', import.meta.url), 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX },
}).outputText;
const optionsModule = { exports: {} };
new Function('require', 'exports', optionsJs)(
  name => ({
    'react/jsx-runtime': { jsx, jsxs, Fragment },
    '../../../shared/i18n/locale': { t: text => text, tf: (text, arg) => text.replace('{0}', String(arg)), dateLocale: () => 'zh-CN' },
    './summaryOptions': { summaryOutputLanguage },
  })[name], optionsModule.exports,
);
const statusHtml = (loading, error) => renderToStaticMarkup(createElement(optionsModule.exports.SummaryRequestOptions, {
  view: null, draft: { scenario: '', language: '', instructions: '' }, loading, error, onReload: () => {},
}));
assert.match(statusHtml(true, null), /role="status">正在读取总结方式/);
assert.doesNotMatch(statusHtml(true, null), /<button/);
const failedHtml = statusHtml(false, failure);
assert.equal(failedHtml.match(/role="alert"/g)?.length, 1);
assert.match(failedHtml, /无法连接总结服务.*可在 .* 后重试/);
assert.match(failedHtml, /<button type="button">重试<\/button>/);
assert.doesNotMatch(failedHtml, /重新初始化/);
console.log('Summary options: defaults, validation, scoped reads, recovery and preserved drafts passed');
