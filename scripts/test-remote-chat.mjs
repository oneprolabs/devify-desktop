import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
let instance;
const react={
 useState(initial){const i=instance.index++;if(!(i in instance.hooks))instance.hooks[i]=initial;return [instance.hooks[i],v=>{instance.hooks[i]=typeof v==='function'?v(instance.hooks[i]):v;}];},
 useRef(initial){const i=instance.index++;return instance.hooks[i]??={current:initial};},
 useEffect(fn,deps){const i=instance.index++,old=instance.hooks[i];if(!old||deps.some((v,j)=>v!==old.deps[j]))instance.effects.push(()=>{old?.cleanup?.();instance.hooks[i]={deps,cleanup:fn()};});},
};
const jsx={jsx:(type,props)=>({type,props}),jsxs:(type,props)=>({type,props})};
let finishLookup;
const calls=[];
const invoke=async command=>{calls.push(command);if(command==='app_settings_get')return {devify:{active_space_id:'space'}};if(command==='chat_remote_lookup')return new Promise(resolve=>{finishLookup=resolve;});throw Error(command);};
const source=fs.readFileSync(new URL('../src/RemoteChatFallback.tsx',import.meta.url),'utf8');
const code=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2020}}).outputText;
const mod={exports:{}};
new Function('require','module','exports',code)(id=>{
 if(id==='react')return react;
 if(id==='react/jsx-runtime')return jsx;
 if(id==='./desktopInvoke')return {desktopInvoke:invoke};
 if(id==='./locale')return {t:s=>s};
 if(id==='./WizardActionButton')return {WizardActionButton:()=>null};
 throw Error(id);
},mod,mod.exports);
instance={hooks:[]};
function render(){instance.index=0;instance.effects=[];const tree=mod.exports.RemoteChatFallback({accountId:'wechat',username:'missing'});instance.effects.forEach(fn=>fn());return tree;}
function text(n){return Array.isArray(n)?n.map(text).join(''):n&&typeof n==='object'?text(n.props?.children):String(n??'');}
let tree=render();assert.ok(text(tree).includes('正在查找云端备份'));assert.ok(!text(tree).includes('暂时没有可查看的记录'));
finishLookup({conversation:null});for(let i=0;i<12;i++)await Promise.resolve();tree=render();
assert.ok(text(tree).includes('暂时没有可查看的记录'));
assert.ok(text(tree).includes('重新检查备份'));
assert.ok(!text(tree).includes('正在查找云端备份'));
assert.ok(!text(tree).includes('正在查看云端备份'));
assert.deepEqual(calls,['app_settings_get','chat_remote_lookup']);
console.log('Missing-record lookup shows one current state and a clear retry action');
