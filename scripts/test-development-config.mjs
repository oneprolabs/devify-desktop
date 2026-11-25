import assert from 'node:assert/strict';
import fs from 'node:fs';
const load = file => JSON.parse(fs.readFileSync(new URL(file, import.meta.url), 'utf8'));
const prod = load('../src-tauri/tauri.conf.json');
const dev = load('../src-tauri/tauri.dev-app.conf.json');
assert.equal(prod.productName, 'AIMyChats');
assert.equal(prod.identifier, 'local.wechat.devifybridge');
assert.deepEqual(prod.plugins['deep-link'].desktop.schemes, ['devify']);
assert.ok(prod.plugins.updater.endpoints.length);
assert.equal(dev.productName, 'AIMyChats Dev');
assert.notEqual(dev.identifier, prod.identifier);
assert.deepEqual(dev.plugins['deep-link'].desktop.schemes, ['devify-dev']);
assert.deepEqual(dev.plugins.updater.endpoints, []);
assert.deepEqual(dev.build.features, ['dev-app']);
assert.notEqual(dev.build.devUrl, prod.build.devUrl);
assert.match(dev.build.beforeBuildCommand, /stage:aimychats-cli:dev/);
assert.match(dev.build.beforeDevCommand, /stage-aimychats-cli\.mjs --prepare --dev/);
const scripts = load('../package.json').scripts;
assert.match(scripts['stage:aimychats-cli:dev'], /aux-binaries,dev-app/);
assert.match(scripts['build:dev'], /--mode development-app/);
assert.match(scripts['desktop:dev'], /tauri.dev-app.conf.json/);
assert.match(scripts['desktop:install:dev'], /--dev/);
console.log('Production identity/channel retained; development app, callback scheme, build features and update configuration isolated');
const release = fs.readFileSync(new URL('./prepare-release.sh', import.meta.url), 'utf8');
assert.match(release, /macos-build\.sh --production --no-install/);
assert.match(release, /Refusing to publish a non-production app/);
assert.notEqual(dev.build.frontendDist, prod.build.frontendDist);
assert.notDeepEqual(dev.bundle.resources, prod.bundle.resources);
assert.match(scripts['build:dev'], /--outDir dist-dev/);
assert.match(scripts['stage:aimychats-cli:dev'], /stage-aimychats-cli\.mjs --dev/);
// Tauri matches src/bin filenames to Cargo target names when selecting packaged executables.
const cargo = fs.readFileSync(new URL('../src-tauri/Cargo.toml', import.meta.url), 'utf8');
assert.match(cargo, /name = "aimychats-cli"\npath = "src\/bin\/aimychats-cli\/main\.rs"\nrequired-features = \["aux-binaries"\]/);
for (const legacy of ['wxq.rs', 'wxq']) {
  assert.ok(!fs.existsSync(new URL(`../src-tauri/src/bin/${legacy}`, import.meta.url)));
}
