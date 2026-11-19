// Browser E2E boundary: production React shell, isolated in-memory native IPC.
// This route never reads credentials, changes macOS permissions or uploads chat data.
import React from 'react';
import { createRoot } from 'react-dom/client';
import PrototypeShell from '../src/PrototypeShell';
import { defaultSettings } from '../src/Settings';
import '../src/style.css';
const query = new URLSearchParams(location.search);
const wx = 'e2e-wechat', wa = '10000000000@s.whatsapp.net';
const accounts = [{id:wx,source:'wechat',modified_at:100},{id:wa,source:'whatsapp',modified_at:99}];
const catalog = (account: string) => Array.from({length:45},(_,i)=>({username:`${account}-chat-${i}`,display:`${account===wa?'WA':'微信'}测试${i%2?'私聊':'群聊'} ${i+1}`,is_group:i%2===0,last_time:1795000000-i*300,member_count:null,preview:null,unread:0}));
const capture = {pending:false,needs_cleanup:false,stage:'idle',transaction_id:null,wechat_official:true,wechat_ad_hoc:false,platform_supported:true,lldb_available:true,user_can_write:true,sip_disabled:true,os_version:'15.7',arch:'aarch64',recovery_error_code:null,resume_available:false};
const state = {
 settings:{...defaultSettings(),user_settings_revision:1,active_account_id:wx,devify:{...defaultSettings().devify,...(query.has('anonymous')?{base_url:query.has('no-server')?'':'https://example.invalid'}:{})},language:{...defaultSettings().language,ui:query.has('english')?'English':'中文'}},
 calls:[] as Array<{command:string;args:unknown}>, errors:[] as string[], writes:[] as unknown[],
 permission:query.get('permission')!=='denied', keys:query.has('fresh')?0:1,
 capture, failNextSave:false, saveDelay:0, opened:0, whatsappError:'', accounts,
 authStatus:{authenticated:!query.has('auth-unavailable')&&!query.has('anonymous'),unavailable:query.has('auth-unavailable'),error:query.has('auth-unavailable')?'备份正在使用登录状态，请在当前上传结束后重试。':''},
 authPaused:false, authResolvers:[] as Array<()=>void>,
 loginPaused:false, loginResolvers:[] as Array<()=>void>, loginError:'', browserError:'', externalUrls:[] as string[],
 authStartError:'', authStartPaused:false, authStartResolvers:[] as Array<()=>void>,
};
Object.assign(window,{__E2E__:state,isTauri:true});
const callbacks = new Map<number,Function>();let callbackId=0;
Object.assign(window,{__TAURI_EVENT_PLUGIN_INTERNALS__:{unregisterListener(){}},__TAURI_INTERNALS__:{
 metadata:{currentWindow:{label:'main'},currentWebview:{label:'main'}},
 transformCallback(fn:Function){const id=++callbackId;callbacks.set(id,fn);return id;},
 unregisterCallback(id:number){callbacks.delete(id);},convertFileSrc:(path:string)=>path,
 invoke:async(command:string,args:any={})=>{
  state.calls.push({command,args:structuredClone(args)});
  const clone=<T,>(x:T):T=>structuredClone(x);
  switch(command){
   case 'app_settings_get':return clone(state.settings);
   case 'app_settings_save':{
    if(state.saveDelay) await new Promise(resolve=>setTimeout(resolve,state.saveDelay));
    if(state.failNextSave){state.failNextSave=false;throw 'SETTINGS_CONFLICT: E2E concurrent edit';}
    if(args.value.user_settings_revision!==state.settings.user_settings_revision)throw 'SETTINGS_CONFLICT: stale E2E revision';
    state.settings=clone({...args.value,user_settings_revision:state.settings.user_settings_revision+1});state.writes.push(clone(state.settings));return clone(state.settings);
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
    if(state.browserError) throw state.browserError;
    state.externalUrls.push(args.url);return null;
   }
   case 'devify_current_user':return {id:1,display_name:'E2E Tester'};
   case 'devify_device_register':return {id:'e2e-device'};
   case 'devify_upload_identity':return {user_id:1,device_id:'e2e-device'};
   case 'entitlements_get':case 'entitlements_refresh':return null;
   case 'plugin:event|listen':return ++callbackId;
   case 'plugin:event|unlisten':case 'diagnostic_record':return null;
   case 'plugin:updater|check':return null;
   case 'app_info':return {platform:'macos',arch:'aarch64',os_version:'15.7',version:'0.1.10',name:'AIMyChats'};
   case 'connector_detect':return {state:query.has('untrusted')&&args.kind==='wechat'?'untrusted':'installed',app_path:args.kind==='whatsapp'?'/Applications/WhatsApp.app':'/Applications/WeChat.app',version:'4.1.2',message:''};
   case 'chat_accounts':return clone(state.accounts);
   case 'chat_account_activate':state.settings.active_account_id=args.accountId;return null;
   case 'whatsapp_status':return {installed:true,version:'2.26',account:wa,name:'E2E WA',sessions:45,messages:90,error:state.whatsappError||null};
   case 'wechat_account_profile':return null;
   case 'wechat_session_catalog':return catalog(args.accountId);
   case 'wechat_sessions':return state.keys||args.accountId===wa?catalog(args.accountId).filter(s=>(state.settings.chat.allowed_usernames[args.accountId]??[]).includes(s.username)):[];
   case 'wechat_catalog_dates':return [['2026-11-18',2],['2026-11-17',1]];
   case 'wechat_scan':return {roots:['/e2e'],skipped:[],accounts:[{id:wx,dir:'/e2e',db_count:1,encrypted_count:1,total_bytes:1024,modified_at:100,dbs:[{rel:'test.db',path:'/e2e/test.db',size:1024,encrypted:true,salt:'test',has_wal:false}]}]};
   case 'wechat_key_status':return {path:'/e2e/keys',stored:state.keys,keys:state.keys?[{account:wx,rel:'test.db',salt:'test',source:'e2e',verified_at:'2026-11-18',verified_secs:1795000000}]:[]};
   case 'wechat_key_source_status':return {path:'/e2e/keys',exists:false,modified_at:null,age_seconds:null,secure:true,extractor_installed:true};
   case 'wechat_env':return {app_data_dir:'/e2e',cache_root:'/e2e',keys_path:'/e2e/keys',scanner_path:null};
   case 'wechat_platform_capabilities':return {platform:'macos',key_acquisition:'capture',supports_macos_capture:true,supports_key_import:true,supports_memory_scan:false};
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
   case 'wechat_avatars':case 'wechat_messages':case 'devify_my_spaces':case 'summary_history_list':case 'wechat_upload_state':return [];
   case 'agent_current':return {id:null,name:'未配置',enabled:false};
   default:state.errors.push(command);throw Error(`E2E unsupported IPC: ${command}`);
  }
 }
}});
window.addEventListener('error',event=>state.errors.push(event.message));
window.addEventListener('unhandledrejection',event=>state.errors.push(String(event.reason)));
createRoot(document.getElementById('root')!).render(<PrototypeShell />);
