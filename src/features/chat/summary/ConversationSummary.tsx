import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { desktopInvoke as invoke } from '../../../shared/api/desktopInvoke';
import { t, tf, dateLocale } from '../../../shared/i18n/locale';
import { useInternalAccount, useCapability } from '../../auth/entitlements';
import { formatSize } from '../../../shared/content/localDataFormat';
import { renderAgentMarkdown } from '../../../shared/content/summary';
import { ALL_MESSAGES_MAX_HISTORY_BYTES, compareSummaryMessages, readSummaryRange, summaryBounds, summaryPeriodError, validateSummaryMessages, validateCloudSummaryInput, type CloudSummary, type SummaryChat, type SummaryMessage, type SummaryPage, type SummaryRange } from './summaryInput';
import { refreshTaskRuntime } from '../../tasks/taskRuntime';
import { mergeSummaryRecords, reconcileSummaryPage, summaryRecordKey } from './summaryRecords';
import { SummaryRequestOptions } from './SummaryRequestOptions';
import { useSummaryOptions } from './useSummaryOptions';
import { summaryOptionFields, summaryScenarioLabel } from './summaryOptions';
import './conversation-summary.css';

const RANGES: Array<[SummaryRange,string]> = [['today','今天'],['yesterday','昨天'],['three','近 3 天'],['week','本周'],['month','本月'],['custom','自定义'],['all','全部消息']];
const waiting = (row:CloudSummary) => row.status==='queued'||row.status==='running';
const keyOf = (message:SummaryMessage) => JSON.stringify([message.username,message.source_shard,message.local_id,message.time]);

function dateRangeLabel({since,until}: NonNullable<ReturnType<typeof summaryBounds>>, withTime=false) {
  const format=new Intl.DateTimeFormat(dateLocale(),{year:'numeric',month:'2-digit',day:'2-digit',...(withTime?{hour:'2-digit',minute:'2-digit',hourCycle:'h23'}:{})});
  return `${format.format(since*1000)} — ${format.format(until*1000)}`;
}

function DisclosureArrow({ expanded }: { expanded:boolean }) {
  return <svg className="summary-disclosure-arrow" data-expanded={expanded} width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m6 9 6 6 6-6"/></svg>;
}

function SummaryBody({ body }: { body:string }) {
  const id=useId();
  const content=useRef<HTMLDivElement>(null);
  const [expanded,setExpanded]=useState(false);
  const [overflow,setOverflow]=useState(false);
  useLayoutEffect(()=>{
    const element=content.current;if(!element)return;
    const measure=()=>setOverflow(element.getBoundingClientRect().height>300);
    measure();
    const observer=new ResizeObserver(measure);observer.observe(element);
    return()=>observer.disconnect();
  },[body]);
  return <div className="summary-body">
    <div id={id} className={`summary-result${overflow&&!expanded?' is-collapsed':''}`}><div ref={content} className="wxv-ai-body">{renderAgentMarkdown(body)}</div></div>
    {overflow&&<button type="button" className="summary-disclosure summary-body-toggle" aria-expanded={expanded} aria-controls={id} onClick={()=>setExpanded(value=>!value)}><span>{expanded?t('收起全文'):t('展开全文')}</span><DisclosureArrow expanded={expanded}/></button>}
  </div>;
}

