import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';

let owner, invoke;
const same=(a,b)=>a&&b&&a.length===b.length&&a.every((x,i)=>Object.is(x,b[i]));
const react={
 useState(initial){const at=owner.index++,self=owner;if(!(at in self.hooks))self.hooks[at]=typeof initial==='function'?initial():initial;return[self.hooks[at],next=>self.hooks[at]=typeof next==='function'?next(self.hooks[at]):next];},
 useRef(initial){const at=owner.index++;return owner.hooks[at]??={current:initial};},
 useEffect(effect,deps){const at=owner.index++,self=owner,previous=self.hooks[at];if(!same(previous?.deps,deps))self.effects.push(()=>{previous?.cleanup?.();self.hooks[at]={deps,cleanup:effect()};});},
 useCallback(fn,deps){const at=owner.index++;if(!same(owner.hooks[at]?.deps,deps))owner.hooks[at]={deps,fn};return owner.hooks[at].fn;},
 useId(){return 'agent-select-test';},
};
const cache=new Map();
function load(path){const file=new URL(path,import.meta.url).href;if(cache.has(file))return cache.get(file);const output=ts.transpileModule(fs.readFileSync(new URL(file),'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020,jsx:ts.JsxEmit.ReactJSX}}).outputText;const module={exports:{}};
 new Function('require','module','exports',output)(name=>{
  if(name==='react')return react;
  if(name==='react/jsx-runtime')return{jsx:(type,props)=>({type,props}),jsxs:(type,props)=>({type,props})};
  if(name==='@tauri-apps/api/core')return{invoke:(...args)=>invoke(...args)};
  if(name.endsWith('/i18n/locale'))return{t:x=>x,tf:(x,...args)=>x.replace(/\{(\d+)\}/g,(_,n)=>args[+n])};
  if(/\.(png|svg|ico)$/.test(name))return name;
  return load(new URL(name+'.ts',file).href);
 },module,module.exports);cache.set(file,module.exports);return module.exports;
}
const {defaultSettings}=load('../src/features/settings/settingsModel.ts');
const {AGENTS,profileFor,selectAgent,effectiveAgent,summaryConfigurationAgent,localSummaryEnabled,agentInvocationNotice}=load('../src/features/agents/agentCatalog.ts');
const {useAgentIntegration}=load('../src/features/agents/useAgentIntegration.ts');
const base=defaultSettings();base.agent.provider_id='codex';base.agent.model='legacy';base.agent.command='/custom/codex';
assert.equal(localSummaryEnabled(base.agent),true,'existing configured users stay enabled');
assert.equal(localSummaryEnabled(defaultSettings().agent),false,'new users start with online summaries');
const claude=selectAgent(base.agent,'claude-code');
assert.equal(claude.model,'');assert.equal(claude.command,'');assert.equal(profileFor(claude,'codex').model,'legacy');
const auto=selectAgent(claude,'');assert.equal(auto.enabled,true,'switching an existing user to automatic keeps summaries enabled');assert.equal(selectAgent(defaultSettings().agent,'codex').enabled,false,'choosing a host does not turn on summaries');assert.equal(auto.model,'');assert.equal(profileFor(auto,'codex').command,'/custom/codex');
const found=[{id:'codex',available:true},{id:'claude-code',available:true}];
assert.equal(effectiveAgent(auto,found),'codex');
assert.equal(summaryConfigurationAgent({...auto,provider_id:'opencode'},found),'opencode','a missing selected CLI opens its own path editor');
assert.equal(summaryConfigurationAgent({...auto,provider_id:'claude-code'},[]),'claude-code','the configured profile remains editable with no detected CLIs');
assert.equal(summaryConfigurationAgent({...auto,provider_id:'workbuddy'},found),'codex','a legacy Skills-only choice cannot become the summary editor');
assert.equal(summaryConfigurationAgent(auto,[]),'codex');
assert.equal(effectiveAgent({...auto,provider_id:'missing'},found),null);
for(const id of ['workbuddy','deepseek-harness']) {
 assert.equal(effectiveAgent({...auto,provider_id:id},[{id,available:true}]),null,'Skills-only discovery cannot enable direct summaries');
}
assert.match(agentInvocationNotice('workbuddy'),/直接总结仅支持/);
assert.match(agentInvocationNotice('deepseek-harness'),/暂未适配/);
const {AgentSelect}=load('../src/features/agents/AgentSelect.tsx');
owner={hooks:[{current:null},{current:null},true,0],index:0,effects:[]};
const dropdown=AgentSelect({value:'',automatic:'codex',agents:AGENTS.map(agent=>({...agent,available:true})),disabled:false,onChange(){}});
const nodes=[];function visit(node){if(Array.isArray(node))node.forEach(visit);else if(node&&typeof node==='object'){nodes.push(node);visit(node.props?.children);}}
visit(dropdown);
const optionNames=nodes.filter(node=>node.props?.role==='option').map(node=>node.props.children[1].props.children[0].props.children);
assert.deepEqual(optionNames,['自动选择','Codex CLI','Claude Code CLI','Gemini CLI','OpenCode CLI','Cursor CLI','Qwen Code']);

const calls=[];let stored=structuredClone(base);stored.user_settings_revision=3;
let scanFailure=false,saveFailure=false,hold,holdSave;let notifications=0;
const deferred=()=>{let resolve;const promise=new Promise(r=>resolve=r);return{promise,resolve};};
globalThis.window={dispatchEvent(){notifications++;}};
invoke=async(command,args)=>{calls.push({command,args:structuredClone(args)});
 if(command==='app_settings_get')return structuredClone(stored);
 if(command==='agent_scan'){if(hold)await hold.promise;if(scanFailure)throw 'scan failed';return found;}
 if(command==='app_settings_save'){if(holdSave)await holdSave.promise;if(saveFailure)throw 'SETTINGS_CONFLICT: changed';assert.equal(args.value.user_settings_revision,stored.user_settings_revision);stored={...args.value,user_settings_revision:stored.user_settings_revision+1};return structuredClone(stored);}
 throw Error(command);
};
const state={hooks:[],index:0,effects:[]};
function render(){owner=state;state.index=0;state.effects=[];const result=useAgentIntegration(false);state.effects.forEach(f=>f());return result;}
const flush=async()=>{for(let i=0;i<40;i++)await Promise.resolve();};
render();await flush();let ui=render();assert.equal(ui.ready,true);assert.equal(ui.agents.length,2);
const before=calls.length;render();await flush();assert.equal(calls.length,before,'rerender does not poll or rediscover');
scanFailure=true;await ui.scan();ui=render();assert.equal(ui.agents.length,2,'failed scan retains previous discovery');assert.match(ui.scanError,/scan failed/);
scanFailure=false;hold=deferred();const first=ui.scan();const scanCalls=calls.filter(c=>c.command==='agent_scan').length;await ui.scan();assert.equal(calls.filter(c=>c.command==='agent_scan').length,scanCalls,'double scan submits once');hold.resolve();await first;hold=null;ui=render();
saveFailure=true;assert.equal(await ui.save(value=>({...value,agent:{...value.agent,enabled:false}})),false);ui=render();assert.equal(ui.settings.agent.enabled,null,'failed toggle never claims to be saved');assert.match(ui.error,/changed/);
saveFailure=false;assert.equal(await ui.save(value=>({...value,agent:{...value.agent,enabled:false}})),true);ui=render();assert.equal(ui.settings.agent.enabled,false);assert.equal(ui.settings.agent.model,'legacy');assert.equal(ui.settings.user_settings_revision,4);
holdSave=deferred();const notificationsBefore=notifications;const lateSave=ui.save(value=>({...value,agent:{...value.agent,enabled:true}}));state.hooks.forEach(h=>h?.cleanup?.());holdSave.resolve();assert.equal(await lateSave,false,'unmounted view does not claim a saved UI state');assert.equal(notifications,notificationsBefore+1,'confirmed native save still refreshes mounted chat readers after leaving the page');
console.log('Agent migration, profile isolation, explicit selection, failure retention, single-flight scanning and confirmed saves passed');

// Run the actual reader effects in isolation: slow discovery must not overwrite a
// newer saved selection, and detached readers must ignore in-flight responses.
for (const [file, setter, endMarker] of [
 ['../src/features/chat/local-reader/WeChatLocal.tsx', 'setAgentCurrent', '}, [pinnedAccountId]);'],
 ['../src/features/chat/summary/ChatSummary.tsx', 'setAgent', '}, []);'],
]) {
 const source=fs.readFileSync(new URL(file,import.meta.url),'utf8');
 const call=source.indexOf("void invoke<AgentCurrent>('agent_current')");
 const effect=source.lastIndexOf('useEffect(() => {',call);
 let body=source.slice(effect+'useEffect(() => {'.length,source.indexOf(endMarker,call));
 if(setter==='setAgentCurrent')body=body.slice(body.indexOf('let agentActive = true;'));
 const output=ts.transpileModule(`function mount(){${body}}`,{compilerOptions:{target:ts.ScriptTarget.ES2020}}).outputText;
 const pending=[],listeners=new Map(),states=[];
 const host={addEventListener:(name,fn)=>listeners.set(name,[...(listeners.get(name)||[]),fn]),removeEventListener:(name,fn)=>listeners.set(name,(listeners.get(name)||[]).filter(value=>value!==fn))};
 const cleanup=new Function('invoke',setter,'window','loadConnection',`${output};return mount();`)(()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});pending.push({resolve,reject});return promise;},value=>states.push(value),host,()=>{});
 const refresh=()=>listeners.get('agent-settings-changed').forEach(fn=>fn());
 refresh();pending[1].resolve({agent_name:'new'});await flush();pending[0].reject('old discovery failed');await flush();
 assert.deepEqual(states,[{agent_name:'new'}],`${file}: old failure cannot hide the new Agent`);
 refresh();refresh();pending[3].resolve({agent_name:'newest'});await flush();pending[2].resolve({agent_name:'stale'});await flush();
 assert.equal(states.at(-1).agent_name,'newest',`${file}: old success cannot replace the new Agent`);
 refresh();cleanup();pending[4].resolve({agent_name:'detached'});await flush();
 assert.equal(states.length,2,`${file}: unmounted reader does not accept a response`);
 assert.equal(listeners.get('agent-settings-changed').length,0);
}
console.log('Chat Agent refreshes reject out-of-order success/failure and detached responses');

