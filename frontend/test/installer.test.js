import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

test('the release asset is the Jay-I setup, built from a pinned Jay-I', () => {
  const workflow = readFileSync(new URL('../../.github/workflows/installer.yml', import.meta.url), 'utf8');
  for (const [, ref] of workflow.matchAll(/^\s*-?\s*uses:\s*\S+@(\S+)/gm)) {
    assert.match(ref, /^[0-9a-f]{40}$/, `every action is pinned by full SHA, got ${ref}`);
  }
  const pin = workflow.match(/repository: TyxMatty\/Jay-I\s+ref: (\S+)/);
  assert.ok(pin, 'the workflow checks out TyxMatty/Jay-I');
  assert.match(pin[1], /^[0-9a-f]{40}$/);
  assert.match(workflow, /jay-i\/scripts\/build-setup\.ps1/);
  assert.match(workflow, /30 \* 1024 \* 1024/);
  // Only the setup is uploaded: the hub takes any asset ending in _x64-setup.exe.
  const upload = workflow.match(/upload-artifact@[\s\S]*?\n\s+path: (\S+)/);
  assert.ok(upload, 'the workflow uploads an artifact');
  assert.doesNotMatch(upload[1], /bundle\/nsis/);
  assert.match(upload[1], /^setup-out\//);
  // The draft release carries the setup too.
  const release = workflow.match(/^.*gh release create.*$/m);
  assert.ok(release, 'the workflow creates a draft release');
  assert.match(release[0], /--draft/);
  assert.match(release[0], /setup-out\//);
  assert.doesNotMatch(release[0], /bundle\/nsis/);
  // The updater (the jay-i crate) and the setup come from the same Jay-I commit.
  const cargo = readFileSync(new URL('../src-tauri/Cargo.toml', import.meta.url), 'utf8');
  const dep = cargo.match(/^jay-i\s*=.*$/m);
  assert.ok(dep, 'Cargo.toml depends on jay-i');
  assert.ok(dep[0].includes(`rev = "${pin[1]}"`), `Cargo.toml jay-i rev must be ${pin[1]}`);
});
