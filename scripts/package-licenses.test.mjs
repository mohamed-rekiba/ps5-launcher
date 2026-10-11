import assert from 'node:assert/strict';
import { test } from 'node:test';
import * as fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { applicableUpstreamFiles, localDocuments, normalizeExpression, safeRelative, selectPackages, selectTargetPackages, upstreamDocuments } from './package-licenses.mjs';

test('vendored oozextract packages MIT terms and its modification notice without upstream fallback', async () => {
  const root = fileURLToPath(new URL('../third_party/oozextract/', import.meta.url));
  const documents = await localDocuments(root);
  const license = documents.find(d => /^(?:licen[cs]e|copying)/i.test(path.posix.basename(d.relative)));
  assert.ok(license, 'vendored oozextract must include local license text');
  assert.match(license.bytes.toString('utf8'), /Permission is hereby granted, free of charge/);
  assert.match(license.bytes.toString('utf8'), /THE SOFTWARE IS PROVIDED "AS IS"/);
  assert.ok(documents.some(d => d.relative === 'NOTICE.md'));
});

test('retain SPDX AND; normalize only legacy Cargo alternatives', () => {
  assert.equal(normalizeExpression('MIT/Apache-2.0'), 'MIT OR Apache-2.0');
  assert.equal(normalizeExpression('ISC AND (Apache-2.0 OR ISC)'), 'ISC AND (Apache-2.0 OR ISC)');
  assert.throws(() => normalizeExpression(null), /no declared license/);
});

test('active tree excludes dev-only/optional metadata and includes duplicate versions', () => {
  const metadata = { resolve: { root: 'app' }, packages: [
    { id: 'app', name: 'app', version: '1.0.0' },
    { id: 'a1', name: 'a', version: '1.0.0' }, { id: 'a2', name: 'a', version: '2.0.0' },
    { id: 'unused', name: 'unused', version: '1.0.0' },
  ] };
  assert.deepEqual(selectPackages(metadata, 'app v1.0.0 (/app)\na v1.0.0\na v2.0.0\na v1.0.0 (*)\n').map(p => p.id), ['a1', 'a2']);
  assert.throws(() => selectPackages(metadata, 'missing v1.0.0'), /missing/);
});

test('paths cannot escape or be absolute', () => {
  for (const p of ['../LICENSE', '/etc/passwd', 'a/../b', 'a\\b', '', 'a//b']) assert.throws(() => safeRelative(p));
  assert.equal(safeRelative('vendor/NOTICE'), 'vendor/NOTICE');
});

test('upstream retains applicable notices, but not unrelated package notices', () => {
  const tree = ['LICENSE', 'NOTICE', 'crates/NOTICE', 'crates/a/LICENSE', 'crates/a/vendor/NOTICE', 'crates/b/NOTICE']
    .map(p => ({ type: 'blob', path: p, mode: '100644' }));
  assert.deepEqual(applicableUpstreamFiles(tree, 'crates/a', 'Apache-2.0', 'owner/repo').map(e => e.path), tree.slice(0, 5).map(e => e.path));
});

test('Slint includes royalty-free text, never silently substitutes commercial/GPL', () => {
  const tree = ['LICENSE.md', 'LICENSES/LicenseRef-Slint-Royalty-free-2.0.md', 'api/rs/slint/LICENSES/GPL-3.0-only.txt']
    .map(p => ({ type: 'blob', path: p, mode: '100644' }));
  assert.deepEqual(applicableUpstreamFiles(tree, 'api/rs/slint', 'GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0', 'slint-ui/slint')
    .map(e => e.path), tree.slice(0, 2).map(e => e.path));
  assert.throws(() => applicableUpstreamFiles([], '', 'LicenseRef-Slint-Royalty-free-2.0', 'slint-ui/slint'), /Missing upstream/);
});

