#!/usr/bin/env node
// Verify the project-scoped configuration consumed by a Codex Remote session.
// This does not start a session or change the host configuration.
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const profile = 'agent-capability-gate-loop';
const requiredText = [
  'approval_policy = "never"',
  `default_permissions = "${profile}"`,
  'allow_login_shell = false',
  'web_search = "disabled"',
  'plugins = false',
  'apps = false',
  'browser_use = false',
  'computer_use = false',
  'in_app_browser = false',
  'hooks = false',
  '[apps._default]',
  'enabled = false',
  '[mcp_servers.firefox-devtools]',
  '[mcp_servers.node_repl]',
];

function run(command, args, options = {}) {
  return spawnSync(command, args, { encoding: 'utf8', ...options });
}

function main() {
  const args = process.argv.slice(2);
  if (args.length !== 2 || args[0] !== '--project' || !path.isAbsolute(args[1])) {
    throw new Error('Usage: remote-loop-config.js --project ABSOLUTE-PROJECT-PATH');
  }
  const project = path.resolve(args[1]);
  const configPath = path.join(project, '.codex', 'config.toml');
  const config = fs.readFileSync(configPath, 'utf8');
  const missing = requiredText.filter((entry) => !config.includes(entry));
  if (missing.length) throw new Error(`Remote project config is missing: ${missing.join(', ')}`);

  const parsed = run('codex', ['--strict-config', '--help'], { cwd: project, stdio: 'ignore' });
  if (parsed.error || parsed.status !== 0) throw new Error(`Codex config parse failed: ${parsed.error?.code || parsed.status}`);

  const inventory = run('codex', ['mcp', 'list', '--json'], { cwd: project });
  if (inventory.error || inventory.status !== 0) throw new Error(`Codex MCP inventory failed: ${inventory.error?.code || inventory.status}`);
  const start = inventory.stdout.indexOf('[');
  if (start < 0) throw new Error('Codex MCP inventory has no JSON array');
  const servers = JSON.parse(inventory.stdout.slice(start));
  if (!Array.isArray(servers)) throw new Error('Codex MCP inventory is not an array');
  const enabled = servers.filter((server) => server.enabled).map((server) => server.name).sort();
  if (enabled.length) throw new Error(`Remote MCP allowlist is not empty: ${enabled.join(', ')}`);

  process.stdout.write(`${JSON.stringify({
    session: 'codex-remote',
    project,
    configPath,
    approvalPolicy: 'never',
    permissionProfile: profile,
    enabledCodexMcp: [],
  }, null, 2)}\n`);
}

try { main(); }
catch (error) { console.error(`Remote loop config failed: ${error.message}`); process.exitCode = 1; }
