import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
const directory = new URL('./', import.meta.url);
const tests = fs.readdirSync(directory).filter(file => /^test-.*\.mjs$/.test(file) && !file.endsWith('.browser.mjs') && !['test-frontend.mjs', 'test-module-loader.mjs', 'test-module-register.mjs'].includes(file)).sort();
for (const test of tests) {
  const result = spawnSync(process.execPath, ['--experimental-test-module-mocks', '--import', fileURLToPath(new URL('test-module-register.mjs', directory)), fileURLToPath(new URL(test, directory))], { stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
console.log(`All ${tests.length} frontend regression scripts passed.`);
