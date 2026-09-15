#!/usr/bin/env node
// Generated from decide-then-execute's project preflight; extended with
// independent-clone and denied-operation probes.
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const net = require('node:net');
const { spawnSync } = require('node:child_process');

const profile = 'agent-capability-gate-loop';
const workPaths = ['.work/cargo-home', '.work/target', '.work/tmp', '.work/reports'];
const commands = ['codex', 'cargo', 'rustc', 'git', 'node'];
const homeDir = process.env.HOME || os.homedir();
const codexHome = process.env.CODEX_HOME || path.join(homeDir, '.codex');
const agentsHome = path.join(homeDir, '.agents');
const globalReadFiles = [
  path.join(codexHome, 'AGENTS.md'),
  path.join(codexHome, 'config.toml'),
  path.join(codexHome, 'memories', 'MEMORY.md'),
  path.join(agentsHome, 'skills', 'decide-then-execute', 'SKILL.md'),
  path.join(codexHome, 'skills', '.system', 'openai-docs', 'SKILL.md'),
];
const globalReadDirectories = [
  path.join(codexHome, 'memories'),
  path.join(codexHome, 'skills'),
  path.join(agentsHome, 'skills'),
];
// Fake MCP is a test child, not a Codex-side MCP tool. The MVP needs none.
const allowedCodexMcpServers = [];
const denied = new Set(['EACCES', 'EPERM', 'ENETUNREACH', 'EHOSTUNREACH']);
const failures = [];

function run(command, args, options = {}) {
  return spawnSync(command, args, { encoding: 'utf8', ...options });
}

function checkResult(label, result) {
  if (result.error || result.status !== 0) failures.push(`${label}: ${result.error?.code || result.status}`);
}

function git(args) {
  return run('git', args, {
    env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null', GIT_TERMINAL_PROMPT: '0' },
  });
}

function expectDenied(label, action) {
  try {
    action();
    failures.push(`${label}: unexpectedly allowed`);
  } catch (error) {
    if (!denied.has(error.code)) failures.push(`${label}: inconclusive ${error.code || error.message}`);
  }
}

function parseArgs() {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === '--static') return { mode: 'static' };
  if (args.length === 1 && args[0] === '--surface') return { mode: 'surface' };
  if (args.length !== 9 || args[0] !== '--effective' || args[1] !== '--source-git-dir' ||
      args[3] !== '--outside-sentinel' || args[5] !== '--env-sentinel' ||
      args[7] !== '--codex-home-sentinel') {
    throw new Error('Usage: preflight.js --static | --surface | --effective --source-git-dir ABS --outside-sentinel ABS --env-sentinel ABS --codex-home-sentinel ABS');
  }
  const [sourceGitDir, outsideSentinel, envSentinel, codexHomeSentinel] = [args[2], args[4], args[6], args[8]];
  if (![sourceGitDir, outsideSentinel, envSentinel, codexHomeSentinel].every(path.isAbsolute)) throw new Error('Paths must be absolute');
  if (path.dirname(codexHomeSentinel) !== path.resolve(codexHome)) throw new Error('Codex home sentinel must be directly inside Codex home');
  return { mode: 'effective', sourceGitDir, outsideSentinel, envSentinel, codexHomeSentinel };
}

function staticChecks() {
  const config = fs.readFileSync('.codex/config.toml', 'utf8');
  if (!config.includes(`[permissions.${profile}]`) || !config.includes('enabled = false')) failures.push('Loop profile definition missing');
  for (const item of [...globalReadFiles.slice(0, 2), ...globalReadDirectories]) {
    const relative = path.relative(homeDir, item);
    if (!config.includes(`"~/${relative}" = "read"`)) failures.push(`Global read allowance missing: ${item}`);
  }
  checkResult('Codex strict config parse', run('codex', ['--strict-config', '--help'], { stdio: 'ignore' }));
  for (const command of commands) checkResult(`${command} --version`, run(command, ['--version'], { stdio: 'ignore' }));
}

function checkCodexMcpSurface() {
  const result = run('codex', ['mcp', 'list', '--json']);
  checkResult('Codex MCP inventory', result);
  if (result.status !== 0) return;
  const start = result.stdout.indexOf('[');
  if (start < 0) { failures.push('Codex MCP inventory has no JSON array'); return; }
  let servers;
  try { servers = JSON.parse(result.stdout.slice(start)); }
  catch (error) { failures.push(`Codex MCP inventory cannot be parsed: ${error.message}`); return; }
  if (!Array.isArray(servers)) { failures.push('Codex MCP inventory is not an array'); return; }
  const enabled = servers.filter((server) => server.enabled).map((server) => server.name).sort();
  const allowed = [...allowedCodexMcpServers].sort();
  if (JSON.stringify(enabled) !== JSON.stringify(allowed)) {
    failures.push(`Enabled Codex MCP servers differ from allowlist: ${enabled.join(', ') || '(none)'}`);
  }
}

