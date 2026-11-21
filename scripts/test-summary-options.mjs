import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
import { summaryOptionFields } from '../src/features/chat/summary/summaryOptions.ts';
import { validateCloudSummaryInput } from '../src/features/chat/summary/summaryInput.ts';

const config={schema_version:1,version:'v7',default_scenario:'three_part',max_instructions_chars:4,scenarios:[{id:'summary',sha256:'s'},{id:'three_part',sha256:'t'},{id:'custom',sha256:'c',requires_instructions:true}]};
const view={config,language:'English',ui_language:'中文',instructions:'saved'};
assert.equal(summaryOptionFields(view,{scenario:'three_part',language:'',instructions:''}).scenario,'three_part');
assert.equal(summaryOptionFields(view,{scenario:'summary',language:'',instructions:''}).language,'English');
assert.equal(summaryOptionFields(view,{scenario:'summary',language:'跟随界面',instructions:''}).language,'中文');
assert.equal(summaryOptionFields(view,{scenario:'summary',language:'按原文语言',instructions:''}).language,'按原文语言');
assert.throws(()=>summaryOptionFields(view,{scenario:'custom',language:'',instructions:'  '}),/填写/);
assert.throws(()=>summaryOptionFields(view,{scenario:'custom',language:'',instructions:'😀'.repeat(5)}),/过长/);
assert.equal(summaryOptionFields(view,{scenario:'custom',language:'es',instructions:'  😀😀  '}).instructions,'😀😀');
assert.throws(()=>summaryOptionFields(null,{scenario:'',language:'',instructions:''}),/初始化/);
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
invoke=command=>{calls.push(command);if(command==='summary_config_refresh')return Promise.resolve();return new Promise((resolve,reject)=>pending.push({resolve,reject}));};
const flush=async()=>{for(let i=0;i<30;i++)await Promise.resolve();};
render('first');render('second');
pending[1].resolve(view);await flush();let model=render('second');assert.equal(model.draft.scenario,'three_part','default is not the first array item');assert.equal(model.draft.instructions,'saved');
pending[0].resolve({...view,language:'old-user'});await flush();model=render('second');assert.equal(model.view.language,'English','late old-account responses are discarded');
model.setDraft(old=>({...old,scenario:'custom',instructions:'mine',language:'按原文语言'}));model=render('second');
const reload=model.reload();pending[2].resolve({...view,config:{...config,version:'v8'}});await reload;model=render('second');assert.equal(model.draft.instructions,'mine');assert.equal(model.draft.language,'按原文语言');
const count=calls.length;render('second');await flush();assert.equal(calls.length,count,'rerenders do not download or poll');assert.ok(!calls.includes('summary_config_refresh'));
const reset=model.initialize();await flush();pending[3].resolve(view);await reset;assert.equal(calls.filter(c=>c==='summary_config_refresh').length,1,'initialization is explicit');
model=render('second');const detach=model.reload();owner.hooks.forEach(h=>h?.cleanup?.());pending[4].resolve({...view,language:'detached'});await detach;
assert.equal(owner.hooks.filter(h=>h?.language==='detached').length,0);
console.log('Summary options: catalog default, explicit languages, Unicode/custom validation, escaped-input budgets, cache-only reads, late account responses and preserved drafts passed');
