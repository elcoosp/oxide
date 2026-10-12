// create-oxide — scaffold Oxide starters (spec §19.2).
// Four frameworks × four shapes; templates generated programmatically so each
// emitted project is self-contained (framework flavor + per-shape route modules).
import { mkdir, writeFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';

export const FRAMEWORKS = ['react', 'vue', 'svelte', 'solid'];
export const SHAPES = ['minimal', 'blog', 'dashboard', 'storefront'];

/** @returns {string} oxide.config.ts content per spec §14.1 */
export function configTs(framework, shape) {
  const adapterImport =
    framework === 'react' ? `import react from '@oxide/react'` :
    framework === 'vue' ? `import vue from '@oxide/vue'` :
    framework === 'svelte' ? `import svelte from '@oxide/svelte'` :
    `import solid from '@oxide/solid'`;
  const shapeOpts =
    shape === 'blog' ? `\n  cache: { handler: { type: 'disk' }, l0MaxSize: '512mb' },` :
    shape === 'dashboard' ? `\n  security: { csp: 'strict', csrf: 'origin' },\n  cache: { handler: { type: 'memory' }, l0MaxSize: '128mb' },` :
    shape === 'storefront' ? `\n  images: { formats: ['avif', 'webp'], remotePatterns: [{ host: 'cdn.example.com' }] },\n  cache: { handler: { type: 'disk' }, l0MaxSize: '512mb' },` : '';
  return `${adapterImport}
import { defineConfig } from 'oxide'

export default defineConfig({
  framework: ${framework}(),
  routes: { dir: 'src/routes' },
  server: { port: 3000, compression: ['zstd', 'br'], http3: false },
  render: { default: 'auto' },${shapeOpts}
  build: { target: 'baseline-widely-available', sourcemap: 'hidden' },
  adapter: 'binary',
  modules: [],
})
`;
}

/** Page component per framework flavor (spec §8.1 page.*). */
export function pageComponent(framework, shape) {
  const title = shape === 'minimal' ? 'Home' : shape[0].toUpperCase() + shape.slice(1);
  if (framework === 'vue') {
    return `<script setup>
import { data } from './page.server'
</script>

<template>
  <main>
    <h1>${title}</h1>
    <p>{{ data.message }}</p>
  </main>
</template>
`;
  }
  if (framework === 'svelte') {
    return `<script>
  export let data;
</script>

<main>
  <h1>${title}</h1>
  <p>{data.message}</p>
</main>
`;
  }
  // react + solid (jsx)
  const isSolid = framework === 'solid';
  return `${isSolid ? `import { For } from 'solid-js';\n` : ''}import type { PageProps } from 'oxide'

export default function Page({ data }: PageProps) {
  return (
    <main>
      <h1>${title}</h1>
      <p>{data.message}</p>
    </main>
  )
}
`;
}

/** Framework-neutral server module (spec §9.1). */
export function pageServerTs(shape) {
  const extra =
    shape === 'blog'
      ? `export const config = { render: 'isr', revalidate: 60, tags: ['posts'] }

`
      : shape === 'dashboard'
        ? `export const actions = {
  save: defineAction({
    async run({ input, locals }) {
      await Promise.resolve(input)
      return { ok: true, savedAt: Date.now() }
    },
  }),
}

`
        : shape === 'storefront'
          ? `export const config = { render: 'isr', revalidate: 30, tags: ['products'] }

` : '';
  return `${extra}import { defineLoad, defineMeta, defer } from 'oxide/server'

export const load = defineLoad(async ({ params }) => {
  return {
    message: 'Hello from Oxide (${shape})',
    params,
    // streamed later via the core boundary protocol (spec §7.1)
    details: defer(Promise.resolve({ at: Date.now() })),
  }
})

export const meta = defineMeta(({ data }) => ({ title: data.message }))
`;
}

export function layoutComponent(framework) {
  if (framework === 'vue') {
    return `<script setup></script>

<template>
  <html lang="en">
    <slot />
  </html>
</template>
`;
  }
  if (framework === 'svelte') {
    return `<slot />
`;
  }
  return `export default function Layout({ children }) {
  return <html lang="en">{children}</html>
}
`;
}

export function packageJson(framework) {
  const scripts = {
    dev: 'oxide dev',
    build: 'oxide build',
    start: 'oxide start',
  };
  return JSON.stringify(
    {
      name: `oxide-${framework}-app`,
      private: true,
      type: 'module',
      scripts,
      dependencies: { oxide: '0.1.0', [`@oxide/${framework}`]: '0.1.0' },
    },
    null,
    2,
  ) + '\n';
}

export function readmeMd(framework, shape) {
  return `# Oxide ${shape} starter (${framework})

Scaffolded by \`create-oxide\` (spec §19.2).

\`\`\`sh
npm install
npm run dev    # http://localhost:3000
npm run build  # production build + manifests
npm run start  # serve the binary target
\`\`\`

Conventions live in \`src/routes\` — see the Oxide spec §8.
`;
}

/** Scaffold one project. Throws on non-empty target dir. */
export async function scaffold(template, framework, dir) {
  if (!FRAMEWORKS.includes(framework)) {
    throw new Error(`unknown framework "${framework}" (expected one of ${FRAMEWORKS.join(', ')})`);
  }
  if (!SHAPES.includes(template)) {
    throw new Error(`unknown template "${template}" (expected one of ${SHAPES.join(', ')})`);
  }
  let existing = [];
  try {
    existing = await readdir(dir);
  } catch {
    /* dir may not exist yet */
  }
  if (existing.length > 0) {
    const err = new Error(`refusing to scaffold into non-empty directory: ${dir}`);
    err.code = 'ENOEMPTY';
    throw err;
  }
  const routes = join(dir, 'src', 'routes');
  await mkdir(routes, { recursive: true });
  const ext = framework === 'vue' ? 'vue' : framework === 'svelte' ? 'svelte' : 'tsx';
  const writes = [
    [join(dir, 'package.json'), packageJson(framework)],
    [join(dir, 'oxide.config.ts'), configTs(framework, template)],
    [join(dir, 'README.md'), readmeMd(framework, template)],
    [join(dir, 'src', 'routes', 'layout.' + ext), layoutComponent(framework)],
    [join(dir, 'src', 'routes', 'page.' + ext), pageComponent(framework, template)],
    [join(dir, 'src', 'routes', 'page.server.ts'), pageServerTs(template)],
  ];
  for (const [path, content] of writes) {
    await mkdir(join(path, '..'), { recursive: true });
    await writeFile(path, content, 'utf8');
  }
  return writes.map(([p]) => p);
}

/** CLI entry (runs under Node 24 type stripping). */
export async function main(argv = process.argv.slice(2)) {
  const [template = 'minimal', framework = 'react', dir = '.'] = argv;
  try {
    const files = await scaffold(template, framework, dir);
    console.log(`✔ scaffolded oxide ${template} (${framework}) into ${dir}`);
    for (const f of files) console.log(`  ${f}`);
    console.log(`\nnext: npm install && npm run dev`);
    return 0;
  } catch (err) {
    console.error(`✖ ${err.message}`);
    return err.code === 'ENOEMPTY' ? 3 : 2;
  }
}

if (process.argv[1] && process.argv[1].endsWith('cli.ts')) {
  main().then((code) => process.exit(code));
}