function checkRemoteProjectConfig() {
  const result = run(process.execPath, [path.resolve('scripts/remote-loop-config.js'), '--project', process.cwd()]);
  checkResult('Remote project configuration', result);
  if (result.status !== 0) return;
  try {
    const prepared = JSON.parse(result.stdout);
    if (JSON.stringify(prepared.enabledCodexMcp) !== JSON.stringify(allowedCodexMcpServers)) {
      failures.push('Prepared Codex MCP allowlist differs from baseline policy');
    }
    if (prepared.session !== 'codex-remote' || prepared.approvalPolicy !== 'never' ||
        prepared.permissionProfile !== profile) {
      failures.push('Remote project configuration is missing approval or permission profile');
    }
  } catch (error) { failures.push(`Prepared Codex launch cannot be parsed: ${error.message}`); }
}

function checkGitIsolation(sourceGitDir) {
  const common = git(['rev-parse', '--path-format=absolute', '--git-common-dir']);
  checkResult('Git common dir', common);
  if (common.status !== 0) return;
  // The source checkout must be unreadable in the loop profile; compare the
  // prepared absolute name without opening it here. The outer judge also
  // checks physical separation before launching the loop.
  if (fs.realpathSync(common.stdout.trim()) === path.resolve(sourceGitDir)) failures.push('Git common dir shared with source');
  if (!fs.statSync('.git').isDirectory()) failures.push('.git is not an independent directory');
  if (fs.existsSync('.git/objects/info/alternates')) failures.push('Git alternates present');
  const remotes = git(['remote']);
  checkResult('Git remotes', remotes);
  if (remotes.stdout.trim()) failures.push('Clone still has a remote');
  const helper = git(['config', '--local', '--get', 'credential.helper']);
  if (helper.status === 0 && helper.stdout.trim()) failures.push('Local credential helper present');
  else if (![0, 1].includes(helper.status)) failures.push('Credential helper inspection failed');

  const tree = git(['write-tree']);
  const head = git(['rev-parse', 'HEAD']);
  checkResult('Git write-tree', tree);
  checkResult('Git HEAD', head);
  if (tree.status !== 0 || head.status !== 0) return;
  const commit = run('git', ['commit-tree', tree.stdout.trim(), '-p', head.stdout.trim(), '-m', 'disposable preflight checkpoint'], {
    env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null',
      GIT_AUTHOR_NAME: 'Preflight', GIT_AUTHOR_EMAIL: 'preflight@invalid.example',
      GIT_COMMITTER_NAME: 'Preflight', GIT_COMMITTER_EMAIL: 'preflight@invalid.example' },
  });
  checkResult('Local commit object', commit);
  if (commit.status === 0) {
    const ref = `refs/preflight/${process.pid}`;
    checkResult('Local preflight ref', git(['update-ref', ref, commit.stdout.trim()]));
    checkResult('Preflight ref cleanup', git(['update-ref', '-d', ref]));
  }
}