export function ConversationSummary({ visible, localAgentReady = false, localAgentName = '', localAgentModel = '', serviceUrl, spaceId, accountId, session, sessions, selection, selectionTotal = selection.length, pickRequest, query, onPick, onOpenTasks, onStatus }: {
  visible:boolean;localAgentReady?:boolean;localAgentName?:string;localAgentModel?:string;serviceUrl?:string;spaceId:string|null;accountId:string|null;session:SummaryChat|null;sessions:SummaryChat[];
  selection:SummaryMessage[];selectionTotal?:number;pickRequest:number;query:string;onPick:()=>void;onOpenTasks:()=>void;
  onStatus:(state:{count:number;busy:boolean})=>void;
}) {
  const { internal }=useInternalAccount();
  const { allowed: localGranted } = useCapability('ai.agent_summary');
  const canUseLocal = internal && localGranted && localAgentReady;
  const [rows,setRows]=useState<CloudSummary[]>([]);
  const [open,setOpen]=useState(false);
  const [range,setRange]=useState<SummaryRange>('today');
  const [dates,setDates]=useState({from:'',to:''});
  const [mode,setMode]=useState<'time'|'pick'>('time');
  const [engine,setEngine]=useState<'cloud'|'local'>('cloud');
  const [extras,setExtras]=useState<string[]>([]);
  const [picker,setPicker]=useState(false);
  const [chatQuery,setChatQuery]=useState('');
  const [excluded,setExcluded]=useState<string[]>([]);
  const [phase,setPhase]=useState('');
  const [allStats,setAllStats]=useState<{ count: number; bytes: number; batches: number } | null>(null);
  const [loading,setLoading]=useState(false);
  const [error,setError]=useState('');
  const [formError,setFormError]=useState('');
  const [notice,setNotice]=useState('');
  const [page,setPage]=useState(1);
  const [more,setMore]=useState(false);
  const [total,setTotal]=useState(0);
  const panelId=useId();
  const [deleting,setDeleting]=useState<number|string|null>(null);
  const lock=useRef(false);
  const scope=JSON.stringify([serviceUrl,spaceId,accountId,session?.username]);
  const currentScope=useRef(scope);currentScope.current=scope;
  const alive=useRef(true);
  const submission=useRef<{scope:string;engine:string;body:Record<string,unknown>;attempted?:boolean}|null>(null);
  const readController=useRef<AbortController|null>(null);
  const request=useRef(0);
  const options = useSummaryOptions(scope, visible && open);
  const missingExtra=extras.some(username=>!sessions.some(chat=>chat.username===username));
  const resetSubmission=useCallback(()=>{submission.current=null;setAllStats(null);},[]);
  useEffect(()=>{alive.current=true;return()=>{alive.current=false;request.current++;};},[]);
  useLayoutEffect(()=>{
    if(submission.current?.body.all_messages&&!submission.current.attempted)resetSubmission();
    return ()=>readController.current?.abort();
  },[scope,visible,canUseLocal,pickRequest,missingExtra,options.view,options.draft,resetSubmission]);
  const current=(owner:string)=>alive.current&&currentScope.current===owner;

  const load=useCallback(async(target=1,quiet=false)=>{
    if(!spaceId||!accountId||!session)return;
    const owner=scope,id=++request.current;
    if(!quiet)setLoading(true);
    try {
      const result=await invoke<SummaryPage>('cloud_summary_list',{spaceId,accountId,username:session.username,page:target});
      if(!current(owner)||id!==request.current)return;
      const reconciled=reconcileSummaryPage(result);
      setRows(old=>(target===1?reconciled.rows:mergeSummaryRecords(old,reconciled.rows)).slice(0,100));
      setTotal(reconciled.total);setMore(Boolean(result.pagination.next)&&target<5);setPage(target);setError(result.offline?t('暂时无法读取云端总结，当前显示本机已有的记录。'):'');
    }catch(reason){if(current(owner)&&id===request.current)setError(String(reason));}
    finally{if(current(owner)&&id===request.current)setLoading(false);}
  },[spaceId,accountId,session?.username,scope]);

  useEffect(()=>{
    request.current++;setRows([]);setOpen(false);setExtras([]);setExcluded([]);setError('');setFormError('');setNotice('');setPhase('');setTotal(0);setMore(false);resetSubmission();
    // The synchronous lock belongs to an in-flight submission, never to the current tab.
  },[scope,resetSubmission]);
  useEffect(()=>{if(visible)void load();return()=>{request.current++;};},[visible,load]);
  const pending=rows.some(waiting);
  useEffect(()=>{onStatus({count:Math.max(total,rows.length),busy:pending||Boolean(phase)});},[total,rows.length,pending,phase,onStatus]);
  useEffect(()=>{
    if(!visible||!pending)return;
    let stopped=false,timer:ReturnType<typeof setTimeout>;
    const poll=async()=>{if(stopped)return;if(!document.hidden)await load(1,true);if(!stopped)timer=setTimeout(poll,5000);};
    timer=setTimeout(poll,5000);
    const show=()=>{if(!document.hidden)void load(1,true);};
    document.addEventListener('visibilitychange',show);
    return()=>{stopped=true;clearTimeout(timer);document.removeEventListener('visibilitychange',show);};
  },[visible,pending,load]);
  useEffect(()=>{if(pickRequest){setMode('pick');setOpen(true);setExcluded([]);resetSubmission();}},[pickRequest,resetSubmission]);
  useEffect(()=>{if(!canUseLocal){setEngine('cloud');resetSubmission();}},[canUseLocal,resetSubmission]);
  useEffect(()=>{resetSubmission();setExcluded([]);},[selection,resetSubmission]);
  const change=()=>{resetSubmission();setError('');setFormError('');setNotice('');};
  const selected=selection.filter(item=>!excluded.includes(keyOf(item)));
  // The reader supplies text-only input, while its selection includes media.
  const nonTextCount=Math.max(0,selectionTotal-selection.length);
  const selectedTotal=selected.length+nonTextCount;
  const allMessages=mode==='time'&&range==='all';
  const localAll=allMessages&&engine==='local';
  const estimatedInputSize = allStats
    ? (allStats.bytes < 1024 ? `${allStats.bytes} B` : formatSize(allStats.bytes))
    : '';
  const preview=(()=>{
    if(!visible||!open||mode!=='time')return null;
    try {
      const bounds=summaryBounds(range,new Date(),dates);
      const invalid=summaryPeriodError(bounds,'local');
      return {bounds,invalid:Boolean(invalid),error:invalid||summaryPeriodError(bounds,engine)};
    } catch (reason) {
      return {bounds:null,invalid:true,error:String(reason).replace(/^Error: /,'')};
    }
  })();
  const rangeErrorId=`${panelId}-range-error`;
  const cannotGenerate=Boolean(phase)||!session||!options.view||options.loading||Boolean(preview?.error)||(mode==='pick'&&!selected.length);

  const generate=async(confirmAll=false)=>{
    if(lock.current||!spaceId||!accountId||!session||preview?.error)return;
    if(allMessages&&(Boolean(allStats)!==confirmAll))return;
    if(engine==='local'&&!canUseLocal){setFormError(t('请先在智能体集成中开启可用的本地智能体。'));return;}
    lock.current=true;const owner=scope;request.current++;setLoading(false);
    let allRead:AbortController|null=null;
    setError('');setFormError('');setNotice('');
    try {
      let body=submission.current?.scope===owner&&submission.current.engine===engine?submission.current.body:null;
      if(confirmAll&&!body)return;
      if(!body){
        setPhase(t('正在读取所选消息…'));
        const fields = summaryOptionFields(options.view, options.draft);
        const scenario = options.view!.config.scenarios.find(item => item.id === fields.scenario)!;
        let messages:SummaryMessage[]=[];
        const bounds=mode==='time'?summaryBounds(range,new Date(),dates):null;
        if(mode==='pick')messages=selected;
        else {
          const problem=summaryPeriodError(bounds,engine);
          if(problem)throw Error(problem);
          const budget = allMessages
            ? { bytes: 1, maxBytes: ALL_MESSAGES_MAX_HISTORY_BYTES }
            : undefined;
          const controller=allMessages?new AbortController():null;
          readController.current=allRead=controller;
          // Release the UI while the current native read finishes; its late result is discarded.
          const cancelled=controller&&new Promise<never>((_,reject)=>{
            controller.signal.addEventListener('abort',()=>reject(Error('SUMMARY_SCOPE_CHANGED')),{once:true});
          });
          // At most eight conversations, read sequentially in bounded pages.
          for(const username of [session.username,...extras]){
            const chat=username===session.username?session:sessions.find(item=>item.username===username);
            if(!chat){
              if(allMessages)throw Error(t('所选聊天已变化，请重新选择。'));
              continue;
            }
            const read=readSummaryRange(args=>invoke('wechat_messages',args),accountId,chat,bounds?.since??null,bounds?.until??null,()=>current(owner)&&!controller?.signal.aborted,budget);
            const pageMessages = await (cancelled ? Promise.race([read, cancelled]) : read);
            for (const message of pageMessages) messages.push(message);
            if(!allMessages && messages.length>2000)throw Error(t('消息内容过多，请缩小时间范围或减少选择。'));
          }
        }
        if(!current(owner)||allRead?.signal.aborted)return;
        const times=validateSummaryMessages(messages,session.username,engine,allMessages);
        const period=bounds??times;
        if(allMessages)messages.sort(compareSummaryMessages);
        else if(engine==='local'&&(messages.length>400||messages.reduce((sum,item)=>sum+item.text.length,0)>40000))throw Error(t('本地单次总结内容过多，请缩小时间范围或减少选择。'));
        const label=mode==='pick'?tf('所选 {0} 条消息',messages.length):range==='custom'?dateRangeLabel(period):t(RANGES.find(item=>item[0]===range)![1]);
        const input={title:session.display,range_label:label,...fields,messages};
        if (engine === 'cloud') validateCloudSummaryInput(input, scenario.budgets.cloud);
        body={request_key:crypto.randomUUID(),...input,mode,period_start:new Date(period.since*1000).toISOString(),period_end:new Date(period.until*1000).toISOString(),...(allMessages?{all_messages:true}:{})};
        if (allMessages) {
          const estimate = await invoke<{ bytes: number; batches: number }>('summary_config_preview_local', { body });
          if (!current(owner) || allRead?.signal.aborted) return;
          setAllStats({ count: messages.length, ...estimate });
        }
        submission.current = { scope: owner, engine, body };
        if (allMessages) return;
      }
      if(!current(owner))return;
      if(submission.current?.body===body)submission.current.attempted=true;
      setPhase(t('正在创建总结任务…'));
      const record=await invoke<CloudSummary>('summary_task_enqueue',{spaceId,accountId,username:session.username,engine,body});
      if(!current(owner))return;
      request.current++;resetSubmission();setRows(old=>mergeSummaryRecords(old,[record]));setOpen(false);setNotice(t('总结任务已创建，可以继续使用应用。'));void refreshTaskRuntime();void load(1,true);
    }catch(reason){if(current(owner)&&!allRead?.signal.aborted&&String(reason)!=='Error: SUMMARY_SCOPE_CHANGED')setFormError(String(reason));}
    finally{readController.current=null;lock.current=false;if(current(owner))setPhase('');}
  };
  const action=async(row:CloudSummary,kind:'retry'|'delete')=>{
    if(lock.current||!spaceId)return;lock.current=true;const owner=scope;setError('');
    try {
      if(row.task_id&&kind==='retry')await invoke('summary_task_retry',{taskId:row.task_id});
      else if(typeof row.id==='string'&&row.task_id)await invoke('summary_task_delete',{taskId:row.task_id});
      else await invoke('cloud_summary_action',{spaceId,summaryId:row.id,action:kind});
      if(current(owner)){setDeleting(null);await load();void refreshTaskRuntime();}
    }catch(reason){if(current(owner))setError(String(reason));}
    finally{lock.current=false;}
  };
  const visibleRows=rows.filter(row=>!query.trim()||`${row.range_label} ${row.body}`.toLowerCase().includes(query.trim().toLowerCase()));
  return <div className="chat-summary-page"><div className="chat-summary-column conversation-summary">
    <div className="chat-summary-intro"><p>{t('总结是所选聊天的独立快照，不参与待办分析。')}</p><button type="button" className={open?'summary-disclosure':'chat-summary-cta'} aria-expanded={open} aria-controls={panelId} disabled={!session||Boolean(phase)} onClick={()=>{setOpen(value=>!value);change();}}>{open?<><span>{t('收起')}</span><DisclosureArrow expanded/></>:t('生成总结')}</button></div>
    {open&&<section id={panelId} className="chat-summary-panel" aria-label={t('生成聊天总结')}>
      <div className="summary-segments" role="group" aria-label={t('选择消息方式')}>{([['time','按时间周期'],['pick','选具体消息']] as const).map(([id,label])=><button type="button" key={id} aria-pressed={mode===id} disabled={Boolean(phase)} onClick={()=>{setMode(id);change();}}>{t(label)}</button>)}</div>
      {mode==='time'?<>
        <div className="chat-summary-row"><span className="chat-summary-label">{t('时间')}</span><div className="chat-summary-time summary-date-range">
          <div className="chat-summary-pills" role="group" aria-label={t('选择时间范围')}>{RANGES.filter(([id])=>id!=='all'||engine==='local').map(([id,label])=><button type="button" key={id} className={range===id?'on':''} aria-pressed={range===id} disabled={Boolean(phase)} onClick={()=>{setRange(id);change();}}>{t(label)}</button>)}</div>
          {range==='custom'&&<div className="imports-dates summary-custom-dates">
            <label>{t('开始日期')}<input type="date" value={dates.from} disabled={Boolean(phase)} aria-invalid={preview?.invalid||undefined} aria-describedby={preview?.error?rangeErrorId:undefined} onChange={event=>{setDates({...dates,from:event.target.value});change();}}/></label>
            <span aria-hidden="true">{t('至')}</span>
            <label>{t('结束日期')}<input type="date" value={dates.to} disabled={Boolean(phase)} aria-invalid={preview?.invalid||undefined} aria-describedby={preview?.error?rangeErrorId:undefined} onChange={event=>{setDates({...dates,to:event.target.value});change();}}/></label>
          </div>}
          {preview?.bounds&&!preview.invalid&&(range==='month'||range==='custom')&&<p className="summary-date-value"><strong>{t('范围')}</strong> {dateRangeLabel(preview.bounds,range==='month')}{range==='custom'&&` ${t('（含结束日）')}`}</p>}
          {localAll?<><p className="summary-date-feedback">{t('读取所选聊天在本机已有的全部文字。历史较多时耗时更长，也可能占用更多模型上下文和额度；本地工具仍可能使用在线模型。')}</p>
            {allStats&&<div className="summary-feedback"><span className="summary-date-value" role="status">{allStats.batches > 1
              ? tf('{0} 条文字消息，分 {1} 批总结，预计输入 {2}（含各批提示词）。完成后合并。', allStats.count, allStats.batches, estimatedInputSize)
              : tf('{0} 条文字消息，预计输入 {1}（含提示词）。', allStats.count, estimatedInputSize)}</span>
              <button type="button" className="summary-all-confirm" disabled={cannotGenerate} onClick={()=>void generate(true)}>{t('确认并生成')}</button></div>}</>
            :!allMessages&&<p className="chat-summary-hint">{t('单次最长一个自然月；消息过多时，请缩小时间范围。')}</p>}
          {preview?.error&&<p id={rangeErrorId} className={`summary-date-feedback${preview.invalid?' invalid':''}`} role="alert">{t(preview.error)}</p>}
        </div></div>
        <div className="chat-summary-row"><span className="chat-summary-label">{t('聊天')}</span><div className="summary-chat-scope"><div className="summary-chat-chips"><span>{session?.display}</span>{extras.map(username=><span key={username}>{sessions.find(item=>item.username===username)?.display??username}<button type="button" aria-label={tf('移除 {0}',username)} disabled={Boolean(phase)} onClick={()=>{setExtras(old=>old.filter(item=>item!==username));change();}}>×</button></span>)}<button type="button" disabled={Boolean(phase)} onClick={()=>setPicker(value=>!value)}>{picker?t('完成选择'):t('添加聊天')}</button></div>
        {picker&&<div className="summary-chat-picker"><input aria-label={t('搜索聊天名称')} placeholder={t('搜索聊天名称')} value={chatQuery} onChange={event=>setChatQuery(event.target.value)}/><div>{sessions.filter(chat=>chat.username!==session?.username&&chat.display.toLowerCase().includes(chatQuery.toLowerCase())).slice(0,80).map(chat=><label key={chat.username}><input type="checkbox" checked={extras.includes(chat.username)} disabled={Boolean(phase)||(!extras.includes(chat.username)&&extras.length>=7)} onChange={event=>{setExtras(old=>event.target.checked?[...old,chat.username]:old.filter(item=>item!==chat.username));change();}}/><span>{chat.display}</span><small>{chat.is_group?t('群聊'):t('单聊')}</small></label>)}</div><small>{t('最多 8 个聊天，仅列出当前平台已加载的会话。')}</small></div>}</div></div>
      </>:<div className="summary-message-picker"><div className="summary-message-picker-head"><strong>{tf('已选 {0} 条消息',selectedTotal)}</strong><button type="button" onClick={onPick}>{t('去原始记录选择')}</button></div>
        {nonTextCount>0&&<p role="status">{tf('其中 {0} 条文字参与总结，{1} 条非文字消息不参与。',selected.length,nonTextCount)}</p>}
        {selection.length?selection.slice(0,100).map(message=><label key={keyOf(message)}><input type="checkbox" checked={!excluded.includes(keyOf(message))} onChange={event=>{setExcluded(old=>event.target.checked?old.filter(key=>key!==keyOf(message)):[...old,keyOf(message)]);change();}}/><span><small>{message.display} · {message.sender} · {new Date(message.time*1000).toLocaleString(dateLocale(),{month:'numeric',day:'numeric',hour:'2-digit',minute:'2-digit'})}</small><span>{message.text}</span></span></label>):<p>{nonTextCount?t('所选消息不含可总结的文字，请返回选择文字消息。'):t('在原始记录中点击消息进行选择，再回来生成总结。')}</p>}{selection.length>100&&<p>{tf('仅预览前 100 条文字消息，共有 {0} 条文字参与总结。',selected.length)}</p>}</div>}
      <SummaryRequestOptions view={options.view} draft={options.draft} loading={options.loading} error={options.error} disabled={Boolean(phase)}
        onChange={patch=>{options.setDraft(old=>({...old,...patch}));change();}} onReload={()=>{change();void options.reload();}}/>
      <div className="chat-summary-row"><span className="chat-summary-label">{t('智能体')}</span>
        <div className="summary-segments summary-agent-options" role="group" aria-label={t('智能体')}>
          <button type="button" aria-pressed={engine==='cloud'} disabled={Boolean(phase)} onClick={()=>{setEngine('cloud');change();}}>
            <span>{t('服务端')}</span><small>{t('默认')}</small>
          </button>
          {internal&&<button type="button" aria-pressed={engine==='local'} disabled={Boolean(phase)||!canUseLocal} title={!canUseLocal?t('请先在智能体集成中开启可用的本地智能体。'):undefined} onClick={()=>{setEngine('local');change();}}>
            <span>{localAgentName?tf('本地 · {0}',localAgentName):t('本地')}</span>
          </button>}
        </div>
      </div>
      <p className="chat-summary-note">{engine==='cloud'?t('由云端处理文字内容，完成后自动保存。图片、语音和视频不参与此次总结。'):tf('模型：{0}。沿用智能体已有的登录状态。',localAgentModel.trim()||t('沿用智能体默认模型'))}</p>
      {engine==='local'&&!localAll&&<p className="chat-summary-note">{t('本地工具可能使用在线模型服务；所选文字会交给该工具处理。')}</p>}
      {formError&&<div className="summary-feedback error" role="alert"><span>{t(formError.replace(/^Error: /,''))}</span></div>}
      <div className="chat-summary-actions"><button type="button" disabled={Boolean(phase)&&!readController.current} onClick={()=>{if(readController.current)readController.current.abort();else{setOpen(false);change();}}}>{t(phase&&readController.current?'取消读取':'取消')}</button><button type="button" className="primary" disabled={cannotGenerate||Boolean(localAll&&allStats)} onClick={()=>void generate()}>{phase||t(localAll?(allStats?'请在上方确认生成':'读取全部消息'):'开始生成')}</button></div>
    </section>}
    {error&&<div className="summary-feedback error" role="alert"><span>{t(error.replace(/^Error: /,''))}</span><button type="button" disabled={loading} onClick={()=>void load()}>{t('重试读取')}</button></div>}
    {notice&&<div className="summary-feedback" role="status"><span>{notice}</span><button type="button" onClick={onOpenTasks}>{t('查看任务')}</button></div>}
    {loading && !rows.length && <div className="summary-progress" role="status">
      <i className="chat-summary-spinner" aria-hidden="true"/>{t('正在读取总结…')}
    </div>}
    {visibleRows.map(row=><article className={`chat-summary-card${waiting(row)?' doing':''}`} key={summaryRecordKey(row)}>
      <header className="chat-summary-card-head"><strong>{row.range_label}</strong><span className="chat-summary-kind">{row.engine==='local'?(row.caller||t('本地工具')):t('云端总结')}</span><span className="chat-summary-card-meta">{tf('{0} 条消息',row.message_count)} · {new Date(row.created_at).toLocaleString(dateLocale(),{month:'numeric',day:'numeric',hour:'2-digit',minute:'2-digit'})}</span></header>
      {waiting(row)?<div className="summary-progress" role="status"><i className="chat-summary-spinner"/>{row.status==='queued'?t('等待处理，可在任务中查看。'):t('正在生成总结，可以继续使用应用。')}</div>:row.status==='failed'?<div className="summary-feedback error"><span>{row.error||t('总结生成失败，请重试。')}</span><button type="button" onClick={()=>void action(row,'retry')}>{t('重试')}</button></div>:<SummaryBody body={row.body}/>}
      <footer className="chat-summary-card-foot"><span>{tf('{0} 个聊天',row.sessions?.length??1)}</span>{summaryScenarioLabel(row.scenario)&&<span>{t(summaryScenarioLabel(row.scenario))}{row.language?` · ${t(row.language)}`:''}</span>}<span className="grow"/>{!waiting(row)&&(deleting===row.id?<><button type="button" onClick={()=>setDeleting(null)}>{t('取消')}</button><button type="button" onClick={()=>void action(row,'delete')}>{t('确认删除')}</button></>:<button type="button" onClick={()=>setDeleting(row.id)}>{t('删除')}</button>)}</footer>
    </article>)}
    {!loading&&!error&&!visibleRows.length&&<div className="chat-summary-empty">{query.trim()?t('没有匹配的总结。'):t('还没有为这个会话生成过总结。选择时间范围或具体消息后开始。')}</div>}
    {more&&<button type="button" className="summary-load-more" disabled={loading} onClick={()=>void load(page+1)}>{loading?t('正在读取…'):t('加载更多总结')}</button>}
  </div></div>;
}
