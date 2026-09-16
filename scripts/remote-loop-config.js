#!/usr/bin/env node
// Verify the project-scoped configuration consumed by a Codex Remote session.
// This does not start a session or change the host configuration.
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const profile = 'agent-capability-gate-loop';
const requiredRootSettings = [
  'approval_policy = "never"',
  `default_permissions = "${profile}"`,
  'allow_login_shell = false',
  'web_search = "disabled"',
];
const requiredSectionSettings = {
  'shell_environment_policy.set': [
    'GIT_CONFIG_NOSYSTEM = "1"',
    'GIT_CONFIG_GLOBAL = "/dev/null"',
    'GIT_TERMINAL_PROMPT = "0"',
  ],
  features: [
  'plugins = false',
  'apps = false',
  'browser_use = false',
  'computer_use = false',
  'in_app_browser = false',
  'hooks = false',
  ],
  'apps._default': ['enabled = false'],
  agents: [
    'enabled = true',
    'max_concurrent_threads_per_session = 1',
    'default_subagent_model = "gpt-5.6-luna"',
    'default_subagent_reasoning_effort = "high"',
  ],
  'mcp_servers.firefox-devtools': ['enabled = false'],
  'mcp_servers.node_repl': ['enabled = false'],
};
const requiredRuntimeReadSettings = [
  '"/bin" = "read"',
  '"/usr/bin" = "read"',
  '"/usr/lib" = "read"',
  '"/System/Library" = "read"',
  '"/usr/local" = "read"',
  '"~/.n/bin/node" = "read"',
  '"~/.cargo/bin" = "read"',
  '"~/.rustup/settings.toml" = "read"',
  '"~/.rustup/toolchains" = "read"',
];
const requiredWorkspaceWriteSettings = [
  '".git" = "write"',
  '".git/index.lock" = "write"',
];

function run(command, args, options = {}) {
  return spawnSync(command, args, { encoding: 'utf8', ...options });
}

function configSections(config) {
  const sections = new Map([['', []]]);
  let current = '';
  for (const rawLine of config.split('\n')) {
    const section = rawLine.match(/^\[([^\]]+)\]\s*$/);
    if (section) {
      current = section[1];
      if (!sections.has(current)) sections.set(current, []);
    } else if (rawLine.trim() && !rawLine.trimStart().startsWith('#')) {
      sections.get(current).push(rawLine.trim());
    }
  }
  return sections;
}

function missingSettings(config) {
  const sections = configSections(config);
  const missing = requiredRootSettings.filter((entry) => !sections.get('').includes(entry))
    .map((entry) => `root:${entry}`);
  for (const [section, settings] of Object.entries(requiredSectionSettings)) {
    for (const entry of settings) {
      if (!sections.get(section)?.includes(entry)) missing.push(`${section}:${entry}`);
    }
  }
  for (const entry of requiredRuntimeReadSettings) {
    if (!sections.get(`permissions.${profile}.filesystem`)?.includes(entry)) {
      missing.push(`permissions.${profile}.filesystem:${entry}`);
    }
  }
  for (const entry of requiredWorkspaceWriteSettings) {
    if (!sections.get(`permissions.${profile}.filesystem.":workspace_roots"`)?.includes(entry)) {
      missing.push(`permissions.${profile}.filesystem.:workspace_roots:${entry}`);
    }
  }
  return missing;
}

function main() {
  const args = process.argv.slice(2);
  if (args.length !== 2 || args[0] !== '--project' || !path.isAbsolute(args[1])) {
    throw new Error('Usage: remote-loop-config.js --project ABSOLUTE-PROJECT-PATH');
  }
  const project = path.resolve(args[1]);
  const configPath = path.join(project, '.codex', 'config.toml');
  const config = fs.readFileSync(configPath, 'utf8');
  const missing = missingSettings(config);
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
    defaultSubagentModel: 'gpt-5.6-luna',
    maxConcurrentSubagents: 1,
    enabledCodexMcp: [],
  }, null, 2)}\n`);
}

if (require.main === module) {
  try { main(); }
  catch (error) { console.error(`Remote loop config failed: ${error.message}`); process.exitCode = 1; }
}

module.exports = { configSections, missingSettings };
