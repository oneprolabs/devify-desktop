import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';

// Real task page: switching identity must discard a delayed cloud-history result.
let instance, runtime = {context:'one',paused:false,running:false,pending_config:0,jobs:[]};
const react = {
  useState(initial) {const i=instance.index++;if(!(i in instance.hooks))instance.hooks[i]=initial;return [instance.hooks[i],value=>{instance.hooks[i]=typeof value==='function'?value(instance.hooks[i]):value;}];},
  useRef(initial) {const i=instance.index++;return instance.hooks[i]??={current:initial};},
  useEffect(fn,deps) {const i=instance.index++,old=instance.hooks[i];if(!old||deps.some((v,j)=>v!==old.deps[j]))instance.effects.push(()=>{old?.cleanup?.();instance.hooks[i]={deps,cleanup:fn()};});},
};
const jsx={jsx:(type,props)=>({type,props}),jsxs:(type,props)=>({type,props})};
let finishCloud;
let finishNames;
const calls=[];
const nameCalls=[];
const invoke=async (command,args)=>{calls.push(command);if(command==='chat_sessions'){nameCalls.push(args);return new Promise(resolve=>{finishNames=resolve;});}if(command==='app_settings_get')return {devify:{active_space_id:'space'}};if(command==='devify_upload_list')return new Promise(resolve=>{finishCloud=resolve;});throw Error(command);};
const output=ts.transpileModule(fs.readFileSync(new URL('../src/features/tasks/Tasks.tsx',import.meta.url),'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2020}}).outputText;
const module={exports:{}};
new Function('require','module','exports',output)(id=>{
 if(id==='react')return react;
 if(id==='react/jsx-runtime')return jsx;
 if(id==='../../shared/i18n/locale')return {t:s=>s,tf:(s,...args)=>s.replace(/\{(\d+)\}/g,(_,n)=>String(args[Number(n)]))};
 if(id==='../../shared/api/desktopInvoke')return {desktopInvoke:invoke};
 if(id==='./taskRuntime')return {useTaskRuntime:()=>runtime,refreshTaskRuntime:async()=>{}};
 if(id==='../../shared/ui/WizardActionButton')return {WizardActionButton:()=>null};
 if(id==='../backup/BackupRuntimeSwitch')return {BackupRuntimeSwitch:()=>null};
 throw Error(id);
},module,module.exports);
instance={hooks:[]};
function render(){instance.index=0;instance.effects=[];const tree=module.exports.Tasks({onNewUpload(){}});instance.effects.forEach(fn=>fn());return tree;}
function all(node){return Array.isArray(node)?node.flatMap(all):node&&typeof node==='object'?[node,...all(node.props?.children)]:[];}
function text(node){return Array.isArray(node)?node.map(text).join(''):node&&typeof node==='object'?text(node.props?.children):String(node??'');}
const flush=async()=>{for(let i=0;i<8;i++)await Promise.resolve();};
let tree=render();
assert.deepEqual(calls,[],'mount only consumes the shared local snapshot');
runtime={...runtime,sources:[{account_id:'wa',platform:'whatsapp',setup_complete:false,backup_enabled:true}]};
tree=render();
assert.ok(text(tree).includes('WhatsApp 配置未完成，备份已暂停。'),'unfinished sources are visible even without current jobs');
runtime={...runtime,sources:[{account_id:'wa',platform:'whatsapp',setup_complete:true,backup_enabled:true}]};
tree=render();
assert.ok(!text(tree).includes('配置未完成'),'the notice clears when configuration finishes');
const button=all(tree).find(n=>n.props?.children==='查看云端历史');
const pending=button.props.onAction();await flush();assert.equal(typeof finishCloud,'function');
runtime={...runtime,context:'two'};render();
finishCloud({list:[{id:1,status:'completed',message_count:999,source_type:'wechat',error_message:'old private result'}],pagination:{next:null}});
await pending;tree=render();assert.ok(!text(tree).includes('old private result'));assert.ok(!text(tree).includes('999'));
assert.deepEqual(calls,['app_settings_get','devify_upload_list']);
runtime={...runtime,jobs:[{id:'job',account_id:'wechat',status:'running',message:'正在准备',updated_at:'2026-01-01',scope:{usernames:['group','person','third']}}]};
render();await flush();
assert.deepEqual(nameCalls,[{accountId:'wechat',focusUsernames:['group','person'],limit:2}],'batch only the first two names of visible tasks');
finishNames([{username:'group',display:'项目讨论群'},{username:'person',display:'联系人'}]);await flush();tree=render();
assert.ok(text(tree).includes('备份微信 · 项目讨论群、联系人等 3 个会话'));
runtime={...runtime,jobs:runtime.jobs.map(job=>({...job,status:'completed',message:'已完成'}))};render();await flush();
assert.equal(nameCalls.length,1,'progress updates do not read names again');
runtime={...runtime,context:'three'};tree=render();await flush();
assert.ok(!text(tree).includes('项目讨论群'),'an identity switch hides names immediately');
runtime={...runtime,context:'four',jobs:[]};render();finishNames([{username:'group',display:'旧账号的群'}]);await flush();tree=render();
assert.ok(!text(tree).includes('旧账号的群'),'late name responses cannot cross identity boundaries');
const batch = { account_id: 'wechat', status: 'completed', message: '本批未新增消息', updated_at: '2026-01-02T12:00:00' };
runtime = { ...runtime, jobs: [
  { ...batch, id: 'latest' },
  { ...batch, id: 'legacy', message: '服务端解析完成，共 0 条消息', updated_at: '2026-01-02T11:00:00' },
  { ...batch, id: 'other-day', updated_at: '2026-01-01T12:00:00' },
  { ...batch, id: 'other-account', account_id: 'other' },
  { ...batch, id: 'with-messages', message: '服务端解析完成，共 3 条消息' },
] };
const rows = all(render()).filter(node => node.type === 'article').map(text);
assert.equal(rows.length, 4, 'only empty batches for the same account and day collapse');
assert.equal(rows.filter(row => row.includes('当天 2 次')).length, 1);
assert.ok(rows.some(row => row.includes('服务端解析完成，共 3 条消息')));
console.log('Task page uses bounded local names, skips progress rescans and rejects old-identity names/history');
