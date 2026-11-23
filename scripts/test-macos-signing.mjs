import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

// Exercise signing selection with the same self-signed identity as CI. Nothing
// reaches a real keychain, build, installation or publishing operation.
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'aimychats-signing-'));
try {
  for (const dir of ['scripts', 'src-tauri', 'tools', 'home']) fs.mkdirSync(path.join(root, dir));
  const script = path.join(root, 'scripts/macos-build.sh');
  fs.copyFileSync(new URL('./macos-build.sh', import.meta.url), script);
  fs.writeFileSync(path.join(root, 'src-tauri/tauri.conf.json'), '{"version":"0.1.26"}');
  const keychain = path.join(root, 'ci.keychain');
  fs.writeFileSync(keychain, 'synthetic keychain placeholder');
  fs.writeFileSync(path.join(root, 'tools/security'), `#!/bin/sh
if [ "$MOCK_IDENTITY" = missing ]; then exit 0; fi
case " $* " in *" -v "*) printf '0 valid identities found\\n';; *) printf '1) AAAAAA "AIMyChats Local Signing" (CSSMERR_TP_NOT_TRUSTED)\\n';; esac
`, { mode: 0o755 });
  fs.writeFileSync(path.join(root, 'tools/caffeinate'), '#!/bin/sh\nprintf "synthetic build reached\\n"\nexit 42\n', { mode: 0o755 });
  const env = { ...process.env, HOME: path.join(root, 'home'), PATH: `${path.join(root, 'tools')}${path.delimiter}${process.env.PATH}`, AIMYCHATS_SIGNING_IDENTITY: 'AIMyChats Local Signing', AIMYCHATS_SIGNING_KEYCHAIN: keychain };
  const run = missing => spawnSync('/bin/bash', [script, '--production', '--no-install'], {
    env: { ...env, MOCK_IDENTITY: missing ? 'missing' : 'self-signed' }, encoding: 'utf8', timeout: 10_000,
  });
  const configured = run(false);
  assert.equal(configured.status, 42, configured.stderr);
  assert.ok(configured.stdout.includes('synthetic build reached'));
  const missing = run(true);
  assert.equal(missing.status, 1, missing.stderr);
  assert.ok(missing.stderr.includes('找不到签名身份：AIMyChats Local Signing'));
  assert.ok(!missing.stderr.includes('unbound variable'));
  assert.ok(!missing.stdout.includes('synthetic build reached'));
} finally {
  fs.rmSync(root, { recursive: true, force: true });
}
console.log('Self-signed CI identity selection and missing-identity error checks passed');
