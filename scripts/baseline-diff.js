#!/usr/bin/env node
// Mechanical input to the independent exit judge; not a completion verdict.
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const protectedPrefixes = [
  'docs/design.md', 'docs/implementation-design.md',
  'docs/verification-design.md', 'docs/autonomous-development.md',
  'docs/mvp-goal-contract.md', '.codex', '.codex/', '.agents', '.agents/',
  '.github', '.github/', 'vendor', 'vendor/', 'rust-toolchain.toml',
];

function git(gitDir, args) {
  const result = spawnSync('git', ['--git-dir', gitDir, ...args], {
    encoding: 'buffer',
    env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null', GIT_TERMINAL_PROMPT: '0' },
  });
  if (result.error || result.status !== 0) {
    throw new Error(`Git ${args[0]} failed: ${result.error?.code || result.status}`);
  }
  return result.stdout;
}

function treeEntries(gitDir, revision) {
  const output = git(gitDir, ['ls-tree', '-rz', '--full-tree', revision]);
  const entries = new Map();
  for (const record of output.toString('binary').split('\0')) {
    if (!record) continue;
    const tab = record.indexOf('\t');
    if (tab < 0) throw new Error('Malformed Git tree record');
    const fields = record.slice(0, tab).split(' ');
    if (fields.length !== 3) throw new Error('Malformed Git tree metadata');
    const rawPath = Buffer.from(record.slice(tab + 1), 'binary');
    entries.set(rawPath.toString('hex'), {
      path: rawPath.toString('utf8'), pathHex: rawPath.toString('hex'),
      mode: fields[0], type: fields[1], object: fields[2],
    });
  }
  return entries;
}

function isProtected(filePath) {
  return protectedPrefixes.some((prefix) => prefix.endsWith('/') ? filePath.startsWith(prefix) : filePath === prefix);
}

function parseArgs() {
  const args = process.argv.slice(2);
  if (args.length !== 6 || args[0] !== '--git-dir' || args[2] !== '--baseline' || args[4] !== '--candidate') {
    throw new Error('Usage: baseline-diff.js --git-dir ABS --baseline SHA --candidate SHA');
  }
  if (!path.isAbsolute(args[1]) || !/^[0-9a-f]{40,64}$/.test(args[3]) || !/^[0-9a-f]{40,64}$/.test(args[5])) {
    throw new Error('Git directory must be absolute and revisions must be full SHA values');
  }
  return { gitDir: args[1], baseline: args[3], candidate: args[5] };
}

function main() {
  const { gitDir, baseline, candidate } = parseArgs();
  git(gitDir, ['cat-file', '-e', `${baseline}^{commit}`]);
  git(gitDir, ['cat-file', '-e', `${candidate}^{commit}`]);
  git(gitDir, ['merge-base', '--is-ancestor', baseline, candidate]);
  const before = treeEntries(gitDir, baseline);
  const after = treeEntries(gitDir, candidate);
  const changes = [];
  for (const key of new Set([...before.keys(), ...after.keys()])) {
    const oldEntry = before.get(key);
    const newEntry = after.get(key);
    if (oldEntry && newEntry && oldEntry.mode === newEntry.mode && oldEntry.type === newEntry.type && oldEntry.object === newEntry.object) continue;
    const entry = newEntry || oldEntry;
    changes.push({ path: entry.path, pathHex: entry.pathHex,
      kind: !oldEntry ? 'added' : !newEntry ? 'deleted' : 'changed',
      before: oldEntry ? { mode: oldEntry.mode, type: oldEntry.type, object: oldEntry.object } : null,
      after: newEntry ? { mode: newEntry.mode, type: newEntry.type, object: newEntry.object } : null,
      protected: isProtected(entry.path),
    });
  }
  changes.sort((a, b) => a.pathHex.localeCompare(b.pathHex));
  const protectedChanges = changes.filter((change) => change.protected);
  process.stdout.write(`${JSON.stringify({
    baseline, candidate, status: protectedChanges.length ? 'protected-drift' : 'mechanical-pass',
    changes, requiredNext: ['clean checkout offline verify', 'P/E requirement drift review', 'effective permission preflight'],
  }, null, 2)}\n`);
  if (protectedChanges.length) process.exitCode = 1;
}

try { main(); } catch (error) { console.error(`Baseline diff input error: ${error.message}`); process.exitCode = 2; }
