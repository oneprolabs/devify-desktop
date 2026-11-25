import assert from 'node:assert/strict';
import {readSummaryRange,validateSummaryMessages,validateCloudSummaryInput,summaryBounds,summaryPeriodError,isSummaryText} from '../src/features/chat/summary/summaryInput.ts';

const chat={username:'group',display:'Group',is_group:true,last_time:null};
const messages=Array.from({length:140},(_,i)=>({local_id:140-i,time:1000-Math.floor(i/3),source_shard:'message_0.db',kind:1,text:`text ${i}`,sender_name:'member'}));
const calls=[];
const result=await readSummaryRange(async args=>{calls.push(args);return messages.filter(m=>!args.before||m.time<args.before||(m.time===args.before&&m.local_id<args.beforeId)).slice(0,args.limit);},'account',chat,0,2000,()=>true);
assert.equal(result.length,140,'A native page of 100 is not EOF');
assert.equal(calls.length,2);assert.equal(calls[0].limit,100);assert.equal(calls[0].before,2001);assert.equal(calls[1].beforeShard,'message_0.db');
assert.equal(new Set(result.map(m=>m.local_id)).size,140,'Same-second messages remain distinct');
const newer=Array.from({length:2100},(_,i)=>({local_id:4000-i,time:5000-i,kind:1,text:'newer'}));
const history=[...newer,...messages];
const readHistory=async args=>history.filter(m=>m.time<args.before||(m.time===args.before&&m.local_id<args.beforeId)).slice(0,args.limit);
assert.deepEqual(await readSummaryRange(readHistory,'account',chat,999,1000,()=>true),
  result.slice(0,6),'Seek past recent history and include both boundary seconds');
assert.deepEqual(await readSummaryRange(readHistory,'account',chat,1010,1020,()=>true),[]);
await assert.rejects(()=>readSummaryRange(readHistory,'account',chat,0,5000,()=>true),/记录过多/);
await assert.rejects(()=>readSummaryRange(async()=>[{...messages[0],text:'a'.repeat(120001)}],'account',chat,0,2000,()=>true),/内容过多/);
let reads=0;
await assert.rejects(()=>readSummaryRange(async()=>{reads++;return [];},'account',chat,0,2000,()=>false),/SUMMARY_SCOPE_CHANGED/);
assert.equal(reads,0,'A changed account cannot start another read');
const chosen=messages.map(m=>({...m,username:chat.username}));
const picked=[chosen[0],{...chosen[1],time:chosen[0].time+7*86400+1}];
assert.throws(()=>validateSummaryMessages([],chat.username,'cloud'),/没有/);
assert.throws(()=>validateSummaryMessages(chosen,'other','local'),/当前会话/);
assert.throws(()=>validateSummaryMessages(picked,chat.username,'cloud'),/一周/);
assert.doesNotThrow(()=>validateSummaryMessages(picked,chat.username,'local'));
assert.throws(()=>validateSummaryMessages([{...chosen[0],text:'a'.repeat(120001)}],chat.username,'local'),/内容过多/);
assert.deepEqual(validateSummaryMessages([chosen[0]],chat.username,'cloud'),{since:1000,until:1000});
const now=new Date(2026,10,20,12);
const epoch=(...time)=>new Date(...time).getTime()/1000;
assert.deepEqual(summaryBounds('today',now),{since:epoch(2026,10,20),until:+now/1000});
assert.equal(summaryBounds('yesterday',now).until-summaryBounds('yesterday',now).since,86399);
assert.equal(summaryBounds('three',now).since,epoch(2026,10,18));
assert.equal(summaryBounds('week',now).since,epoch(2026,10,16));
assert.deepEqual(summaryBounds('month',now),{since:epoch(2026,10,1),until:+now/1000});
const custom=(from,to)=>summaryBounds('custom',now,{from,to});
assert.deepEqual(custom('2024-02-29','2024-02-29'),{since:epoch(2024,1,29),until:epoch(2024,2,1)-1});
assert.equal(custom('2026-11-20','2026-11-20').until,+now/1000,'Today ends at now');
assert.throws(()=>custom('','2026-11-20'),/请选择/);
assert.throws(()=>custom('2025-02-29','2025-03-01'),/有效/);
assert.throws(()=>custom('2026-11-20','2026-11-19'),/早于/);
assert.throws(()=>custom('2026-11-20','2026-11-21'),/晚于今天/);
for(const [from,to,firstInvalid] of [
  ['2024-01-01','2024-01-31','2024-02-01'],
  ['2024-01-31','2024-02-29','2024-03-01'],
  ['2025-01-31','2025-02-28','2025-03-01'],
  ['2024-02-01','2024-02-29','2024-03-01'],
  ['2024-03-01','2024-03-31','2024-04-01'],
  ['2025-12-15','2026-01-14','2026-01-15'],
]) {
  assert.equal(summaryPeriodError(custom(from,to),'local'),'');
  assert.match(summaryPeriodError(custom(from,firstInvalid),'local'),/一个自然月/);
  assert.match(summaryPeriodError(custom(from,to),'cloud'),/一周/);
}
assert.equal(summaryPeriodError(custom('2026-10-30','2026-11-04'),'cloud'),'');
assert.equal(summaryPeriodError({since:1000,until:1000+7*86400},'cloud'),'');
assert.equal(custom('2024-03-09','2024-03-10').until,epoch(2024,2,11)-1,'End is computed from local midnight, including DST days');
console.log('Summary cursor paging, limits, time range and changed-account cancellation passed');

