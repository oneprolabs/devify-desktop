import assert from 'node:assert/strict'
import { findDailyPlan, enableDailyPlan, stepTimePart } from '../src/dailyPlan.ts'
const plan = { id: 3, title: '微信工作日报', period: 'day', time_local: '18:00:00', enabled: true, timezone: 'Asia/Shanghai' }
assert.equal(findDailyPlan([plan])?.id, 3)
const serverPlan = { ...plan, id: 5, title: '每日报告', time_local: '18:00:00' }
assert.equal(findDailyPlan([plan, serverPlan])?.id, 5)
assert.equal(findDailyPlan([]), null)
assert.throws(() => findDailyPlan([plan, { ...plan, id: 4 }]))
assert.throws(() => findDailyPlan([{ ...plan, time_local: 'invalid' }]))
let created = 0, updated = []
const api = { list: async () => [], create: async (time) => { created++; assert.equal(time, '08:30') }, enable: async (id) => updated.push(id) }
await enableDailyPlan(api, '08:30')
assert.equal(created, 1)
assert.equal(stepTimePart('18:59', 'minute', 1), '18:00')
assert.equal(stepTimePart('00:15', 'hour', -1), '23:15')
assert.equal(stepTimePart('18:00', 'minute', -1), '18:59')
await enableDailyPlan({ ...api, list: async () => [serverPlan] }, '08:30')
assert.deepEqual(updated, [5])
assert.equal(created, 1)
console.log('daily plan checks passed')
