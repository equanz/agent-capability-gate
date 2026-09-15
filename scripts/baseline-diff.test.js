const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');

function git(cwd, args) {
  const result = spawnSync('git', args, {
    cwd, encoding: 'utf8',
    env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null',
      GIT_AUTHOR_NAME: 'Fixture', GIT_AUTHOR_EMAIL: 'fixture@invalid.example',
      GIT_COMMITTER_NAME: 'Fixture', GIT_COMMITTER_EMAIL: 'fixture@invalid.example' },
  });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

test('mechanical diff flags protected content and reports ordinary changes', () => {
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), 'baseline-diff-'));
  try {
    git(repo, ['init', '-q']);
    fs.mkdirSync(path.join(repo, 'docs'));
    fs.mkdirSync(path.join(repo, 'src'));
    fs.writeFileSync(path.join(repo, 'docs/design.md'), 'baseline\n');
    fs.writeFileSync(path.join(repo, 'src/main.rs'), 'fn main() {}\n');
    git(repo, ['add', '.']);
    git(repo, ['commit', '-qm', 'baseline']);
    const baseline = git(repo, ['rev-parse', 'HEAD']);
    fs.writeFileSync(path.join(repo, 'docs/design.md'), 'changed\n');
    fs.writeFileSync(path.join(repo, 'src/main.rs'), 'fn main() { println!("ok"); }\n');
    git(repo, ['add', '.']);
    git(repo, ['commit', '-qm', 'candidate']);
    const candidate = git(repo, ['rev-parse', 'HEAD']);
    const result = spawnSync(process.execPath, [path.join(__dirname, 'baseline-diff.js'),
      '--git-dir', path.join(repo, '.git'), '--baseline', baseline, '--candidate', candidate], { encoding: 'utf8' });
    assert.equal(result.status, 1, result.stderr);
    const report = JSON.parse(result.stdout);
    assert.equal(report.status, 'protected-drift');
    assert.deepEqual(report.changes.map((change) => [change.path, change.protected]),
      [['docs/design.md', true], ['src/main.rs', false]]);
  } finally {
    fs.rmSync(repo, { recursive: true, force: true });
  }
});
