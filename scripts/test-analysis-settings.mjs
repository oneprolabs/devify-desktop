import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';

let current, invoke;
const react = {
  useState(initial) { const owner=current,index=owner.index++; if(!(index in owner.hooks))owner.hooks[index]=typeof initial==='function'?initial():initial;return[owner.hooks[index],value=>{owner.hooks[index]=typeof value==='function'?value(owner.hooks[index]):value;}]; },
  useRef(initial) { const index=current.index++;return current.hooks[index]??={current:initial}; },
  useEffect(effect,deps) { const owner=current,index=owner.index++,previous=owner.hooks[index];if(!previous||deps.some((value,position)=>value!==previous.deps[position]))owner.effects.push(()=>{previous?.cleanup?.();owner.hooks[index]={deps,cleanup:effect()};}); },
  useCallback(callback) { return callback; },
};
const jsx={jsx:(type,props)=>({type,props}),jsxs:(type,props)=>({type,props})};
const translate={t:value=>value,tf:(pattern,...args)=>pattern.replace(/\{(\d+)\}/g,(_,index)=>String(args[Number(index)]))};
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
const flush=async()=>{for(let count=0;count<30;count++)await Promise.resolve();};
function WizardActionButton() {}
function DailyTimePicker() {}
function AgentSettings() {}
const cache=new Map();
function load(file) {
  file = new URL(file, import.meta.url).href;
  if(cache.has(file))return cache.get(file);
  const output=ts.transpileModule(fs.readFileSync(new URL(file,import.meta.url),'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2020}}).outputText;
  const module={exports:{}};
  new Function('require','module','exports',output)(name=>{
    if(name==='react')return react;
    if(name==='react/jsx-runtime')return jsx;
    if(name==='@tauri-apps/api/core')return{invoke:(...args)=>invoke(...args)};
    if(['../../shared/i18n/locale', '../i18n/locale'].includes(name))return translate;
    if(name==='../settings/Settings')return{defaultSettings:()=>({devify:{active_space_id:null}}),mergeSettings:value=>value};
    if(['../../shared/scheduling/dailyPlan','./reportPlans','./ReportSchedules'].includes(name))return load(`${new URL(name, file).href}.${name.endsWith('ReportSchedules')?'tsx':'ts'}`);
    if(name==='./analysisSettingsText')return{analysisSettingsError:value=>value instanceof Error?value.message:String(value)};
    if(name==='../../shared/ui/WizardActionButton')return{WizardActionButton};
    if(name==='../../shared/ui/DailyTimePicker')return{DailyTimePicker};
    if(name==='../agents/AgentSettings')return{AgentSettings};
    if(name.endsWith('.css'))return{};
    throw Error(name);
  },module,module.exports);
  cache.set(file,module.exports);return module.exports;
}
const {AnalysisSettings}=load('../src/features/analysis/AnalysisSettings.tsx');
const {ReportSchedules,ReportScheduleRow}=load('../src/features/analysis/ReportSchedules.tsx');
const {REPORT_PLANS,reportPlanSelection}=load('../src/features/analysis/reportPlans.ts');
const nodes=tree=>Array.isArray(tree)?tree.flatMap(nodes):tree&&typeof tree==='object'?[tree,...nodes(tree.props?.children)]:[];
const text=tree=>Array.isArray(tree)?tree.map(text).join(''):tree&&typeof tree==='object'?text(tree.props?.children):String(tree??'');
function find(tree,test){const found=nodes(tree).find(test);assert.ok(found,'expected control missing');return found;}
function view(component,props={}) {return {hooks:[],component,props};}
function render(owner){current=owner;owner.index=0;owner.effects=[];const tree=owner.component(owner.props);owner.effects.forEach(effect=>effect());return tree;}
const clean=owner=>owner.hooks.forEach(hook=>hook?.cleanup?.());
const initial={event_granularity:'coarse',summary_length:'detailed',summary_style:'actions',conversation_types:['group'],lookback_days:90,auto_analyze:true};
let plans=[{id:77,title:'微信工作日报',period:'day',enabled:false,time_local:'17:45',timezone:'UTC'}];
const prefRead=deferred(),topicRead=deferred(),calls=[],mutations=[];
invoke=(command,args)=>{
  calls.push({command,args:structuredClone(args)});
  if(command==='app_settings_get')return Promise.resolve({devify:{active_space_id:'space'}});
  if(command==='devify_analysis_preferences_get')return prefRead.promise;
  if(command==='devify_topics')return topicRead.promise;
  if(command==='devify_analysis_preferences_save'){const pending=deferred();mutations.push({args:structuredClone(args),...pending});return pending.promise;}
  throw Error(command);
};
const navigation=[];
const preferences=view(AnalysisSettings,{onOpenReports(){navigation.push('reports');},onOpenTopics(){navigation.push('topics');},onOpenChats(){navigation.push('chat');}});
render(preferences);await flush();let tree=render(preferences);await flush();
const reportsNode=find(tree,node=>node.type===ReportSchedules);
const reports=view(ReportSchedules,reportsNode.props);render(reports);await flush();
assert.ok(calls.some(call=>call.command==='devify_topics'),'report reads do not wait for analysis preferences');
assert.ok(calls.some(call=>call.command==='devify_analysis_preferences_get'));
prefRead.resolve(structuredClone(initial));topicRead.resolve(structuredClone(plans));await flush();tree=render(preferences);
assert.equal(find(tree,node=>node.props?.['aria-label']==='调整事件与笔记设置').props['aria-expanded'],false);
assert.equal(nodes(tree).filter(node=>node.props?.role==='switch').length,0,'auto-analysis has no unsupported off switch or fake runtime indicator');
assert.ok(!text(tree).includes('自动运行'));
find(tree,node=>node.props?.['aria-label']==='调整事件与笔记设置').props.onClick();tree=render(preferences);
assert.ok(!text(tree).includes('时间范围'),'scheduled analysis does not apply lookback_days');
assert.ok(text(tree).includes('聊天总结、报告和主题使用各自的生成规则。'));
assert.ok(text(tree).includes('保存事件与笔记设置'));
assert.ok(text(tree).includes('用于之后新建的事件分析任务'));
assert.ok(!text(tree).includes('摘要风格')&&!text(tree).includes('摘要详略'));
assert.ok(!text(tree).includes('本地总结设置'),'ordinary accounts have no local tool settings');
find(tree,node=>node.type==='button'&&text(node)==='前往聊天').props.onClick();
find(tree,node=>node.type==='button'&&text(node)==='管理主题').props.onClick();
assert.deepEqual(navigation,['chat','topics'],'each capability opens its own working page');
assert.equal(mutations.length,0,'navigation never saves analysis preferences or creates report plans');
preferences.props.canUseLocalSummary=true;
tree=render(preferences);
assert.ok(!nodes(tree).some(node=>node.type===AgentSettings),'local tool detection stays lazy');
find(tree,node=>node.type==='button'&&text(node).includes('本地总结设置')).props.onClick();
tree=render(preferences);
const summarySection=find(tree,node=>node.props?.['aria-label']==='聊天总结设置');
assert.equal(find(summarySection,node=>node.type===AgentSettings).props.summaryOnly,true,'existing local settings render inside the chat summary group, without voice settings');
assert.ok(!nodes(find(tree,node=>node.props?.['aria-label']==='事件与笔记设置')).some(node=>node.type===AgentSettings),'event preferences never contain local summary options');
const style=find(tree,node=>node.type==='select');assert.equal(style.props.value,'actions');
style.props.onChange({target:{value:'concise'}});tree=render(preferences);
let pending=find(tree,node=>node.type===WizardActionButton).props.onAction();find(tree,node=>node.type===WizardActionButton).props.onAction();await flush();
assert.equal(mutations.length,1);assert.deepEqual(mutations[0].args.preferences,{...initial,summary_style:'concise'},'hidden historical range and other values are preserved');
mutations[0].reject('temporary preference error');await pending;tree=render(preferences);
assert.ok(text(tree).includes('temporary preference error'));assert.equal(find(tree,node=>node.type==='select').props.value,'concise');
pending=find(tree,node=>node.type===WizardActionButton).props.onAction();mutations[1].resolve({...initial,summary_style:'concise'});await pending;tree=render(preferences);
assert.ok(text(tree).includes('已保存'));assert.equal(find(tree,node=>node.type===WizardActionButton).props.disabled,true);
assert.equal(nodes(render(reports)).filter(node=>node.type===ReportScheduleRow).length,3);
clean(preferences);clean(reports);

// Each period preserves existing IDs, times, timezones and unrelated configuration.
const reportCalls=[],writes=[];let readError='';
invoke=(command,args)=>{
  reportCalls.push({command,args:structuredClone(args)});
  if(command==='devify_topics')return readError?Promise.reject(readError):Promise.resolve(structuredClone(plans));
  if(['devify_report_plan_update','devify_topic_create'].includes(command)){const pending=deferred();writes.push({command,args:structuredClone(args),...pending});return pending.promise;}
  throw Error(command);
};
const day=view(ReportScheduleRow,{spaceId:'space',plans:structuredClone(plans),definition:REPORT_PLANS[0],onOpenTopics(){}});
tree=render(day);let control=find(tree,node=>node.props?.role==='switch');control.props.onClick();control.props.onClick();await flush();
assert.equal(writes.length,1);assert.deepEqual(writes[0].args,{spaceId:'space',topicId:77,period:'day',enabled:true,timeLocal:null});
assert.equal(find(render(day),node=>node.props?.role==='switch').props.disabled,true);
plans[0].enabled=true;writes[0].resolve({});await flush();tree=render(day);
assert.equal(find(tree,node=>node.type===DailyTimePicker).props.value,'17:45');assert.ok(text(tree).includes('UTC'));
find(tree,node=>node.type===DailyTimePicker).props.onChange('18:15');tree=render(day);find(tree,node=>node.type===DailyTimePicker).props.onConfirm();await flush();
assert.equal(writes[1].args.timeLocal,'18:15');assert.equal(writes[1].args.enabled,null);
writes[1].reject('temporary report failure');await flush();tree=render(day);
assert.equal(find(tree,node=>node.props?.role==='switch').props['aria-checked'],true,'failure retains confirmed enabled state');
assert.equal(find(tree,node=>node.type===DailyTimePicker).props.value,'18:15','failure retains the draft until re-read');
assert.ok(text(tree).includes('temporary report failure'));find(tree,node=>node.type==='button'&&text(node)==='重新读取').props.onClick();await flush();tree=render(day);
assert.equal(find(tree,node=>node.type===DailyTimePicker).props.value,'17:45');assert.equal(find(tree,node=>node.props?.role==='switch').props.disabled,false);
clean(day);

for (const definition of REPORT_PLANS.slice(1)) {
  plans=[];const start=writes.length;
  const row=view(ReportScheduleRow,{spaceId:'space',plans:[],definition,onOpenTopics(){}});
  tree=render(row);control=find(tree,node=>node.props?.role==='switch');control.props.onClick();control.props.onClick();await flush();
  assert.equal(writes.length,start+1,'duplicate clicks cannot create repeated plans');
  assert.equal(writes[start].command,'devify_topic_create');assert.equal(writes[start].args.period,definition.period);assert.equal(writes[start].args.title,definition.title);
  const created={id:100+start,title:definition.title,period:definition.period,enabled:true,time_local:'08:30',timezone:'Asia/Shanghai'};
  plans=[created];writes[start].reject('response lost');await flush();tree=render(row);
  assert.equal(find(tree,node=>node.props?.role==='switch').props.disabled,true);
  find(tree,node=>node.type==='button'&&text(node)==='重新读取').props.onClick();await flush();tree=render(row);
  assert.equal(find(tree,node=>node.props?.role==='switch').props['aria-checked'],true);
  find(tree,node=>node.props?.role==='switch').props.onClick();await flush();
  assert.equal(writes[start+1].command,'devify_report_plan_update');assert.equal(writes[start+1].args.topicId,created.id);
  assert.equal(writes[start+1].args.enabled,false);assert.equal(writes[start+1].args.period,definition.period);
  plans[0].enabled=false;writes[start+1].resolve({});await flush();tree=render(row);
  assert.ok(!nodes(tree).some(node=>node.type===DailyTimePicker));clean(row);
}
// An enabled subscription with no time is not an active automatic schedule.
plans=[{id:200,title:'每月报告',period:'month',enabled:true,time_local:null,timezone:'UTC'}];
const unscheduled=view(ReportScheduleRow,{spaceId:'space',plans:structuredClone(plans),definition:REPORT_PLANS[2],onOpenTopics(){}});
tree=render(unscheduled);assert.equal(find(tree,node=>node.props?.role==='switch').props['aria-checked'],false);
find(tree,node=>node.props?.role==='switch').props.onClick();await flush();assert.equal(writes.at(-1).args.timeLocal,'08:30');
plans[0].time_local='08:30';writes.at(-1).resolve({});await flush();clean(unscheduled);

const weekly={id:300,title:'每周报告',period:'week',enabled:true,time_local:'11:15',timezone:'UTC'};
assert.equal(reportPlanSelection([{...weekly,title:'Weekly report'}],REPORT_PLANS[1]).plan.id,300);
assert.throws(()=>reportPlanSelection([weekly,{...weekly,id:301,title:'Weekly report'}],REPORT_PLANS[1]));
let opened=false;
const ambiguous=view(ReportScheduleRow,{spaceId:'space',plans:[weekly,{...weekly,id:301,title:'Weekly report'}],definition:REPORT_PLANS[1],onOpenTopics(){opened=true;}});
tree=render(ambiguous);assert.equal(find(tree,node=>node.props?.role==='switch').props.disabled,true);
find(tree,node=>node.type==='button'&&text(node)==='管理主题').props.onClick();assert.equal(opened,true);clean(ambiguous);
assert.equal(reportPlanSelection([weekly,{...weekly,id:302,title:'客户进展'}],REPORT_PLANS[1]).others,1);
assert.ok(!reportCalls.some(call=>call.command==='app_settings_save'));
console.log('Analysis preferences preserve hidden fields; day/week/month schedules isolate changes, reuse plans, recover lost responses and reject ambiguous plans');