function checkFilesystem({ outsideSentinel, envSentinel, codexHomeSentinel }) {
  for (const item of ['docs/design.md', 'docs/implementation-design.md',
    'docs/verification-design.md', 'docs/autonomous-development.md',
    'Cargo.toml', 'Cargo.lock', 'verify', 'tests/requirements.toml']) {
    try {
      fs.readFileSync(item);
      fs.closeSync(fs.openSync(item, 'a'));
    } catch (error) {
      failures.push(`Required editable input ${item}: ${error.code || error.message}`);
    }
  }
  for (const target of workPaths) {
    fs.mkdirSync(target, { recursive: true });
    const temporary = fs.mkdtempSync(path.join(target, '.preflight-'));
    fs.rmdirSync(temporary);
  }
  for (const item of globalReadFiles) {
    try { fs.readFileSync(item); }
    catch (error) { failures.push(`Read global input ${item}: ${error.code || error.message}`); }
    expectDenied(`write global input ${item}`, () => fs.closeSync(fs.openSync(item, 'a')));
  }
  for (const item of globalReadDirectories) {
    try { fs.readdirSync(item); }
    catch (error) { failures.push(`List global input ${item}: ${error.code || error.message}`); }
    expectDenied(`write global directory ${item}`, () => {
      const temporary = fs.mkdtempSync(path.join(item, '.preflight-'));
      fs.rmdirSync(temporary);
    });
  }
  for (const item of ['.codex', '.agents', '.github', 'vendor']) {
    if (!fs.existsSync(item)) continue;
    expectDenied(`write ${item}`, () => {
      const temporary = fs.mkdtempSync(path.join(item, '.preflight-'));
      fs.rmdirSync(temporary);
    });
  }
  expectDenied('write rust-toolchain.toml', () => fs.closeSync(fs.openSync('rust-toolchain.toml', 'a')));
  expectDenied('read outside sentinel', () => fs.readFileSync(outsideSentinel));
  expectDenied('write outside sentinel', () => fs.closeSync(fs.openSync(outsideSentinel, 'a')));
  expectDenied('read env sentinel', () => fs.readFileSync(envSentinel));
  expectDenied('read Codex home sentinel', () => fs.readFileSync(codexHomeSentinel));
  expectDenied('write Codex home sentinel', () => fs.closeSync(fs.openSync(codexHomeSentinel, 'a')));
  const child = run(process.execPath, ['-e', 'const fs=require("fs");try{fs.readFileSync(process.argv[1]);process.exit(1)}catch(e){process.exit(["EPERM","EACCES"].includes(e.code)?0:2)}', outsideSentinel], { stdio: 'ignore' });
  checkResult('Child inherits filesystem denial', child);
  const networkChild = run(process.execPath, ['-e', 'const net=require("net");const s=net.connect({host:"1.1.1.1",port:443});s.setTimeout(1500,()=>process.exit(2));s.on("connect",()=>process.exit(1));s.on("error",e=>process.exit(["EPERM","EACCES","ENETUNREACH","EHOSTUNREACH"].includes(e.code)?0:2))'], { stdio: 'ignore', timeout: 3000 });
  checkResult('Child inherits network denial', networkChild);
}

function tcpProbe(label, host, port) {
  return new Promise((resolve) => {
    const socket = net.connect({ host, port });
    const timer = setTimeout(() => { socket.destroy(); failures.push(`${label}: timeout is not a proven denial`); resolve(); }, 1500);
    socket.once('connect', () => { clearTimeout(timer); socket.destroy(); failures.push(`${label}: connected`); resolve(); });
    socket.once('error', (error) => {
      clearTimeout(timer);
      if (!denied.has(error.code)) failures.push(`${label}: inconclusive ${error.code}`);
      resolve();
    });
  });
}

function unixBindProbe() {
  return new Promise((resolve) => {
    const socketPath = path.resolve(`.work/tmp/preflight-${process.pid}.sock`);
    const server = net.createServer();
    server.once('error', (error) => {
      if (!denied.has(error.code)) failures.push(`Unix socket: inconclusive ${error.code}`);
      resolve();
    });
    server.listen(socketPath, () => {
      failures.push('Unix socket: unexpectedly bound');
      server.close(() => { if (fs.existsSync(socketPath)) fs.unlinkSync(socketPath); resolve(); });
    });
  });
}

async function effectiveChecks(options) {
  // This validates the launch configuration. The actual agent tool catalog
  // must also be inspected in the new session before goal work begins.
  checkRemoteProjectConfig();
  checkGitIsolation(options.sourceGitDir);
  checkFilesystem(options);
  await tcpProbe('public TCP', '1.1.1.1', 443);
  await tcpProbe('private TCP', '192.168.255.254', 443);
  await tcpProbe('loopback TCP', '127.0.0.1', 65535);
  await unixBindProbe();
  if (!fs.existsSync('verify')) failures.push('verify missing');
  else checkResult('verify all offline', run('./verify', ['all'], { stdio: 'inherit', timeout: 120000 }));
}

async function main() {
  const options = parseArgs();
  staticChecks();
  if (options.mode === 'surface') checkCodexMcpSurface();
  if (options.mode === 'effective') await effectiveChecks(options);
  if (failures.length) {
    console.error(`Preflight failed for ${profile} (${options.mode})`);
    for (const failure of failures) console.error(`- ${failure}`);
    process.exitCode = 1;
  } else console.log(`Preflight passed for ${profile} (${options.mode})`);
}

main().catch((error) => { console.error(`Preflight failed: ${error.code || error.message}`); process.exitCode = 1; });