// Clipboard write begins before native text preparation resolves (WebKit activation).
const {copyInstallationPrompt}=load('../src/features/agents/agentInstallationPrompt.ts');
let prompt=deferred(), clipboardStarted=false, copied='';
invoke=async command=>{assert.equal(command,'agent_access_installation_prompt');return prompt.promise;};
globalThis.ClipboardItem=class {constructor(data){this.data=data;}};
Object.defineProperty(globalThis,'navigator',{configurable:true,value:{clipboard:{async write(items){clipboardStarted=true;copied=await (await items[0].data['text/plain']).text();}}}});
const copying=copyInstallationPrompt();
assert.equal(clipboardStarted,true,'clipboard authorization starts in the click, before IPC completes');
prompt.resolve('app-scoped prompt');await copying;assert.equal(copied,'app-scoped prompt');
prompt=deferred();const nativeFailure=copyInstallationPrompt();prompt.resolve(Promise.reject(Error('listener unavailable')));
await assert.rejects(nativeFailure,/listener unavailable/);
invoke=async()=> 'another prompt';navigator.clipboard.write=async()=>{throw Error('clipboard denied');};
await assert.rejects(copyInstallationPrompt(),/clipboard denied/);
delete globalThis.ClipboardItem;navigator.clipboard.writeText=async text=>{copied=text;};
await copyInstallationPrompt();assert.equal(copied,'another prompt','writeText fallback copies the generated prompt');
console.log('Installation prompt clipboard activation, native failure and clipboard failure passed');

const { AgentUsage } = load('../src/features/agents/AgentUsage.tsx');
for (const skillName of ['aimychats-dev', 'aimychats']) {
  owner = { hooks: [], index: 0, effects: [] };
  nodes.length = 0;
  visit(AgentUsage({ skillName }));
  const prompt = nodes.find(node => node.type === 'p' && String(node.props.children).includes(skillName));
  assert.match(prompt.props.children, /在当前运行环境执行健康检查/);
  assert.match(prompt.props.children, /最多 1 条获准会话/);
  const copy = nodes.find(node => node.type === 'button' && node.props['aria-label'] === '复制示例');
  copy.props.onClick();
  await flush();
  assert.equal(copied, prompt.props.children);
}
console.log('Connection check copies the matching Dev or production Skill prompt');
