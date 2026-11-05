#!/usr/bin/env node
// Stage the read-only `wxq` CLI next to the bundle resources.
//
// The app only serves the binary over HTTP, it never executes it, so it does not
// belong in Contents/MacOS (Tauri/`codesign` would treat it as a helper
// executable). It is built by a separate opt-in cargo step and copied here with
// the Rust target triple suffix that `bundle.resources` expects.
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync } from 'node:fs';
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
const source = join(root, 'src-tauri', 'target', 'release', `wxq${ext}`);
const dir = join(root, 'src-tauri', 'binaries');
const target = join(dir, `wxq-${host}${ext}`);

mkdirSync(dir, { recursive: true });
copyFileSync(source, target);
console.log(`已暂存 ${target}`);
