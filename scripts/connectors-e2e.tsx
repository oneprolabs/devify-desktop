// Browser E2E boundary: production React shell, isolated in-memory native IPC.
// This route never reads credentials, changes macOS permissions or uploads chat data.
import React from 'react';
import { createRoot } from 'react-dom/client';
import PrototypeShell from '../src/app/PrototypeShell';
import { RemoteChatFallback } from '../src/features/chat/RemoteChatFallback';
import { ChatPlatformBar, type ChatPlatform } from '../src/features/chat/ChatPlatformBar';
import wechatIcon from '../src/assets/connectors/wechat.svg';
import whatsappIcon from '../src/assets/connectors/whatsapp.svg';
import telegramIcon from '../src/assets/connectors/telegram.svg';
import slackIcon from '../src/assets/connectors/slack.svg';
import discordIcon from '../src/assets/connectors/discord.svg';
import lineIcon from '../src/assets/connectors/line.svg';
import wecomIcon from '../src/assets/connectors/wecom.svg';
import { defaultSettings } from '../src/features/settings/settingsModel';
import '../src/app/styles/style.css';
import { APP_NAME, DEVELOPMENT_APP } from '../src/shared/api/buildFlavor';
const query = new URLSearchParams(location.search);
const intel = query.has('intel');
// Optional, local-only snapshot. Never import chat data into the production bundle.
// The file is excluded locally and fetched only for this explicit preview URL.
const localSnapshot = query.has('local-chat')
 ? await fetch('/scripts/.local-preview/chat.json', {cache:'no-store'}).then(response => {
   if (!response.ok) throw Error('Local chat preview snapshot is unavailable.');
   return response.json();
 }) : null;
