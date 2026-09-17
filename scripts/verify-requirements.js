#!/usr/bin/env node

const fs = require('node:fs');
const path = require('node:path');

const manifest = path.join(__dirname, '..', 'tests', 'requirements.toml');
if (!fs.existsSync(manifest)) {
  console.error(`requirements manifest missing: ${manifest}`);
  process.exit(1);
}

const text = fs.readFileSync(manifest, 'utf8');
const required = [
  ...Array.from({ length: 20 }, (_, i) => `P${String(i + 1).padStart(2, '0')}`),
  ...Array.from({ length: 17 }, (_, i) => `E${String(i + 1).padStart(2, '0')}`),
];

function parseString(value, field, line) {
  try {
    const parsed = JSON.parse(value);
    if (typeof parsed !== 'string' || parsed.length === 0) throw new Error('not a non-empty string');
    return parsed;
  } catch (error) {
    throw new Error(`${field} must be a non-empty JSON/TOML string at line ${line}: ${error.message}`);
  }
}

function parseStringArray(value, field, line) {
  try {
    const parsed = JSON.parse(value);
    if (!Array.isArray(parsed) || parsed.length === 0 || parsed.some((item) => typeof item !== 'string' || item.length === 0)) {
      throw new Error('not a non-empty string array');
    }
    return parsed;
  } catch (error) {
    throw new Error(`${field} must be a non-empty string array at line ${line}: ${error.message}`);
  }
}

function parseManifest(source) {
  const entries = [];
  let current = null;
  source.split(/\r?\n/).forEach((raw, index) => {
    const line = index + 1;
    const value = raw.trim();
    if (value.startsWith('#')) return;
    const fieldValue = value.replace(/\s+#.*$/, '').trim();
    if (!fieldValue) return;
    if (fieldValue === '[[requirement]]') {
      current = { line };
      entries.push(current);
      return;
    }
    if (!current) throw new Error(`field outside [[requirement]] at line ${line}`);
    const match = fieldValue.match(/^([a-z_]+)\s*=\s*(.+)$/);
    if (!match) throw new Error(`invalid manifest line ${line}`);
    const [, field, rawValue] = match;
    if (Object.prototype.hasOwnProperty.call(current, field)) {
      throw new Error(`duplicate ${field} at line ${line}`);
    }
    if (field === 'id' || field === 'kind' || field === 'status' || field === 'criterion') {
      current[field] = parseString(rawValue, field, line);
    } else if (field === 'references') {
      current[field] = parseStringArray(rawValue, field, line);
    } else {
      throw new Error(`unknown field ${field} at line ${line}`);
    }
  });
  return entries;
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function validateReference(reference, entry) {
  const separator = reference.indexOf('::');
  const relative = separator === -1 ? reference : reference.slice(0, separator);
  const selector = separator === -1 ? null : reference.slice(separator + 2);
  if (path.isAbsolute(relative) || relative.includes('\\') || relative.split('/').includes('..')) {
    throw new Error(`${entry.id} has non-repository reference ${reference}`);
  }
  const resolved = path.resolve(__dirname, '..', relative);
  const root = path.resolve(__dirname, '..');
  if (resolved !== root && !resolved.startsWith(`${root}${path.sep}`) || !fs.existsSync(resolved) || !fs.statSync(resolved).isFile()) {
    throw new Error(`${entry.id} references missing file ${reference}`);
  }
  if (selector !== null) {
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(selector)) throw new Error(`${entry.id} has invalid test selector ${reference}`);
    const source = fs.readFileSync(resolved, 'utf8');
    const functionPattern = new RegExp(`(?:pub\\s+)?(?:async\\s+)?fn\\s+${escapeRegExp(selector)}\\s*\\(`);
    if (!functionPattern.test(source)) throw new Error(`${entry.id} references missing test/source selector ${reference}`);
    const testPattern = new RegExp(`(?:#\\[[^\\]]*\\btest\\b[^\\]]*\\]\\s*)+(?:pub\\s+)?(?:async\\s+)?fn\\s+${escapeRegExp(selector)}\\s*\\(`);
    if (!testPattern.test(source)) throw new Error(`${entry.id} references non-test selector ${reference}`);
  }
}

let entries;
try {
  entries = parseManifest(text);
  const seen = new Set();
  for (const entry of entries) {
    for (const field of ['id', 'kind', 'status', 'references', 'criterion']) {
      if (!Object.prototype.hasOwnProperty.call(entry, field)) throw new Error(`${entry.id || `entry at line ${entry.line}`} missing ${field}`);
    }
    if (!required.includes(entry.id)) throw new Error(`unknown requirement id ${entry.id}`);
    if (seen.has(entry.id)) throw new Error(`duplicate requirement id ${entry.id}`);
    seen.add(entry.id);
    if (!['test', 'review'].includes(entry.kind)) throw new Error(`${entry.id} has invalid kind ${entry.kind}`);
    if (!['covered', 'review'].includes(entry.status)) throw new Error(`${entry.id} has invalid status ${entry.status}`);
    if (entry.status === 'covered' && entry.kind !== 'test') {
      throw new Error(`${entry.id} covered entry must be kind=test; use kind=review until the criterion is actually automated`);
    }
    if (entry.kind === 'test' && entry.status !== 'covered') {
      throw new Error(`${entry.id} kind=test must be status=covered; use kind=review for an unfulfilled criterion`);
    }
    if (entry.kind === 'test' && !entry.references.some((reference) => reference.includes('::'))) {
      throw new Error(`${entry.id} test entry must reference a test selector`);
    }
    if (entry.kind === 'review' && entry.status !== 'review') {
      throw new Error(`${entry.id} review entry must remain status=review`);
    }
    entry.references.forEach((reference) => validateReference(reference, entry));
  }
  const missing = required.filter((id) => !seen.has(id));
  if (missing.length) throw new Error(`requirements missing: ${missing.join(', ')}`);
  if (entries.length !== required.length) throw new Error(`expected exactly ${required.length} requirements, got ${entries.length}`);
} catch (error) {
  console.error(`requirements invalid: ${error.message}`);
  process.exit(1);
}
const covered = entries.filter((entry) => entry.status === 'covered').length;
const review = entries.length - covered;
console.log(`requirements present: ${entries.length}; automated: ${covered}; review: ${review}`);
