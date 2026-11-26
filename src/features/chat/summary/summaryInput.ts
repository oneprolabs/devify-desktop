export type SummaryChat = { username: string; display: string; is_group: boolean; last_time: number | null };
export type SummaryMessage = { username: string; display: string; sender: string; time: number; text: string; is_group?:boolean; local_id?: number; source_shard?: string };
export type CloudSummary = {
  id: number|string; request_key: string; title: string; range_label: string; mode: 'time'|'pick';
  status: 'queued'|'running'|'ready'|'failed'; body: string; error: string; message_count: number;
  period_start: string; period_end: string; created_at: string; model: string;
  sessions: Array<{username:string;display:string;messages:number}>;
  task_id?:string;engine?:'cloud'|'local';caller?:string;initiator_id?:number;
  scenario?:string|null;language?:string;instructions?:string;prompt_version?:string;prompt_digest?:string;
};
export type SummaryPage = { offline?: boolean;list:CloudSummary[];local?:CloudSummary[];pagination:{total:number;page:number;next:string|null}};

export type SummaryRange = 'today'|'yesterday'|'three'|'week'|'month'|'custom'|'all';
type SummaryPeriod = {since:number;until:number};
type SummaryDates = {from:string;to:string};

// Conservative admission policy for the current 12,000-byte cloud budget.
// Reserve 2,000 bytes for the server prompt/framing; the server remains authoritative.
const CLOUD_SUMMARY_MAX_CHARACTERS = 3000;
const CLOUD_SUMMARY_MAX_INPUT_BYTES = 10000;
export function validateCloudSummaryInput(input: {
  title:string; range_label:string; language:string; messages:SummaryMessage[]; instructions?:string;
}, budget?: {max_content_bytes:number}) {
  const tooLong = () => { throw Error('所选内容超过单次总结范围，请缩小时间范围或减少消息后重试。'); };
  let characters = 0;
  const speakers:string[] = [];
  const speakerIds = new Map<string,number>();
  const chats = new Map<string,{name:string;messages:Array<[string,number,string]>}>();
  for (const message of input.messages) {
    characters += Array.from(message.text).length;
    if (characters > (budget ? 120000 : CLOUD_SUMMARY_MAX_CHARACTERS)) tooLong();
    if (!speakerIds.has(message.sender)) {
      speakerIds.set(message.sender,speakers.length);
      speakers.push(message.sender);
    }
    let chat = chats.get(message.username);
    if (!chat) { chat = {name:message.display,messages:[]}; chats.set(message.username,chat); }
    // A timestamp with seconds and an explicit offset is at most 25 characters.
    chat.messages.push(['2000-01-01T00:00:00+00:00',speakerIds.get(message.sender)!,message.text]);
  }
  // Mirror the compact model view, not the much larger transport JSON. Names occur once.
  const content = JSON.stringify({title:input.title,range:input.range_label,language:input.language,speakers,chats:[...chats.values()],...(input.instructions ? {instructions:input.instructions} : {})});
  if (new TextEncoder().encode(content).length > (budget?.max_content_bytes ?? CLOUD_SUMMARY_MAX_INPUT_BYTES)) tooLong();
}

function localDate(value: string) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) return null;
  const date = new Date(`${value}T00:00:00`);
  const [year,month,day] = value.split('-').map(Number);
  return date.getFullYear()===year && date.getMonth()===month-1 && date.getDate()===day ? date : null;
}

export function summaryBounds(range: SummaryRange, now = new Date(), dates?: SummaryDates): SummaryPeriod | null {
  if (range === 'all') return null;
  if (range === 'custom') {
    if (!dates?.from || !dates.to) throw Error('请选择自定义时间范围的开始日期和结束日期。');
    const start = localDate(dates.from), end = localDate(dates.to);
    if (!start || !end) throw Error('请选择有效的开始日期和结束日期。');
    if (end < start) throw Error('结束日期不能早于开始日期。');
    if (end > now) throw Error('日期不能晚于今天。');
    end.setDate(end.getDate()+1);
    end.setHours(0,0,0,0);
    return {since:Math.floor(start.getTime()/1000),until:Math.min(Math.floor(now.getTime()/1000),end.getTime()/1000-1)};
  }
  const start = new Date(now.getFullYear(), now.getMonth(), range==='month'?1:now.getDate());
  const end = new Date(now);
  if (range === 'yesterday') { end.setTime(start.getTime()-1000); start.setDate(start.getDate()-1); }
  if (range === 'three') start.setDate(start.getDate()-2);
  if (range === 'week') start.setDate(start.getDate()-((start.getDay()+6)%7));
  return {since:Math.floor(start.getTime()/1000),until:Math.floor(end.getTime()/1000)};
}