test('local traversal preserves nested native notices byte-for-byte and rejects escaping symlinks', async () => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'ps5-license-test-'));
  try {
    const root = path.join(dir, 'crate');
    await fs.mkdir(path.join(root, 'vendor/licenses'), { recursive: true });
    const bytes = Buffer.from('Copyright upstream\r\nOriginal notice\r\n');
    await fs.writeFile(path.join(root, 'vendor/NOTICE'), bytes);
    await fs.writeFile(path.join(root, 'vendor/licenses/terms.txt'), 'license terms');
    await fs.writeFile(path.join(root, 'implementation.rs'), 'not a licensing document');
    const documents = await localDocuments(root);
    assert.equal(documents.length, 2);
    assert.deepEqual(documents.find(d => d.relative === 'vendor/NOTICE').bytes, bytes);
    await assert.rejects(localDocuments(root, 'MISSING.txt'), /absent/);
    await fs.writeFile(path.join(dir, 'outside.txt'), 'outside');
    await fs.symlink('../outside.txt', path.join(root, 'LICENSE'));
    await assert.rejects(localDocuments(root), /escapes package/);
  } finally { await fs.rm(dir, { recursive: true, force: true }); }
});

async function withUpstreamFixture(run) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'ps5-upstream-test-'));
  const originalFetch = globalThis.fetch;
  const originalToken = process.env.GITHUB_TOKEN;
  process.env.GITHUB_TOKEN = 'fixture-github-token';
  try {
    const commit = '4fc083f1c6d6784577e38b0ee8dbd344481e2fd2';
    await fs.writeFile(path.join(root, '.cargo_vcs_info.json'), JSON.stringify({
      git: { sha1: commit }, path_in_vcs: 'crates/block2',
    }));
    await run({ root, commit });
  } finally {
    globalThis.fetch = originalFetch;
    if (originalToken === undefined) delete process.env.GITHUB_TOKEN;
    else process.env.GITHUB_TOKEN = originalToken;
    await fs.rm(root, { recursive: true, force: true });
  }
}

test('macOS block2 retrieves the root license from its published objc2 commit', async () => {
  await withUpstreamFixture(async ({ root, commit }) => {
    const source = `https://raw.githubusercontent.com/madsmtm/objc2/${commit}/LICENSE.txt`;
    const bytes = Buffer.from('MIT License\r\nCopyright (c) Steven Sheldon\r\n');
    globalThis.fetch = async (url, options) => {
      assert.equal(options.redirect, 'error');
      if (url === `https://api.github.com/repos/madsmtm/objc2/git/trees/${commit}?recursive=1`) {
        return Response.json({ truncated: false, tree: [
          { type: 'blob', mode: '100644', path: 'LICENSE.txt' },
          { type: 'blob', mode: '100644', path: 'crates/other/NOTICE' },
        ] });
      }
      assert.equal(url, source);
      return new Response(bytes);
    };
    const result = await upstreamDocuments({ name: 'block2', version: '0.5.1',
      license: 'MIT', repository: 'https://github.com/madsmtm/objc2',
      manifest_path: path.join(root, 'Cargo.toml') }, true, new Map());
    assert.deepEqual(result.documents, [{ relative: 'upstream/LICENSE.txt', bytes, source }]);
    assert.deepEqual(result.upstreamNoticeSearch.noticePaths, []);
  });
});

