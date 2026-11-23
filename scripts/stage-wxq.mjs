#!/usr/bin/env node
// Stage the read-only `wxq` CLI next to the bundle resources.
//
// The app only serves the binary over HTTP, it never executes it, so it does not
// belong in Contents/MacOS (Tauri/`codesign` would treat it as a helper
// executable). It is built by a separate opt-in cargo step and copied here with
// the Rust target triple suffix that `bundle.resources` expects.
//
// Two modes, because `tauri_build::build()` validates the `bundle.resources`
// globs *while the crate compiles*, and the matched file is produced by that very
// compile. A clean checkout (CI, or anyone who deleted `src-tauri/binaries/`)
// therefore has a chicken-and-egg problem:
//
//   node scripts/stage-wxq.mjs --prepare   → seed an empty placeholder first
//   cargo build --release --bin wxq ...    → build scripts see the glob match
//   node scripts/stage-wxq.mjs             → copy the real binary over it
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const host = execFileSync('rustc', ['-vV'], { encoding: 'utf8' })
  .match(/^host: (.+)$/m)?.[1]
  ?.trim();
if (!host) {
  console.error('无法从 rustc -vV 读取 host triple。');
  process.exit(1);
}

const ext = process.platform === 'win32' ? '.exe' : '';
const development = process.argv.includes('--dev');
const dir = join(root, 'src-tauri', development ? 'binaries-dev' : 'binaries');
const target = join(dir, `wxq-${host}${ext}`);
const source = join(root, 'src-tauri', 'target', 'release', `wxq${ext}`);

mkdirSync(dir, { recursive: true });

if (process.argv.includes('--prepare')) {
  if (!existsSync(target)) {
    writeFileSync(target, '');
    console.log(`已创建占位 ${target}`);
  }
} else {
  copyFileSync(source, target);
  // Validate the copied file, not a shared Cargo output that another build can replace.
  const info = JSON.parse(execFileSync(target, ['build-info', '--json'], { encoding: 'utf8' }));
  const expected = 'local.wechat.devifybridge' + (development ? '.dev' : '');
  if (info.identifier !== expected || info.development !== development) {
    throw new Error('CLI build channel mismatch; refusing to bundle it.');
  }
  console.log(`已暂存 ${target}`);
}