const wx = 'e2e-wechat', wa = '10000000000@s.whatsapp.net';
const accounts = [{id:wx,source:'wechat',modified_at:100},{id:wa,source:'whatsapp',modified_at:99}];
const catalog = (account: string) => localSnapshot ? localSnapshot.sessions[account] ?? [] : Array.from({length:query.has('chat')?query.has('long')?275:125:45},(_,i)=>({username:`${account}-chat-${i}`,display:`${account===wa?'WA':'微信'}测试${i%2?'私聊':'群聊'} ${i+1}`,is_group:i%2===0,last_time:1795000000-i*300,member_count:null,preview:null,unread:0}));
const capture = {pending:false,needs_cleanup:false,stage:'idle',transaction_id:null,wechat_official:true,wechat_ad_hoc:false,platform_supported:!intel,lldb_available:true,user_can_write:true,sip_disabled:true,os_version:intel?'12.7.6':'15.7',arch:intel?'x86_64':'aarch64',recovery_error_code:null,resume_available:false};
const feedbackDraft = () => ({client_id:'00000000-0000-4000-8000-000000000001',environment:{app_version:'0.1.17',platform:'macos',arch:'aarch64',os_version:'macOS 15.7',wechat_version:'4.1.2'},related_batch_ids:[],receiver:'https://example.invalid',owner_id:1,category:'other',description:'',page:'settings',screenshots:[] as {data:string}[],diagnostics:null as any,attempted:false});
const state = {
 summaryConfig:{schema_version:1,version:'fixture-v7',default_scenario:'three_part',max_instructions_chars:4000,scenarios:[['summary','总结','只概括主要内容、结论与当前结果。'],['action_items','待办','只列行动、负责人、期限及待确认项。'],['timeline','时间线','只按时间顺序整理事情经过。'],['three_part','综合分析','总结、时间线和待办三项都有。'],['custom','自定义','描述希望从聊天记录中得到什么，自定关注点和输出结构。']].map(([id,name,description])=>({id,name,description,requires_instructions:id==='custom',sha256:'fixture-'+id,budgets:{cloud:{unit:'utf8_bytes',max_request_bytes:12000,prompt_bytes:2000,reserved_overhead_bytes:64,max_content_bytes:9936},local_cli:{unit:'utf8_bytes',max_request_bytes:12000,prompt_bytes:1800,reserved_overhead_bytes:2048,max_content_bytes:8152}}}))},summaryConfigError:'',
 runtime:{context:'e2e',revision:1,paused:query.has('backup-disabled'),running:false,pending_config:0,jobs:[] as any[],sources:accounts.map(a=>({account_id:a.id,platform:a.source,setup_complete:true,backup_enabled:true,wizard_step:null as 'sessions'|'backup'|null}))},
 settings:{...defaultSettings(),sync:{...defaultSettings().sync,enabled:!query.has('backup-disabled')},user_settings_revision:1,active_account_id:wx,devify:{...defaultSettings().devify,...(query.has('anonymous')?{base_url:query.has('no-server')?'':'https://example.invalid'}:{})},language:{...defaultSettings().language,ui:query.has('english')?'English':'中文'}},
 calls:[] as Array<{command:string;args:unknown}>, errors:[] as string[], writes:[] as unknown[],
 permission:query.get('permission')!=='denied', keys:query.has('fresh')?0:1,
 capture, failNextSave:false, saveDelay:0, opened:0, whatsappError:'', accounts,
 chatPageError:'', chatPageAccount:'', chatBadgeDelay:0,
 chatMessageOverrides:{} as Record<string,any[]>,
 summaries:[] as any[],summaryLocal:[] as any[],summaryError:'',
 agentApps:query.has('no-agents')?[]:['workbuddy'], agentScanError:'', agentScanDelay:0, agentInstallError:'', agentSkills:{} as Record<string,string[]>,
 agents: query.has('no-agents')?[]:['codex','claude-code','gemini-cli'].map(id=>({id,name:id,command:'/e2e/'+id,version:'1.0 (fixture)',available:true,note:'Synthetic tool'})),
 analysisEvents:[] as any[],analysisNotes:{} as Record<number,any[]>,analysisReports:[] as any[],questions:[] as any[],analysisLegacy:false,analysisSearch:false,
 analysisPreferences:{event_granularity:'balanced',summary_length:'standard',summary_style:'concise',conversation_types:['direct','group'],lookback_days:0,auto_analyze:true},
 analysisSaveError:'', reportSaveError:'', reportReadError:'', reportDelay:0,
 feedbackDraft:feedbackDraft(),feedbackHistory:[] as any[],feedbackSubmitError:'',feedbackSubmits:0,
 dailyPlans:query.has('daily-on')?[{id:71,title:'微信工作日报',instruction:'汇总已备份内容中的进展、待办和风险。',include_open:true,period:'day',enabled:true,time_local:'08:30',timezone:'Asia/Shanghai'}]:[] as any[],
 authStatus:{authenticated:!query.has('auth-unavailable')&&!query.has('anonymous'),unavailable:query.has('auth-unavailable'),error:query.has('auth-unavailable')?'备份正在使用登录状态，请在当前上传结束后重试。':''},
 authPaused:false, authResolvers:[] as Array<()=>void>,
 loginPaused:false, loginResolvers:[] as Array<()=>void>, loginError:'', browserError:'', externalUrls:[] as string[],
 authStartError:'', authStartPaused:false, authStartResolvers:[] as Array<()=>void>,
 browserOpenPaused:false, browserOpenResolvers:[] as Array<()=>void>, networkMode:'auto',
};
Object.assign(window,{__E2E__:state,isTauri:true});
const callbacks = new Map<number,Function>();const eventHandlers = new Map<string,number[]>();let callbackId=0;
Object.assign(window,{__TASK_EMIT__:()=>{for(const id of eventHandlers.get('task-runtime')??[]) callbacks.get(id)?.({event:'task-runtime',id,payload:structuredClone(state.runtime)});}});
Object.assign(window,{__TAURI_EVENT_PLUGIN_INTERNALS__:{unregisterListener(){}},__TAURI_INTERNALS__:{
 metadata:{currentWindow:{label:'main'},currentWebview:{label:'main'}},
 transformCallback(fn:Function){const id=++callbackId;callbacks.set(id,fn);return id;},
 unregisterCallback(id:number){callbacks.delete(id);},convertFileSrc:(path:string)=>path,
 invoke:async(command:string,args:any={})=>{
  const loggedArgs = command === 'wechat_import_keys' ? { accountId: args.accountId } : args;
  state.calls.push({command,args:structuredClone(loggedArgs)});
  const clone=<T,>(x:T):T=>structuredClone(x);
  switch(command){
   case 'plugin:resources|close':return null;
   case 'app_settings_get':return clone({...state.settings,sync:{...state.settings.sync,enabled:state.settings.sync.enabled&&!state.runtime.paused}});
   case 'network_connection_get':return state.networkMode;
   case 'network_connection_set':state.networkMode=args.mode;return state.networkMode;
   case 'devify_spaces':return [{id:'e2e-space',name:'个人空间',kind:'personal',role:'owner'}];
   case 'devify_space_ensure':return 'e2e-space';
   case 'feedback_draft_get':return clone(state.feedbackDraft);
   case 'feedback_draft_save':state.feedbackDraft=clone(args.draft);return null;
   case 'feedback_draft_discard':state.feedbackDraft=feedbackDraft();return null;
   case 'feedback_list':return {results:clone(state.feedbackHistory),next:null};
   case 'feedback_diagnostics':return {schema_version:2,generated_at:new Date().toISOString(),lines:['2026-11-20 INFO settings.load complete']};
   case 'feedback_submit':{
    state.feedbackSubmits++;if(state.feedbackSubmitError)throw state.feedbackSubmitError;
    const report={id:'00000000-0000-4000-8000-000000000002',category:args.draft.category,description:args.draft.description,status:'received',reply:'',fix_version:'',created_at:new Date().toISOString()};
    state.feedbackHistory.unshift(report);state.feedbackDraft=feedbackDraft();return clone(report);
   }
   case 'devify_analysis_preferences_get':return clone(state.analysisPreferences);
   case 'devify_analysis_preferences_save':{
    if(state.analysisSaveError)throw state.analysisSaveError;
    if(args.preferences.auto_analyze!==true)throw Error('Disabling automatic analysis is not supported.');
    state.analysisPreferences=clone(args.preferences);return clone(state.analysisPreferences);
   }
   case 'devify_topics':if(state.reportReadError)throw state.reportReadError;return clone(state.dailyPlans);
   case 'devify_analysis_list':{
    if(args.kind!=='events')return {list:clone(state.analysisReports),pagination:{total:state.analysisReports.length,page:1}};
    const counts={my_todo:0,related:0,other:0};
    for(const row of state.analysisEvents)counts[(row.viewer_relevance??'other') as keyof typeof counts]++;
    const list=state.analysisEvents.filter(row=>state.analysisLegacy||!args.viewerRelevance||(row.viewer_relevance??'other')===args.viewerRelevance)
     .filter(row=>!state.analysisSearch||!args.searchQuery||`${row.title} ${row.content??''}`.toLowerCase().includes(args.searchQuery.trim().toLowerCase()))
     .map(row=>state.analysisLegacy?Object.fromEntries(Object.entries(row).filter(([key])=>!key.startsWith('viewer_'))):row);
    if(!args.pageSize)return {list:clone(list),pagination:{total:list.length,page:1,next:null}};
    const page=args.page??1,size=args.pageSize??100,start=(page-1)*size;
    return {list:clone(list.slice(start,start+size)),search_supported:!state.analysisLegacy&&state.analysisSearch,pagination:{total:list.length,page,page_size:size,next:start+size<list.length?'next':null},
     ...(!state.analysisLegacy&&(args.includeRelevanceCounts||args.viewerRelevance)?{relevance_counts:counts}:{})};
   }
   case 'devify_event_update':{
    const row=state.analysisEvents.find(event=>event.id===args.eventId);if(!row)throw Error('Event not found');
    for(const key of ['title','content','status'])if(args[key]!=null)row[key]=args[key];
    if(args.status==='done'&&row.viewer_relevance==='my_todo')row.viewer_relevance='related';return clone(row);
   }
   case 'devify_analysis_detail':return clone((args.kind==='events'?state.analysisEvents:state.analysisReports).find(row=>row.id===args.itemId));
   case 'devify_event_notes':return clone(state.analysisNotes[args.eventId]??[]);
   case 'devify_event_relations':return [];
   case 'devify_questions':return clone(state.questions);
   case 'devify_question_detail':return clone(state.questions.find(row=>row.id===args.questionId));
   case 'devify_question_ask':{const row={id:state.questions.length+1,question:args.question,status:'ready',answer:'## 回答\n这是一份样式演示回答。请以真实验证结果为准。',created_at:new Date().toISOString(),citations:[]};state.questions.push(row);return clone(row);}

   case 'devify_topic_detail':return clone({...state.dailyPlans.find(plan=>plan.id===args.topicId),include_open:true,matched_events:[],execution_history:[]});
   case 'devify_topic_create':{
    if(state.reportDelay)await new Promise(resolve=>setTimeout(resolve,state.reportDelay));
    if(state.reportSaveError)throw state.reportSaveError;
    const plan={id:101+state.dailyPlans.length,title:args.title,instruction:args.instruction,include_open:true,period:args.period,enabled:true,time_local:args.timeLocal,timezone:args.timezone};
    state.dailyPlans.push(plan);return clone(plan);
   }
   case 'devify_daily_plan_update':
   case 'devify_report_plan_update':{
    if(state.reportDelay)await new Promise(resolve=>setTimeout(resolve,state.reportDelay));
    if(state.reportSaveError)throw state.reportSaveError;
    const plan=state.dailyPlans.find(plan=>plan.id===args.topicId);if(!plan)throw Error('Daily plan not found.');
    if(args.period&&plan.period!==args.period)throw Error('Report period mismatch.');
    if(args.enabled!==null)plan.enabled=args.enabled;if(args.timeLocal!==null)plan.time_local=args.timeLocal;return clone(plan);
   }
   case 'connector_setup_state':{
     const source=state.runtime.sources.find(source=>source.account_id===args.accountId);
     if(source){source.setup_complete=args.completed;source.backup_enabled=!args.skipBackup;source.wizard_step=args.completed?null:args.wizardStep??'sessions';}
     state.runtime.revision++;
     (window as any).__TASK_EMIT__();
     return clone(state.settings);
   }
   case 'task_runtime_status':return clone({...state.runtime,paused:state.runtime.paused||!state.settings.sync.enabled});
   case 'task_runtime_pause':{
    state.runtime.paused=args.paused;state.settings.sync.enabled=!args.paused;
    state.settings.user_settings_revision++;state.runtime.revision++;(window as any).__TASK_EMIT__();return null;
   }
   case 'task_runtime_retry':return null;
   case 'app_settings_save':{
    if(state.saveDelay) await new Promise(resolve=>setTimeout(resolve,state.saveDelay));
    if(state.failNextSave){state.failNextSave=false;throw 'SETTINGS_CONFLICT: E2E concurrent edit';}
    if(args.value.user_settings_revision!==state.settings.user_settings_revision)throw 'SETTINGS_CONFLICT: stale E2E revision';
    state.settings=clone({...args.value,user_settings_revision:state.settings.user_settings_revision+1});state.writes.push(clone(state.settings));
    state.runtime.paused=!state.settings.sync.enabled;state.runtime.revision++;(window as any).__TASK_EMIT__();
    for(const id of eventHandlers.get('development-service-changed')??[]) callbacks.get(id)?.({event:'development-service-changed',id,payload:state.settings.devify.base_url});
    return clone(state.settings);
   }
   case 'devify_auth_status':{
    if(state.authPaused) await new Promise<void>(resolve=>state.authResolvers.push(resolve));
    return clone(state.authStatus);
   }
   case 'devify_auth_cancel':return null;
   case 'devify_login_password':{
    if(state.loginPaused) await new Promise<void>(resolve=>state.loginResolvers.push(resolve));
    if(state.loginError) throw state.loginError;
    state.authStatus={authenticated:true,unavailable:false,error:''};return null;
   }
   case 'devify_auth_start':{
    if(state.authStartPaused) await new Promise<void>(resolve=>state.authStartResolvers.push(resolve));
    if(state.authStartError) throw state.authStartError;
    return 'https://example.invalid/auth/desktop?state=e2e';
   }
   case 'devify_auth_open':{
    if(state.browserOpenPaused) await new Promise<void>(resolve=>state.browserOpenResolvers.push(resolve));
    if(state.browserError) throw state.browserError;
    state.externalUrls.push(args.url);return null;
   }
   case 'chat_remote_lookup':return {conversation:{id:10,title:'Synthetic backup'}};
   case 'devify_conversation_messages':return {list:Array.from({length:100},(_,i)=>({id:i+1,sender_name:'Synthetic sender',sender_key:'test',preview:`Synthetic preview ${i+1}`,occurred_at:'2026-11-19T10:00:00Z'})),pagination:{page:2,total:200,page_size:100}};
   case 'devify_message_detail':return {id:args.messageId,parts:[{id:1,kind:'text',text:'Synthetic full message'},{id:2,kind:'image',upload_item_id:50}]};
   case 'chat_remote_image':return 'data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==';
   case 'devify_current_user':return {id:1,display_name:localSnapshot?'本地预览':'E2E Tester'};
   case 'devify_device_register':return {id:'e2e-device'};
   case 'devify_upload_identity':return {user_id:1,device_id:'e2e-device'};
   case 'entitlements_get':case 'entitlements_refresh':return {internal:query.has('internal'),features:['cloud.chat_summary'],enforced:false,grace_expired:false};
   case 'cloud_summary_list':{
    if(state.summaryError)throw state.summaryError;
    const rows=state.summaries.filter(row=>row.source_namespace===args.accountId&&row.source_conversation_id===args.username);
    const local=state.summaryLocal.filter(row=>row.source_namespace===args.accountId&&row.source_conversation_id===args.username);
    return {list:rows.filter(row=>row.engine==='cloud'),local:clone([...rows,...local]),pagination:{total:rows.filter(row=>row.engine==='cloud').length,page:1,next:null}};
   }
   case 'summary_config_get':{
    if(state.summaryConfigError)throw state.summaryConfigError;
    return {config:clone(state.summaryConfig),language:state.settings.language.summary==='跟随界面'?state.settings.language.ui:state.settings.language.summary,ui_language:state.settings.language.ui,instructions:state.settings.agent.instructions};
   }
   case 'summary_config_refresh':if(state.summaryConfigError)throw state.summaryConfigError;return null;
   case 'summary_task_enqueue':{
    if(state.summaryError)throw state.summaryError;
    const old=state.summaries.find(row=>row.request_key===args.body.request_key);if(old)return clone(old);
    const row={...clone(args.body),id:args.engine==='local'?`task:${args.body.request_key}`:1000+state.summaries.length,task_id:args.body.request_key,source_namespace:args.accountId,source_conversation_id:args.username,engine:args.engine,caller:args.engine==='local'?'Codex CLI':'云端模型',status:'queued',message_count:args.body.messages.length,created_at:new Date().toISOString(),sessions:[{username:args.username,display:args.body.title,messages:args.body.messages.length}],body:'',error:''};
    state.summaries.unshift(row);
    const job={id:row.task_id,kind:'summary',account_id:args.accountId,title:row.title,caller:row.caller,engine:row.engine,initiator_id:1,status:'queued',message:'等待生成总结。',updated_at:row.created_at};
    state.runtime.jobs.unshift(job);state.runtime.revision++;(window as any).__TASK_EMIT__();
    setTimeout(()=>{row.status='ready';row.body='## 概览\n这是一份用于界面调试的演示总结。\n\n## 关键进展\n- 所选消息已按时间整理。\n\n## 待办与跟进\n- 可以通过任务页面查看调用方式与状态。';job.status='completed';job.message='总结已完成。';state.runtime.revision++;(window as any).__TASK_EMIT__();},1500);
    return clone(row);
   }
   case 'cloud_summary_action':case 'summary_task_delete':{
    state.summaries=state.summaries.filter(row=>row.id!==args.summaryId&&row.task_id!==args.taskId);return null;
   }
   case 'summary_task_retry':return null;
   case 'agent_scan': {
    if(state.agentScanDelay)await new Promise(resolve=>setTimeout(resolve,state.agentScanDelay));
    if(state.agentScanError)throw state.agentScanError;return clone(state.agents);
   }
   case 'agent_access_installation_prompt':if(state.agentInstallError)throw state.agentInstallError;return 'Synthetic AIMyChats installation prompt: identify your own Skills directory; preserve existing content; check health.';
   case 'agent_access_status':return {running:true,reason:null,wxq:{path:'/e2e/wxq'},skills:[{name:'aimychats'}],targets:['codex','claude-code','gemini-cli','workbuddy','deepseek-harness','opencode','cursor-agent','qwen-code'].map(id=>({id,name:id,available:state.agents.some(agent=>agent.id===id&&agent.available)||state.agentApps.includes(id),command:state.agents.some(agent=>agent.id===id&&agent.available)?'/e2e/'+id:null,desktop_app:state.agentApps.includes(id)?'/e2e/'+id+'.app':null,skills_dir:state.settings.agent.profiles?.[id]?.skills_dir||'/e2e/'+id+'/skills',installed:state.agentSkills[id]??[]}))};
   case 'agent_access_install': {
    if(state.agentInstallError)throw state.agentInstallError;
    await new Promise(resolve=>setTimeout(resolve,150));state.agentSkills[args.tool]=['aimychats'];return {tool:args.tool,installed:clone(state.agentSkills[args.tool]),skills_dir:'/e2e/'+args.tool+'/skills'};
   }
   case 'wechat_voice_status':return {settings:{engine:'',model:'',language:''},engines:[],selected:null};
   case 'wechat_voice_settings_save':return {settings:args,engines:[],selected:null};
   case 'agent_models':return [];
   case 'agent_test':return {output:'OK',elapsed_ms:30,command:'Codex CLI'};
   case 'plugin:event|listen':eventHandlers.set(args.event,[...(eventHandlers.get(args.event)??[]),args.handler]);return ++callbackId;
   case 'plugin:event|unlisten':case 'diagnostic_record':return null;
   case 'plugin:updater|check':return query.has('update')?{rid:901,version:'0.1.20',currentVersion:'0.1.10',body:'Synthetic update for preview'}:null;
   case 'app_info':return {platform:'macos',arch:capture.arch,os_version:capture.os_version,version:'0.1.10',name:APP_NAME,development:DEVELOPMENT_APP};
   case 'connector_detect':return {state:query.has('untrusted')&&args.kind==='wechat'?'untrusted':'installed',app_path:args.kind==='whatsapp'?'/Applications/WhatsApp.app':'/Applications/WeChat.app',version:'4.1.2',message:''};
   case 'chat_accounts':return clone(state.accounts);
   case 'chat_account_activate':state.settings.active_account_id=args.accountId;return null;
   case 'whatsapp_status':return {installed:true,version:'2.26',account:wa,name:'E2E WA',sessions:45,messages:90,error:state.whatsappError||null};
   case 'wechat_account_profile':return null;
   case 'wechat_session_catalog':return catalog(args.accountId);
   case 'chat_sessions':{
    if(args.offset&&state.chatPageError&&args.accountId===state.chatPageAccount)throw state.chatPageError;
    const all=state.keys||args.accountId===wa?catalog(args.accountId).filter(s=>(state.settings.chat.allowed_usernames[args.accountId]??[]).includes(s.username)):[];
    if(args.focusUsername)return all.filter(s=>s.username===args.focusUsername);
    if(args.focusUsernames)return all.filter(s=>args.focusUsernames.includes(s.username));
    return all.slice(args.offset??0,(args.offset??0)+(args.limit??50));
   }
   case 'wechat_catalog_dates':return [['2026-11-18',2],['2026-11-17',1]];
   case 'wechat_scan':return {roots:['/e2e'],skipped:[],accounts:(localSnapshot?state.accounts:[{id:wx}]).map(account=>({id:account.id,dir:'/e2e',db_count:1,encrypted_count:1,total_bytes:1024,modified_at:100,dbs:[{rel:'test.db',path:'/e2e/test.db',size:1024,encrypted:true,salt:'test',has_wal:false}]}))};
   case 'wechat_key_status':return {path:'/e2e/keys',stored:state.keys,keys:state.keys?(localSnapshot?state.accounts:[{id:wx}]).map(account=>({account:account.id,rel:'test.db',salt:'test',source:'e2e',verified_at:'2026-11-18',verified_secs:1795000000})):[]};
   case 'wechat_import_keys': {
    const input = args.text.trim();
    let key = input;
    if (input.startsWith('{')) {
     try { key = JSON.parse(input).passphrase; }
     catch { throw '无法识别密钥格式。'; }
    }
    if (typeof key !== 'string' || !/^[0-9a-f]{64}$/i.test(key)) throw '无法识别密钥格式。';
    if (args.accountId !== wx || key !== 'ab'.repeat(32)) {
     return {matched:0,already:0,unmatched:[],accounts:[],message:'导入内容未通过当前微信账号的校验。'};
    }
    state.keys = 1;
    return {matched:1,already:0,unmatched:[],accounts:[{account:wx,matched:1,already:0,total:1,selected:true}],message:'密钥已校验。'};
   }
   case 'wechat_key_source_status':return {path:'/e2e/keys',exists:false,modified_at:null,age_seconds:null,secure:true,extractor_installed:true};
   case 'wechat_env':return {app_data_dir:'/e2e',cache_root:'/e2e',keys_path:'/e2e/keys',scanner_path:null};
   case 'wechat_platform_capabilities':return {platform:'macos',key_acquisition:intel?'import_only':'macos_capture_or_import',supports_macos_capture:!intel,supports_key_import:true,supports_memory_scan:false};
   case 'perm_status':return {granted:state.permission,app_path:'/Applications/AIMyChats.app',bundled:true,installed:true,location:'applications',detail:''};
   case 'perm_open_settings':case 'perm_reveal_app':return null;
   case 'wechat_capture_status':return clone(state.capture);
   case 'wechat_capture_prepare':Object.assign(state.capture,{pending:true,needs_cleanup:true,stage:'awaiting_manual_launch',transaction_id:'e2e-transaction',wechat_official:false,wechat_ad_hoc:true});return {transaction_id:'e2e-transaction',version:'4.1.2',build:'test',backup_created:true,entitlement_mode:'preserved',dropped_entitlements:[]};
   case 'wechat_capture_open':state.opened++;return {launched:true};
   case 'wechat_capture_confirm_launch':Object.assign(state.capture,{stage:'launched'});return {ready_for_preflight:true,transaction_id:'e2e-transaction',pid:1234};
   case 'wechat_capture_preflight':Object.assign(state.capture,{stage:'preflight_passed'});return {transaction_id:'e2e-transaction',pid:1234,pbkdf_locations:1};
   case 'wechat_capture_start':state.keys=1;Object.assign(state.capture,{pending:false,needs_cleanup:false,stage:'idle',wechat_official:true,wechat_ad_hoc:false});return {matched:1,already:0,accounts:[{account:wx,matched:1,total:1}],message:'E2E connection complete',restored:true,validated_roles:['message']};
   case 'wechat_capture_restore':Object.assign(state.capture,{pending:false,needs_cleanup:false,stage:'idle',wechat_official:true,wechat_ad_hoc:false});return {restored:true,verified:true};
   case 'wechat_decrypt':case 'wechat_sync':return ['session/session.db','message/message_0.db'].map(rel=>({account:wx,rel,status:'ok',pages:1,error:null}));
   case 'wechat_messages':{
    if(state.chatMessageOverrides[args.accountId]) return clone(state.chatMessageOverrides[args.accountId])
      .filter((message:any)=>!args.before||message.time<args.before||(message.time===args.before&&message.local_id<args.beforeId))
      .slice(0,args.limit??50);
    if(localSnapshot) return (localSnapshot.messages[`${args.accountId}:${args.username}`]??[])
      .filter((message:any)=>!args.before||message.time<args.before||(message.time===args.before&&message.local_id<args.beforeId))
      .slice(0,args.limit??50);
    return query.has('chat')?[{local_id:1,source_shard:args.accountId===wa?'whatsapp':'synthetic',time:1795000000,sender:'tester',sender_name:'测试联系人',kind:args.accountId===wa?0:1,text:`${args.accountId===wa?'WhatsApp':'微信'} · ${args.username} 的测试消息`,is_self:false}]:[];
   }
   case 'wechat_message_media':return {items:[],counts:{images:0,videos:0,files:0,voices:0,encrypted:0,missing:0}};
   case 'wechat_upload_state':{
    if(localSnapshot)return args.usernames.map((username:string)=>({username,state:'unknown',new_messages:0}));
    if(state.chatBadgeDelay)await new Promise(resolve=>setTimeout(resolve,state.chatBadgeDelay));
    return query.has('chat')?args.usernames.map((username:string,index:number)=>({username,state:index===0?'local':index===1?'pending':'synced',new_messages:index===1?3:0})):[];
   }
   case 'wechat_avatars':case 'devify_my_spaces':case 'summary_history_list':return [];
   case 'agent_current': {
    const id=state.settings.agent.provider_id||state.agents[0]?.id||'';
    const configured=state.settings.agent.enabled??Boolean(state.settings.agent.provider_id);
    return {configured,agent_id:id,agent_name:state.agents.find(agent=>agent.id===id)?.name||id,model:state.settings.agent.profiles?.[id]?.model??(state.settings.agent.provider_id===id?state.settings.agent.model:''),command:configured?'/e2e/'+id:null,available:configured&&state.agents.some(agent=>agent.id===id)};
   }
   default:state.errors.push(command);throw Error(`E2E unsupported IPC: ${command}`);
  }
 }
}});
window.addEventListener('error',event=>state.errors.push(event.message));
window.addEventListener('unhandledrejection',event=>state.errors.push(String(event.reason)));
if(query.has('fallback')) state.settings.devify.active_space_id='e2e-space';
if(query.has('analysis')) state.settings.devify.active_space_id='e2e-space';
if(query.has('chat')) {
 state.settings.devify.active_space_id='e2e-space';
 state.settings.chat.allowed_usernames={[wx]:catalog(wx).map(s=>s.username),[wa]:catalog(wa).map(s=>s.username)};
}
if(localSnapshot) {
 state.accounts=localSnapshot.accounts;
 state.runtime.sources=localSnapshot.accounts.map((account:any)=>({account_id:account.id,platform:account.source,setup_complete:true,backup_enabled:true,wizard_step:null}));
 state.settings.active_account_id=localSnapshot.accounts[0]?.id;
 state.settings.chat.allowed_usernames=Object.fromEntries(localSnapshot.accounts.map((account:any)=>[account.id,catalog(account.id).map((session:any)=>session.username)]));
 document.title='AIMyChats · 本地聊天快照预览';
 const previewStyles=document.createElement('style');
 previewStyles.textContent='.chat-workbench-sidebar-head p::after{content:"本地快照 · 仅文本 · 不执行备份";display:block;margin-top:6px;color:#74746e;font-size:11px}.chat-workbench-embedded .wxv-chat-foot>span:first-child{font-size:0}.chat-workbench-embedded .wxv-chat-foot>span:first-child::after{content:"样式预览 · 每个会话最多 40 条文本";font-size:11px}';
 document.head.append(previewStyles);
}
function PlatformBarPreview() {
 const [selected,setSelected]=React.useState('wechat');
 const demos:ChatPlatform[]=[['wechat','微信',wechatIcon],['whatsapp','WhatsApp',whatsappIcon],['telegram','Telegram',telegramIcon],['slack','Slack',slackIcon],['discord','Discord',discordIcon],['line','LINE',lineIcon],['wecom','企业微信',wecomIcon]].map(([id,label,icon])=>({id,label,icon,updated:id!==selected&&['whatsapp','discord','wecom'].includes(id)}));
 return <div style={{width:310,margin:24,padding:'20px 0',background:'#fafafa'}}><ChatPlatformBar platforms={demos} selected={selected} onSelect={setSelected}/></div>;
}
createRoot(document.getElementById('root')!).render(query.has('platform-bar') ? <PlatformBarPreview/> : query.has('fallback') ? <div className="wxv-messages" style={{height:500,overflow:'auto'}}><RemoteChatFallback accountId={wx} username="missing-local" /></div> : <PrototypeShell />);