test('reviewed dispatch publication preserves its MIT declaration over HTTPS when no LICENSE exists', async () => {
  await withUpstreamFixture(async ({ root }) => {
    const commit = '82d6c7a5b75dc0c71c3f46f87bb6c16a476f7748';
    await fs.writeFile(path.join(root, '.cargo_vcs_info.json'), JSON.stringify({ git: { sha1: commit } }));
    const manifest = Buffer.from('[package]\nlicense = "MIT"\nauthors = ["Steven Sheldon"]\n');
    globalThis.fetch = async url => {
      assert.ok(url.startsWith('https://'));
      if (url.startsWith('https://api.github.com/repos/SSheldon/rust-dispatch/')) {
        return Response.json({ tree: [{ type: 'blob', mode: '100644', path: 'Cargo.toml' }] });
      }
      assert.equal(url, `https://raw.githubusercontent.com/SSheldon/rust-dispatch/${commit}/Cargo.toml`);
      return new Response(manifest);
    };
    const pkg = { name: 'dispatch', version: '0.2.0', license: 'MIT',
      repository: 'http://github.com/SSheldon/rust-dispatch', manifest_path: path.join(root, 'Cargo.toml') };
    const result = await upstreamDocuments(pkg, true, new Map());
    assert.equal(result.metadataOnly, true);
    assert.deepEqual(result.documents[0].bytes, manifest);
    assert.equal(result.documents[0].relative, 'upstream/Cargo.toml');
    await assert.rejects(upstreamDocuments({ ...pkg, version: '0.2.1' }, true, new Map()), /no upstream LICENSE text/);
    await assert.rejects(upstreamDocuments({ ...pkg, repository: 'http://github.com/unreviewed/repo' }, true, new Map()),
      /no reviewed upstream fallback/);
  });
});

test('gilrs-core retains GitLab ancestor notices across tree pages without sending GitHub credentials', async () => {
  await withUpstreamFixture(async ({ root, commit }) => {
    await fs.writeFile(path.join(root, '.cargo_vcs_info.json'), JSON.stringify({ git: { sha1: commit }, path_in_vcs: 'gilrs-core' }));
    const base = 'https://gitlab.com/api/v4/projects/gilrs-project%2Fgilrs/repository';
    const bytes = Buffer.from('MIT License\n');
    globalThis.fetch = async (url, options) => {
      assert.equal(options.headers.Authorization, undefined);
      if (url === `${base}/tree?ref=${commit}&recursive=true&per_page=100&page=1`) {
        return Response.json([{ type: 'blob', mode: '100644', path: 'LICENSE-MIT' }], { headers: { 'x-next-page': '2' } });
      }
      if (url === `${base}/tree?ref=${commit}&recursive=true&per_page=100&page=2`) {
        return Response.json([{ type: 'blob', mode: '100644', path: 'gilrs-core/NOTICE' },
          { type: 'blob', mode: '100644', path: 'gilrs/NOTICE' }]);
      }
      assert.ok([`${base}/files/LICENSE-MIT/raw?ref=${commit}`, `${base}/files/gilrs-core%2FNOTICE/raw?ref=${commit}`].includes(url));
      return new Response(bytes);
    };
    const result = await upstreamDocuments({ name: 'gilrs-core', version: '0.6.8', license: 'Apache-2.0/MIT',
      repository: 'https://gitlab.com/gilrs-project/gilrs', manifest_path: path.join(root, 'Cargo.toml') }, true, new Map());
    assert.deepEqual(result.documents.map(d => d.relative), ['upstream/gilrs-core/NOTICE', 'upstream/LICENSE-MIT']);
    assert.deepEqual(result.upstreamNoticeSearch.noticePaths, ['gilrs-core/NOTICE']);
  });
});

test('universal bundle includes Intel-only crates and retains shared crates once', () => {
  const app = { id: 'app', name: 'app', version: '1.0.0' };
  const common = { id: 'common', name: 'common', version: '1.0.0' };
  const arm = { id: 'arm', name: 'arm', version: '1.0.0' };
  const intel = { id: 'intel', name: 'intel', version: '1.0.0' };
  const metadata = packages => ({ resolve: { root: 'app' }, packages: [app, ...packages] });
  const selected = selectTargetPackages([
    { metadata: metadata([common, arm]), tree: 'app v1.0.0\ncommon v1.0.0\narm v1.0.0' },
    { metadata: metadata([common, intel]), tree: 'app v1.0.0\ncommon v1.0.0\nintel v1.0.0' },
  ]);
  assert.deepEqual(selected.map(p => p.id), ['arm', 'common', 'intel']);
});
