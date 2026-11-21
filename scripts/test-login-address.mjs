import assert from 'node:assert/strict'
import fs from 'node:fs'
import ts from 'typescript'
const source = fs.readFileSync(new URL('../src/shared/api/serviceUrl.ts', import.meta.url), 'utf8')
const js = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ES2022, target: ts.ScriptTarget.ES2022 } }).outputText
const { apiBaseUrl } = await import(`data:text/javascript;base64,${Buffer.from(js).toString('base64')}`)
for (const address of ['https://aimychats.com/', 'https://www.aimychats.com', 'https://app.aimychats.com/']) {
  assert.equal(apiBaseUrl(address), 'https://app.aimychats.com')
  assert.equal(`${apiBaseUrl(address)}/login?forgot=1`, 'https://app.aimychats.com/login?forgot=1')
}
assert.equal(apiBaseUrl('http://127.0.0.1:18080/'), 'http://127.0.0.1:18080')
console.log('login and recovery URL mapping passed')
