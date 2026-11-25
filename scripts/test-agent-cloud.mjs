import assert from 'node:assert/strict';
import fs from 'node:fs';
const read = path => fs.readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
const skill = read('skills/aimychats/SKILL.md');
const cloud = read('src-tauri/src/mcp/cloud.rs');
for (const tool of ['get_cloud_context','list_todos','get_todo','list_reports','get_report','list_topics','get_topic','list_topic_reports']) {
  assert.ok(skill.includes(`aimychats_${tool}`), `bundled Skill must document aimychats_${tool}`);
  assert.ok(cloud.includes(`aimychats_${tool}`));
}
for (const command of ['cloud-context','todos','todo','reports','report','topics','topic','topic-reports']) {
  assert.ok(skill.includes(`wxq ${command}`), `missing CLI example ${command}`);
}
assert.match(skill, /source_info\.subscription_id/);
assert.match(skill, /50/);
assert.match(skill, /digest.*不是.*我的待办/);
assert.match(skill, /云端.*远程读取/);
assert.ok(read('src/features/agents/AgentAccess.tsx').includes("t('使用智能体访问 AIMyChats')"));
assert.ok(read('src/features/agents/AgentAccess.tsx').includes('不会生成报告、完成待办或修改订阅'));
assert.ok(read('src/features/agents/AgentUsage.tsx').includes('已经生成的上周周报'));
console.log('Cloud Skill discovery, CLI examples, contract limits and Agent UI copy are aligned.');
