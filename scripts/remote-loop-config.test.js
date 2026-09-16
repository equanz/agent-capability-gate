const assert = require('node:assert/strict');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');
const { missingSettings } = require('./remote-loop-config.js');

test('remote project config selects never and closes the Codex MCP surface', () => {
  const result = spawnSync(process.execPath, [path.join(__dirname, 'remote-loop-config.js'), '--project', process.cwd()], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  const prepared = JSON.parse(result.stdout);
  assert.equal(prepared.session, 'codex-remote');
  assert.equal(prepared.approvalPolicy, 'never');
  assert.equal(prepared.permissionProfile, 'agent-capability-gate-loop');
  assert.equal(prepared.defaultSubagentModel, 'gpt-5.6-luna');
  assert.equal(prepared.maxConcurrentSubagents, 1);
  assert.deepEqual(prepared.enabledCodexMcp, []);
});

test('remote configuration requires each policy section explicitly', () => {
  const config = require('node:fs').readFileSync('.codex/config.toml', 'utf8')
    .replace('[mcp_servers.firefox-devtools]\nenabled = false', '[mcp_servers.firefox-devtools]\nenabled = true');
  assert.deepEqual(missingSettings(config), ['mcp_servers.firefox-devtools:enabled = false']);
});

test('remote configuration requires the read-only command runtimes', () => {
  const config = require('node:fs').readFileSync('.codex/config.toml', 'utf8')
    .replace('"/usr/local" = "read"\n', '');
  assert.deepEqual(missingSettings(config), [
    'permissions.agent-capability-gate-loop.filesystem:"/usr/local" = "read"',
  ]);
});

test('remote configuration requires the Git index lock write allowance', () => {
  const config = require('node:fs').readFileSync('.codex/config.toml', 'utf8')
    .replace('".git/index.lock" = "write"\n', '');
  assert.deepEqual(missingSettings(config), [
    'permissions.agent-capability-gate-loop.filesystem.:workspace_roots:".git/index.lock" = "write"',
  ]);
});
