const assert = require('node:assert/strict');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');

test('remote project config selects never and closes the Codex MCP surface', () => {
  const result = spawnSync(process.execPath, [path.join(__dirname, 'remote-loop-config.js'), '--project', process.cwd()], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  const prepared = JSON.parse(result.stdout);
  assert.equal(prepared.session, 'codex-remote');
  assert.equal(prepared.approvalPolicy, 'never');
  assert.equal(prepared.permissionProfile, 'agent-capability-gate-loop');
  assert.deepEqual(prepared.enabledCodexMcp, []);
});