export function summaryPeriodError(period: SummaryPeriod | null, engine: 'cloud'|'local', all = period === null) {
  if (all && engine !== 'local') return '全部消息仅支持本地工具。请重新选择云端时间范围。';
  if (!period) return '';
  const {since,until} = period;
  const start = new Date(since*1000), end = new Date(until*1000);
  if (!Number.isInteger(since) || !Number.isInteger(until) || isNaN(+start) || isNaN(+end) || until<since) return '总结的时间范围无效。';
  const monthDelta = (end.getFullYear()-start.getFullYear())*12+end.getMonth()-start.getMonth();
  // End dates are inclusive: stop before the next month's same day, or at month-end if it doesn't exist.
  if (!all && (monthDelta>1 || (monthDelta===1 && end.getDate()>=start.getDate()))) {
    return '所选范围超过一个自然月，请缩短后重试。';
  }
  // shortcut: the cloud API still caps elapsed time at seven days, replace with a server capability when supported.
  return engine==='cloud' && until-since>7*86400 ? '当前服务端最多支持一周；请选择较短的范围。' : '';
}

type RawMessage = {local_id:number;source_shard?:string;time:number;kind:number;sender?:string|null;sender_name?:string|null;text:string;is_self?:boolean};
export function isSummaryText(message:{kind:number;source_shard?:string;text:string}):boolean {
  return Boolean(message.text.trim()) && (message.source_shard==='whatsapp' ? message.kind===0 : (message.kind&0xffff)===1);
}
type ReadPage = (args: {accountId:string;username:string;limit:number;before?:number;beforeId?:number;beforeShard?:string;strictHistory?:true})=>Promise<RawMessage[]>;
type HistoryBudget = { bytes: number; maxBytes: number };
export const ALL_MESSAGES_MAX_HISTORY_BYTES = 4 * 1024 * 1024;
const LOCAL_SUMMARY_BUDGET_ERROR = '本次聊天记录超过本地任务的 4 MiB 上限，请缩小范围。';
type MessagePosition = Pick<SummaryMessage,'time'|'local_id'|'source_shard'>;
export function compareSummaryMessages(a:MessagePosition,b:MessagePosition) {
  const left=a.source_shard??'',right=b.source_shard??'';
  return a.time-b.time || (a.local_id??0)-(b.local_id??0) || (left<right?-1:left>right?1:0);
}

export async function readSummaryRange(read:ReadPage, accountId:string, chat:SummaryChat, since:number|null, until:number|null, current:()=>boolean, allBudget?:HistoryBudget):Promise<SummaryMessage[]> {
  const result: SummaryMessage[] = [];
  let characters=0;
  let previous:RawMessage|undefined;
  // Native `before` excludes that whole second unless an ID is supplied.
  let cursor: {before?:number;beforeId?:number;beforeShard?:string} = until===null?{}:{before:until+1};
  const encoder = new TextEncoder();
  for (let page = 0; allBudget || page < 20; page++) {
    if (!current()) throw Error('SUMMARY_SCOPE_CHANGED');
    // Native reads clamp to 100. Asking for 500 makes a full page look like EOF.
    const limit=100;
    const batch=await read({accountId,username:chat.username,limit,...cursor,...(allBudget?{strictHistory:true as const}:{})});
    if (!current()) throw Error('SUMMARY_SCOPE_CHANGED');
    for (const message of batch) {
      if (allBudget) {
        if (!Number.isSafeInteger(message.time) || !Number.isSafeInteger(message.local_id) || !message.source_shard ||
          (previous && compareSummaryMessages(message,previous)>=0)) throw Error('消息读取未能继续，请重试。');
        previous=message;
      }
      if ((since===null || message.time>=since) && (until===null || message.time<=until) && isSummaryText(message)) {
        const item = {
          username: chat.username, display: chat.display, is_group: chat.is_group,
          sender: message.sender_name || message.sender || '', time: message.time,
          text: message.text, local_id: message.local_id, source_shard: message.source_shard,
        };
        if (allBudget) {
          allBudget.bytes += encoder.encode(JSON.stringify(item)).length + 1;
          if (allBudget.bytes > allBudget.maxBytes) throw Error(LOCAL_SUMMARY_BUDGET_ERROR);
        } else {
          characters+=message.text.length;
          if(characters>120000)throw Error('消息内容过多，请缩小时间范围或减少选择。');
        }
        result.push(item);
      }
    }
    const last=batch[batch.length-1];
    if (!last || batch.length<limit || (since!==null && last.time<since)) return result;
    const next={before:last.time,beforeId:last.local_id,beforeShard:last.source_shard};
    if (JSON.stringify(next)===JSON.stringify(cursor)) throw Error('消息读取未能继续，请重试。');
    cursor=next;
  }
  throw Error('这个范围的聊天记录过多，请缩小时间范围。');
}

export function validateSummaryMessages(messages:SummaryMessage[], primary:string, engine: 'cloud'|'local', all = false) {
  if (!messages.length) throw Error('这个范围没有可总结的文字消息。');
  if (!all && !messages.some(item=>item.username===primary)) throw Error('请选择当前会话的文字消息。');
  if (!all && (messages.length>2000 || messages.reduce((sum,item)=>sum+item.text.length,0)>120000)) throw Error('消息内容过多，请缩小时间范围或减少选择。');
  if (new Set(messages.map(item=>item.username)).size>8) throw Error('单次最多总结 8 个聊天。');
  let since = Infinity, until = -Infinity;
  for (const { time } of messages) {
    since = Math.min(since, time);
    until = Math.max(until, time);
  }
  const problem=summaryPeriodError({since,until},engine,all);
  if (problem) throw Error(problem);
  return {since,until};
}
