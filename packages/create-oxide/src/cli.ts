// create-oxide — scaffold Oxide starters (spec §19.2).
// Four frameworks × four shapes; templates generated programmatically so each
// emitted project is self-contained (framework flavor + per-shape route modules).
import { mkdir, writeFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';

export type Framework = 'react' | 'vue' | 'svelte' | 'solid';
export type Shape = 'minimal' | 'blog' | 'dashboard' | 'storefront';
type PageVariant = Shape | 'blog-post' | 'dashboard-home' | 'product';

export const FRAMEWORKS: readonly Framework[] = ['react', 'vue', 'svelte', 'solid'];
export const SHAPES: readonly Shape[] = ['minimal', 'blog', 'dashboard', 'storefront'];

/** Error thrown when the target directory already has content. */
class NonEmptyDirError extends Error {
  readonly code = 'ENOEMPTY';
  constructor(dir: string) {
    super(`refusing to scaffold into non-empty directory: ${dir}`);
    this.name = 'NonEmptyDirError';
  }
}

function isFramework(s: string): s is Framework {
  return (FRAMEWORKS as readonly string[]).includes(s);
}

function isShape(s: string): s is Shape {
  return (SHAPES as readonly string[]).includes(s);
}

/** oxide.config.ts content per spec §14.1. */
export function configTs(framework: Framework, shape: Shape): string {
  const adapterImport =
    framework === 'react' ? `import react from '@oxide-fw/react'` :
    framework === 'vue' ? `import vue from '@oxide-fw/vue'` :
    framework === 'svelte' ? `import svelte from '@oxide-fw/svelte'` :
    `import solid from '@oxide-fw/solid'`;
  const shapeOpts =
    shape === 'blog' ? `\n  cache: { handler: { type: 'disk' }, l0: { maxSize: '512mb' } },` :
    shape === 'dashboard' ? `\n  security: { csp: 'strict', csrf: 'origin' },\n  cache: { handler: { type: 'memory' }, l0: { maxSize: '128mb' } },` :
    shape === 'storefront' ? `\n  images: { formats: ['avif', 'webp'], remotePatterns: [{ host: 'cdn.example.com' }] },\n  cache: { handler: { type: 'disk' }, l0: { maxSize: '512mb' } },` : '';
  return `${adapterImport}
import { defineConfig } from '@oxide-fw/core'

export default defineConfig({
  framework: ${framework}({ ssr: 'real' }),
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
export function pageComponent(framework: Framework, shape: PageVariant): string {
  const titles: Record<PageVariant, string> = {
    minimal: 'Home',
    blog: 'Blog',
    dashboard: 'Dashboard',
    storefront: 'Storefront',
    'blog-post': 'Post',
    'dashboard-home': 'Dashboard',
    product: 'Product',
  };
  const title = titles[shape];
  if (framework === 'vue') {
    return `<script setup>
import { computed } from 'vue'

const props = defineProps<{ data: { message: string } }>()
</script>

<template>
  <main>
    <h1>${title}</h1>
    <p>{{ props.data.message }}</p>
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
  const isSolid = framework === 'solid';
  return `${isSolid ? `import { For } from 'solid-js';\n` : ''}import type { PageProps } from '@oxide-fw/core'

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
export function pageServerTs(shape: Shape): string {
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
  return `${extra}import { defineLoad, defineAction, defineMeta, defer } from '@oxide-fw/core/server'

export const load = defineLoad(async ({ params }) => {
  return {
    message: 'Hello from Oxide (${shape})',
    params,
    details: defer(Promise.resolve({ at: Date.now() })),
  }
})

export const meta = defineMeta(({ data }) => ({ title: data.message }))
`;
}

export function layoutComponent(framework: Framework): string {
  if (framework === 'vue') {
    return `<script setup></script>

<template>
  <slot />
</template>
`;
  }
  if (framework === 'svelte') {
    return `<slot />
`;
  }
  return `export default function Layout({ children }) {
  return <>{children}</>
}
`;
}

export function packageJson(framework: Framework): string {
  const scripts = {
    dev: 'oxide dev',
    build: 'oxide build',
    start: 'oxide start',
  };
  const inWorkspace = (process.env['PWD'] ?? '').includes('packages/');
  const v = inWorkspace ? 'workspace:*' : '0.2.0';
  const deps: Record<string, string> = {
    '@oxide-fw/core': v,
    [`@oxide-fw/${framework}`]: v,
  };
  return JSON.stringify(
    {
      name: `oxide-${framework}-app`,
      private: true,
      type: 'module',
      scripts,
      dependencies: deps,
    },
    null,
    2,
  ) + '\n';
}

export function readmeMd(framework: Framework, shape: Shape): string {
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

type Write = readonly [string, string];

function blogExtras(framework: Framework, ext: string): readonly Write[] {
  return [
    [
      join('src', 'routes', 'blog', '[slug]', 'page.' + ext),
      pageComponent(framework, 'blog-post'),
    ],
    [
      join('src', 'routes', 'blog', '[slug]', 'page.server.ts'),
      `import { defineLoad } from '@oxide-fw/core/server'

export const config = { render: 'isr', revalidate: 60, tags: ['posts'] }

export const load = defineLoad(async ({ params }) => {
  const slug = String(params.slug ?? 'welcome')
  return { title: 'Post: ' + slug, body: 'Content for ' + slug, tags: ['posts'] }
})

export async function generateParams() {
  return [{ slug: 'welcome' }, { slug: 'hello-world' }]
}
`,
    ],
  ];
}

function dashboardExtras(framework: Framework, ext: string): readonly Write[] {
  return [
    [
      join('src', 'routes', 'dashboard', 'page.' + ext),
      pageComponent(framework, 'dashboard-home'),
    ],
    [
      join('src', 'routes', 'dashboard', 'page.server.ts'),
      `import { defineLoad, defineAction } from '@oxide-fw/core/server'

export const load = defineLoad(async ({ locals }) => {
  const user = (locals && locals.session && locals.session.user) || 'guest'
  return { greeting: 'Hello, ' + user, stats: { visits: 3 } }
})

export const actions = {
  save: defineAction({
    async run({ input }) {
      await Promise.resolve(input)
      return { ok: true, savedAt: Date.now() }
    },
  }),
}
`,
    ],
  ];
}

function storefrontExtras(framework: Framework, ext: string): readonly Write[] {
  return [
    [
      join('src', 'routes', 'products', '[id]', 'page.' + ext),
      pageComponent(framework, 'product'),
    ],
    [
      join('src', 'routes', 'products', '[id]', 'page.server.ts'),
      `import { defineLoad } from '@oxide-fw/core/server'

export const config = { render: 'isr', revalidate: 30, tags: ['products'] }

export const load = defineLoad(async ({ params }) => {
  const id = String(params.id ?? 'sample')
  return { id, name: 'Product ' + id, priceCents: 1999, tags: ['products'] }
})

export async function generateParams() {
  return [{ id: 'sample' }, { id: 'featured' }]
}
`,
    ],
  ];
}

function shapeExtras(framework: Framework, shape: Shape, ext: string): readonly Write[] {
  if (shape === 'blog') return blogExtras(framework, ext);
  if (shape === 'dashboard') return dashboardExtras(framework, ext);
  if (shape === 'storefront') return storefrontExtras(framework, ext);
  return [];
}

export async function scaffold(template: string, framework: string, dir: string): Promise<string[]> {
  if (!isFramework(framework)) {
    throw new Error(`unknown framework "${framework}" (expected one of ${FRAMEWORKS.join(', ')})`);
  }
  if (!isShape(template)) {
    throw new Error(`unknown template "${template}" (expected one of ${SHAPES.join(', ')})`);
  }
  let existing: string[] = [];
  try {
    existing = await readdir(dir);
  } catch {
    /* dir may not exist yet */
  }
  if (existing.length > 0) {
    throw new NonEmptyDirError(dir);
  }
  const routes = join(dir, 'src', 'routes');
  await mkdir(routes, { recursive: true });
  const ext = framework === 'vue' ? 'vue' : framework === 'svelte' ? 'svelte' : 'tsx';
  const writes: Write[] = [
    [join(dir, 'package.json'), packageJson(framework)],
    [join(dir, 'oxide.config.ts'), configTs(framework, template)],
    [join(dir, 'README.md'), readmeMd(framework, template)],
    [join(dir, 'src', 'routes', 'layout.' + ext), layoutComponent(framework)],
    [join(dir, 'src', 'routes', 'page.' + ext), pageComponent(framework, template)],
    [join(dir, 'src', 'routes', 'page.server.ts'), pageServerTs(template)],
  ];
  const extras = shapeExtras(framework, template, ext);
  for (const [rel, content] of extras) {
    writes.push([join(dir, rel), content]);
  }
  for (const [path, content] of writes) {
    await mkdir(join(path, '..'), { recursive: true });
    await writeFile(path, content, 'utf8');
  }
  return writes.map(([p]) => p);
}

export async function main(argv: readonly string[] = process.argv.slice(2)): Promise<number> {
  const [template = 'minimal', framework = 'react', dir = '.'] = argv;
  if (!isShape(template) || !isFramework(framework)) {
    console.error(`✖ unknown template "${template}" or framework "${framework}"`);
    return 2;
  }
  try {
    const files = await scaffold(template, framework, dir);
    console.log(`✔ scaffolded oxide ${template} (${framework}) into ${dir}`);
    for (const f of files) console.log(`  ${f}`);
    console.log(`\nnext: npm install && npm run dev`);
    return 0;
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    const code = err instanceof NonEmptyDirError ? 3 : 2;
    console.error(`✖ ${message}`);
    return code;
  }
}

if (process.argv[1] && process.argv[1].endsWith('cli.ts')) {
  main().then((code) => process.exit(code));
}
