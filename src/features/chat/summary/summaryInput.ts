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

export type SummaryRange = 'today'|'yesterday'|'three'|'week';

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

export function summaryBounds(range: SummaryRange, now = new Date()) {
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const end = new Date(now);
  if (range === 'yesterday') { end.setTime(start.getTime()-1000); start.setDate(start.getDate()-1); }
  if (range === 'three') start.setDate(start.getDate()-2);
  if (range === 'week') start.setDate(start.getDate()-((start.getDay()+6)%7));
  return {since:Math.floor(start.getTime()/1000),until:Math.floor(end.getTime()/1000)};
}

type RawMessage = {local_id:number;source_shard?:string;time:number;kind:number;sender?:string|null;sender_name?:string|null;text:string;is_self?:boolean};
export function isSummaryText(message:{kind:number;source_shard?:string;text:string}):boolean {
  return Boolean(message.text.trim()) && (message.source_shard==='whatsapp' ? message.kind===0 : (message.kind&0xffff)===1);
}
type ReadPage = (args: {accountId:string;username:string;limit:number;before?:number;beforeId?:number;beforeShard?:string})=>Promise<RawMessage[]>;
export async function readSummaryRange(read:ReadPage, accountId:string, chat:SummaryChat, since:number, until:number, current:()=>boolean):Promise<SummaryMessage[]> {
  const result: SummaryMessage[] = [];
  let characters=0;
  let cursor: {before?:number;beforeId?:number;beforeShard?:string} = {};
  for (let page=0;page<20;page++) {
    if (!current()) throw Error('SUMMARY_SCOPE_CHANGED');
    // Native reads clamp to 100. Asking for 500 makes a full page look like EOF.
    const batch=await read({accountId,username:chat.username,limit:100,...cursor});
    if (!current()) throw Error('SUMMARY_SCOPE_CHANGED');
    for (const message of batch) {
      if (message.time>=since && message.time<=until && isSummaryText(message)) {
        characters+=message.text.length;
        if(characters>120000)throw Error('消息内容过多，请缩小时间范围或减少选择。');
        result.push({username:chat.username,display:chat.display,is_group:chat.is_group,sender:message.sender_name||message.sender||'',time:message.time,text:message.text,local_id:message.local_id,source_shard:message.source_shard});
      }
    }
    const last=batch[batch.length-1];
    if (!last || batch.length<100 || last.time<since) return result;
    const next={before:last.time,beforeId:last.local_id,beforeShard:last.source_shard};
    if (JSON.stringify(next)===JSON.stringify(cursor)) throw Error('消息读取未能继续，请重试。');
    cursor=next;
  }
  throw Error('这个范围的聊天记录过多，请缩小时间范围。');
}

export function validateSummaryMessages(messages:SummaryMessage[], primary:string) {
  if (!messages.length) throw Error('这个范围没有可总结的文字消息。');
  if (!messages.some(item=>item.username===primary)) throw Error('请选择当前会话的文字消息。');
  if (messages.length>2000 || messages.reduce((sum,item)=>sum+item.text.length,0)>120000) throw Error('消息内容过多，请缩小时间范围或减少选择。');
  if (new Set(messages.map(item=>item.username)).size>8) throw Error('单次最多总结 8 个聊天。');
  const times=messages.map(item=>item.time);
  const since=Math.min(...times),until=Math.max(...times);
  if (until-since>7*86400) throw Error('单次总结的时间范围不能超过一周。');
  return {since,until};
}
