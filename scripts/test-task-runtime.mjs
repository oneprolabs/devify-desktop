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
const calls=[];
const invoke=async command=>{calls.push(command);if(command==='app_settings_get')return {devify:{active_space_id:'space'}};if(command==='devify_upload_list')return new Promise(resolve=>{finishCloud=resolve;});throw Error(command);};
const output=ts.transpileModule(fs.readFileSync(new URL('../src/Tasks.tsx',import.meta.url),'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2020}}).outputText;
const module={exports:{}};
new Function('require','module','exports',output)(id=>{
 if(id==='react')return react;
 if(id==='react/jsx-runtime')return jsx;
 if(id==='./locale')return {t:s=>s};
 if(id==='./desktopInvoke')return {desktopInvoke:invoke};
 if(id==='./taskRuntime')return {useTaskRuntime:()=>runtime,refreshTaskRuntime:async()=>{}};
 if(id==='./WizardActionButton')return {WizardActionButton:()=>null};
 throw Error(id);
},module,module.exports);
instance={hooks:[]};
function render(){instance.index=0;instance.effects=[];const tree=module.exports.Tasks({onNewUpload(){}});instance.effects.forEach(fn=>fn());return tree;}
function all(node){return Array.isArray(node)?node.flatMap(all):node&&typeof node==='object'?[node,...all(node.props?.children)]:[];}
function text(node){return Array.isArray(node)?node.map(text).join(''):node&&typeof node==='object'?text(node.props?.children):String(node??'');}
const flush=async()=>{for(let i=0;i<8;i++)await Promise.resolve();};
let tree=render();
assert.deepEqual(calls,[],'mount only consumes the shared local snapshot');
const button=all(tree).find(n=>n.props?.children==='查看云端历史');
const pending=button.props.onAction();await flush();assert.equal(typeof finishCloud,'function');
runtime={...runtime,context:'two'};render();
finishCloud({list:[{id:1,status:'completed',message_count:999,source_type:'wechat',error_message:'old private result'}],pagination:{next:null}});
await pending;tree=render();assert.ok(!text(tree).includes('old private result'));assert.ok(!text(tree).includes('999'));
assert.deepEqual(calls,['app_settings_get','devify_upload_list']);
console.log('Task page performs no mount-time cloud read and rejects old-identity history responses');
