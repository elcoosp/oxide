// create-oxide tests (spec §19.2): scaffolds every framework×shape combo,
// refuses non-empty dirs, emitted config matches §14.1 keys.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, readdir, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  FRAMEWORKS,
  SHAPES,
  scaffold,
  configTs,
  pageServerTs,
  main,
} from '../src/cli.ts';

test('scaffolds every framework × shape combo with required files', async () => {
  const base = await mkdtemp(join(tmpdir(), 'oxide-create-'));
  try {
    for (const fw of FRAMEWORKS) {
      for (const shape of SHAPES) {
        const dir = join(base, `${fw}-${shape}`);
        const files = await scaffold(shape, fw, dir);
        // SPEC-19.2: shape-specific extras — minimal=6 (the base set),
        // blog=8 (blog/[slug]/page + page.server), dashboard=8, storefront=8.
        const expectedFiles =
          shape === 'minimal' ? 6 : 8;
        assert.equal(
          files.length,
          expectedFiles,
          `${fw}/${shape}: ${expectedFiles} files`,
        );
        const pkg = JSON.parse(await readFile(join(dir, 'package.json'), 'utf8'));
        assert.equal(pkg.scripts.dev, 'oxide dev');
        assert.equal(pkg.scripts.build, 'oxide build');
        assert.equal(pkg.scripts.start, 'oxide start');
        assert.ok(pkg.dependencies[`@oxide/${fw}`]);
        const cfg = await readFile(join(dir, 'oxide.config.ts'), 'utf8');
        for (const key of ['framework:', 'routes:', 'server:', 'render:', 'adapter:', 'build:']) {
          assert.ok(cfg.includes(key), `${fw}/${shape} config has ${key}`);
        }
        assert.ok(cfg.includes(`render: { default: 'auto' }`));
        const server = await readFile(join(dir, 'src', 'routes', 'page.server.ts'), 'utf8');
        assert.ok(server.includes('defineLoad'));
        assert.ok(server.includes('defer('), 'deferred data per §7.1');
        if (shape === 'dashboard') assert.ok(server.includes('defineAction'));
        if (shape === 'blog' || shape === 'storefront') {
          assert.ok(server.includes("render: 'isr'"));
          assert.ok(server.includes('tags: ['));
        }
        const ext = fw === 'vue' ? 'vue' : fw === 'svelte' ? 'svelte' : 'tsx';
        await readFile(join(dir, 'src', 'routes', `layout.${ext}`), 'utf8');
        await readFile(join(dir, 'src', 'routes', `page.${ext}`), 'utf8');
      }
    }
  } finally {
    await rm(base, { recursive: true, force: true });
  }
});

test('shape-specific extra routes are real files with shape-specific loaders', async () => {
  const base = await mkdtemp(join(tmpdir(), 'oxide-shape-extras-'));
  try {
    // blog → blog/[slug] with ISR + generateParams
    const blogDir = join(base, 'blog');
    await scaffold('blog', 'react', blogDir);
    const blogServer = await readFile(
      join(blogDir, 'src', 'routes', 'blog', '[slug]', 'page.server.ts'),
      'utf8',
    );
    assert.ok(blogServer.includes('generateParams'), 'blog has generateParams');
    assert.ok(blogServer.includes("render: 'isr'"), 'blog is ISR');
    const blogPage = await readFile(
      join(blogDir, 'src', 'routes', 'blog', '[slug]', 'page.tsx'),
      'utf8',
    );
    assert.ok(blogPage.includes('Post'), 'blog page exists and is titled Post');

    // dashboard → dashboard/ with actions
    const dashDir = join(base, 'dashboard');
    await scaffold('dashboard', 'react', dashDir);
    const dashServer = await readFile(
      join(dashDir, 'src', 'routes', 'dashboard', 'page.server.ts'),
      'utf8',
    );
    assert.ok(dashServer.includes('defineAction'), 'dashboard has actions');
    assert.ok(dashServer.includes('locals.session'), 'dashboard reads session');

    // storefront → products/[id] with ISR + generateParams
    const storeDir = join(base, 'storefront');
    await scaffold('storefront', 'react', storeDir);
    const storeServer = await readFile(
      join(storeDir, 'src', 'routes', 'products', '[id]', 'page.server.ts'),
      'utf8',
    );
    assert.ok(storeServer.includes('generateParams'), 'storefront has generateParams');
    assert.ok(storeServer.includes("tags: ['products']"), 'storefront is tagged');

    // minimal → no extras
    const minDir = join(base, 'minimal');
    await scaffold('minimal', 'react', minDir);
    const minEntries = await readdir(join(minDir, 'src', 'routes'));
    assert.equal(minEntries.length, 3, 'minimal has 3 route entries (layout, page, page.server)');
  } finally {
    await rm(base, { recursive: true, force: true });
  }
});


test('refuses to scaffold into a non-empty directory (exit 3)', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'oxide-create-occupied-'));
  try {
    await writeFile(join(dir, 'keep.txt'), 'x');
    await assert.rejects(() => scaffold('minimal', 'react', dir), /non-empty/);
    const code = await main(['minimal', 'react', dir]);
    assert.equal(code, 3);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('unknown framework/template fail fast with helpful message', async () => {
  await assert.rejects(() => scaffold('minimal', 'angular', '/tmp/never-oxide'), /unknown framework/);
  await assert.rejects(() => scaffold('hackernews', 'react', '/tmp/never-oxide'), /unknown template/);
});

test('config helpers are deterministic and per-framework', () => {
  const react = configTs('react', 'minimal');
  const vue = configTs('vue', 'minimal');
  assert.ok(react.includes("@oxide/react") && react.includes('react()'));
  assert.ok(vue.includes("@oxide/vue") && vue.includes('vue()'));
  assert.equal(react, configTs('react', 'minimal'));
  assert.ok(configTs('react', 'storefront').includes("images: { formats: ['avif', 'webp']"));
  assert.ok(configTs('react', 'dashboard').includes("security: { csp: 'strict'"));
  assert.ok(pageServerTs('blog').includes('revalidate: 60'));
});

test('cli main prints files and returns 0 on success', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'oxide-create-main-'));
  try {
    const code = await main(['minimal', 'solid', dir]);
    assert.equal(code, 0);
    const entries = await readdir(dir);
    assert.ok(entries.includes('oxide.config.ts'));
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
