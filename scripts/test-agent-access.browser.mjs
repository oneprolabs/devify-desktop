import assert from 'node:assert/strict';

// Run through Ego against connectors-e2e.html?internal. Native IPC is synthetic;
// this verifies the production React page, not actual host permission or installation.
export async function runAgentAccessChecks(page) {
  const before=await page.evaluate(()=>JSON.stringify(window.__E2E__.settings.agent));
  assert.deepEqual(await page.evaluate(()=>[...document.querySelectorAll('.integration-tool strong')].map(x=>x.textContent)),['Codex CLI','Claude Code CLI','OpenCode CLI','WorkBuddy']);
  assert.equal(await page.evaluate(()=>document.querySelector('.integration-skills').textContent.includes('ChatGPT')),false);
  assert.equal(await page.evaluate(()=>document.querySelector('[aria-label="OpenCode CLI"] button').disabled),true);
  assert.ok(await page.evaluate(()=>document.querySelector('[aria-label="WorkBuddy"]').textContent.includes('已安装客户端')));
  await page.click('[aria-label="Codex CLI"] .wizard-action');
  await page.waitForFunction(()=>document.querySelector('[aria-label="Codex CLI"] .wizard-action').textContent.includes('更新 Skills'));
  assert.equal(await page.evaluate(()=>window.__E2E__.calls.filter(x=>x.command==='agent_access_install').length),1);
  assert.equal(await page.evaluate(()=>JSON.stringify(window.__E2E__.settings.agent)),before,'installation does not enable summaries or change provider');
  await page.evaluate(()=>{
    Object.defineProperty(navigator,'clipboard',{configurable:true,value:{async write(items){if(window.__clipboardFailure)throw Error('clipboard denied');window.__copied=await(await items[0].getType('text/plain')).text();},async writeText(text){if(window.__clipboardFailure)throw Error('clipboard denied');window.__copied=text;}}});
  });
  await page.click('.integration-generic button');
  await page.waitForFunction(()=>window.__copied?.includes('identify your own Skills directory'));
  assert.equal(await page.evaluate(()=>document.querySelector('.integration-generic textarea')),null,'prompt body remains hidden');
  assert.equal(await page.evaluate(()=>JSON.stringify(window.__E2E__.settings.agent)),before);
  await page.evaluate(()=>{window.__clipboardFailure=true;});
  await page.click('.integration-generic button');
  await page.waitForFunction(()=>document.querySelector('.integration-generic [role="alert"]')?.textContent.includes('clipboard denied'));
  await page.evaluate(()=>{window.__clipboardFailure=false;window.__E2E__.agentInstallError='Synthetic kit unavailable';});
  await page.click('.integration-generic button');
  await page.waitForFunction(()=>document.querySelector('.integration-generic [role="alert"]')?.textContent.includes('Synthetic kit unavailable'));
  await page.click('[aria-label="Claude Code CLI"] .wizard-action');
  await page.waitForFunction(()=>document.querySelector('.integration-skills>[role="alert"]')?.textContent.includes('Synthetic kit unavailable'));
  assert.equal(await page.evaluate(()=>Boolean(window.__E2E__.agentSkills['claude-code'])),false,'failed install does not claim installed');
  await page.evaluate(()=>{window.__E2E__.agentInstallError='';});
  await page.click('[aria-label="Claude Code CLI"] .wizard-action');
  await page.waitForFunction(()=>document.querySelector('[aria-label="Claude Code CLI"] .wizard-action').textContent.includes('更新 Skills'));
  await page.click('.integration-generic button');
  await page.waitForFunction(()=>!document.querySelector('.integration-generic [role="alert"]'));
  assert.equal(await page.evaluate(()=>window.__E2E__.calls.filter(x=>x.command==='app_settings_save').length),0,'Skills operations never save summary settings');
  assert.deepEqual(await page.evaluate(()=>window.__E2E__.errors),[]);
  return {catalog:4,installation:'simulated',clipboard:'stubbed',failures:'verified',summarySettings:'unchanged'};
}