assert.equal(isSummaryText({kind:0,source_shard:'whatsapp',text:'WA text'}),true);
assert.equal(isSummaryText({kind:1,source_shard:'whatsapp',text:'[图片]'}),false);
assert.equal(isSummaryText({kind:65537,source_shard:'message_0.db',text:'WeChat text'}),true);
const wa=await readSummaryRange(async()=>[{local_id:1,time:1000,kind:0,source_shard:'whatsapp',text:'WA text'}],'account',chat,0,2000,()=>true);
assert.equal(wa.length,1,'WhatsApp native type 0 is a text message');

const cloudInput = (text, extra={}) => ({title:'Group',range_label:'今天',language:'中文',messages:[{username:'group',display:'Group',sender:'member',time:1000,text,...extra}]});
assert.doesNotThrow(()=>validateCloudSummaryInput(cloudInput('中'.repeat(3000))),'Chinese text at the character boundary fits the reserved budget');
assert.throws(()=>validateCloudSummaryInput(cloudInput('a'.repeat(3001))),/超过单次总结范围/,'Long ASCII is blocked too');
assert.throws(()=>validateCloudSummaryInput(cloudInput('😀'.repeat(3000))),/超过单次总结范围/,'UTF-8 size is checked independently of character count');
assert.throws(()=>validateCloudSummaryInput(cloudInput('short',{display:'群'.repeat(4000)})),/超过单次总结范围/,'Metadata consumes the input budget');
const manyShort=cloudInput('a');
manyShort.messages=Array.from({length:300},()=>({...manyShort.messages[0]}));
assert.throws(()=>validateCloudSummaryInput(manyShort),/超过单次总结范围/,'Many short messages can exceed the budget through timestamp/framing overhead');
const repeatedNames=cloudInput('消息',{display:'群'.repeat(100),sender:'人'.repeat(100)});
repeatedNames.messages=Array.from({length:68},()=>({...repeatedNames.messages[0]}));
assert.doesNotThrow(()=>validateCloudSummaryInput(repeatedNames),'Repeated names are compacted, avoiding the old transport-JSON false rejection');
assert.doesNotThrow(()=>validateSummaryMessages(cloudInput('a'.repeat(3001)).messages,'group','local'),'Local tools keep their own limits');
console.log('Cloud pre-submit character, UTF-8, metadata and many-message admission checks passed');
