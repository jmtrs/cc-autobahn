#!/usr/bin/env node
// One-command release (D35): bumps the version in every file that carries it
// (npm manifests, Tauri config/Cargo files, and AppStream metadata),
// runs the backend tests, commits, tags `vX.Y.Z` and pushes. The tag triggers
// .github/workflows/release.yml, which re-runs the tests, builds the unsigned
// universal dmg into a draft; Linux CI validates and attaches its bundles,
// publishes the complete release, then updates the Homebrew cask.
//
// Usage: node scripts/release.mjs <patch|minor|major|X.Y.Z>

import { readFileSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';

const SEMVER = /^\d+\.\d+\.\d+$/;

const fail = (msg) => {
  console.error(`release: ${msg}`);
  process.exit(1);
};
const out = (cmd, args) => execFileSync(cmd, args, { encoding: 'utf8' }).trim();
const run = (cmd, args) => execFileSync(cmd, args, { stdio: 'inherit' });

const arg = process.argv[2];
if (!arg) fail('usage: node scripts/release.mjs <patch|minor|major|X.Y.Z>');

// --- preconditions -----------------------------------------------------------
if (out('git', ['status', '--porcelain'])) fail('working tree is dirty — commit or stash first');
if (out('git', ['branch', '--show-current']) !== 'main') fail('releases are cut from main');

// --- resolve new version ------------------------------------------------------
const current = JSON.parse(readFileSync('package.json', 'utf8')).version;
const bump = (v, kind) => {
  const [x, y, z] = v.split('.').map(Number);
  if (kind === 'major') return `${x + 1}.0.0`;
  if (kind === 'minor') return `${x}.${y + 1}.0`;
  return `${x}.${y}.${z + 1}`;
};

let next;
if (SEMVER.test(arg)) next = arg;
else if (['patch', 'minor', 'major'].includes(arg)) next = bump(current, arg);
else fail(`bad version/bump: ${arg}`);

if (out('git', ['tag', '-l', `v${next}`])) fail(`tag v${next} already exists locally`);
if (out('git', ['ls-remote', '--tags', 'origin', `v${next}`])) fail(`tag v${next} already exists on origin`);

// --- gate: backend tests -------------------------------------------------------
console.log(`release: ${current} → ${next} — running backend tests first…`);
run('cargo', ['test', '--manifest-path', 'src-tauri/Cargo.toml', '--quiet']);

// --- bump every published version source ---------------------------------------
const releaseDate = new Date().toISOString().slice(0, 10);
const edits = [
  ['package.json', /"version": "[^"]+"/, `"version": "${next}"`],
  [
    'package-lock.json',
    /("name": "cc-autobahn",\n\s+"version": ")[^"]+"/g,
    `$1${next}"`,
  ],
  ['src-tauri/tauri.conf.json', /"version": "[^"]+"/, `"version": "${next}"`],
  ['src-tauri/Cargo.toml', /^version = "[^"]+"/m, `version = "${next}"`],
  ['src-tauri/Cargo.lock', /(name = "cc-autobahn"\nversion = ")[^"]+"/, `$1${next}"`],
  [
    'src-tauri/templates/com.jmtrs.cc-autobahn.metainfo.xml',
    /  <releases>\n/,
    `  <releases>\n    <release version="${next}" date="${releaseDate}">\n      <description>\n        <p>See the ${next} release notes for changes.</p>\n      </description>\n    </release>\n`,
  ],
];
for (const [file, re, replacement] of edits) {
  const before = readFileSync(file, 'utf8');
  const after = before.replace(re, replacement);
  if (after === before) fail(`version pattern not found in ${file}`);
  writeFileSync(file, after);
}

// --- commit, tag, push -----------------------------------------------------------
run('git', ['add', ...edits.map(([file]) => file)]);
run('git', ['commit', '-m', `chore(release): v${next}`]);
run('git', ['tag', `v${next}`]);
run('git', ['push', 'origin', 'main']);
run('git', ['push', 'origin', `v${next}`]);

console.log(`\nrelease: v${next} pushed — CI tests, builds, publishes the release and bumps the Homebrew cask.`);
console.log('release: watch with: gh run list --workflow=release.yml --limit=1');
