// create-oxide — scaffold Oxide starters (spec §19.2).
// Four frameworks × four shapes; templates generated programmatically so each
// emitted project is self-contained (framework flavor + per-shape route modules).
import { mkdir, writeFile, readdir } from 'node:fs/promises';
import { existsSync, realpathSync } from 'node:fs';
import { dirname as pathDirname, join as pathJoin, join, resolve as pathResolve } from 'node:path';
import { pathToFileURL } from 'node:url';

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

/** True when `dir` sits inside a pnpm workspace (a `pnpm-workspace.yaml`
 *  above it), walking up to the filesystem root.
 *
 *  The previous check — `process.env['PWD'].includes('packages/')` — was a
 *  bug twice over: `PWD` is whatever the parent shell had at process START
 *  (`process.chdir()` does not update it, so tests scaffolding into a tmpdir
 *  still "saw" the caller's cwd), and `'packages/'` is a substring of plenty
 *  of legitimate install paths (`/home/me/packages/`, `/opt/packages/`).
 *  The result depended on HOW the suite was invoked (`pnpm -C
 *  packages/create-oxide test` passed, `pnpm test:packages` from the repo
 *  root failed) — an environment-dependent test, not a behavior.
 *  `pnpm-workspace.yaml` is the actual workspace marker, detected from the
 *  OUTPUT directory, which is what `workspace:*` linking needs.
 */
export function isInsidePnpmWorkspace(dir: string): boolean {
  let cur: string;
  try {
    cur = pathResolve(dir);
  } catch {
    return false;
  }
  for (;;) {
    if (existsSync(pathJoin(cur, 'pnpm-workspace.yaml'))) return true;
    const parent = pathDirname(cur);
    if (parent === cur) return false;
    cur = parent;
  }
}

export function packageJson(framework: Framework, inWorkspace = false): string {
  const scripts = {
    dev: 'oxide dev',
    build: 'oxide build',
    start: 'oxide start',
  };
  // P0-3 (pre-0.1 readiness): the release is 0.1.0 — the scaffold used to pin
  // 0.2.0, which does not exist, so `npm install` failed outright.
  // Inside the oxide monorepo (the workspace dev workflow) the scaffold pins
  // `workspace:*` instead, so pnpm links the local packages.
  const v = inWorkspace ? 'workspace:*' : '0.1.0';
  const deps: Record<string, string> = {
    '@oxide-fw/core': v,
    [`@oxide-fw/${framework}`]: v,
    // P0-4: the `oxide dev` script needs the CLI binary. The npm wrapper
    // downloads the prebuilt binary in postinstall, so a scaffold works
    // without a global install or a Rust toolchain.
    '@oxide-fw/cli': v,
  };
  // P0-3: the adapters declare the framework runtimes as OPTIONAL peer
  // dependencies, so nothing installed them and the generated project could
  // not compile. Add the runtime the chosen adapter needs, mirroring the
  // adapter's own peer range.
  const runtime = FRAMEWORK_RUNTIME_DEPS[framework];
  for (const [name, range] of Object.entries(runtime)) {
    deps[name] = range;
  }
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

/** Framework runtime packages the generated project must install (P0-3),
 *  mirroring each adapter's peerDependencies ranges. */
export const FRAMEWORK_RUNTIME_DEPS: Record<Framework, Record<string, string>> = {
  react: { react: '^19.0.0', 'react-dom': '^19.0.0' },
  vue: { vue: '^3.6.0-rc.11', '@vue/server-renderer': '^3.6.0-rc.11' },
  svelte: { svelte: '^5.0.0' },
  solid: { 'solid-js': '^2.0.0-rc.14', '@solidjs/web': '^2.0.0-rc.14' },
};

/** Generated tsconfig.json — TypeScript 7 (native preview) style, module
 *  resolution + JSX settings matching what `oxide build` expects. */
export function tsconfigJson(framework: Framework): string {
  const jsx: Record<Framework, string> = {
    react: 'react-jsx',
    solid: 'react-jsx',
    vue: 'preserve',
    svelte: 'preserve',
  };
  return JSON.stringify(
    {
      compilerOptions: {
        target: 'ES2023',
        module: 'ESNext',
        moduleResolution: 'bundler',
        lib: ['ES2023', 'DOM', 'DOM.Iterable'],
        types: [],
        strict: true,
        noEmit: true,
        isolatedModules: true,
        verbatimModuleSyntax: true,
        skipLibCheck: true,
        jsx: jsx[framework],
        jsxImportSource: framework === 'solid' ? 'solid-js' : framework === 'react' ? 'react' : undefined,
        allowImportingTsExtensions: true,
      },
      include: ['src', 'oxide.config.ts'],
    },
    null,
    2,
  ) + '\n';
}

/** Generated .gitignore — build output and env files (P0-3). */
export function gitignore(): string {
  return `node_modules/
dist/
.oxide/
.env
.env.*
!.env.example
*.log
.DS_Store
`;
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
  // BUG-PWD: workspace detection comes from the OUTPUT directory (a
  // `pnpm-workspace.yaml` above it), not the caller's `PWD` env var.
  const inWorkspace = isInsidePnpmWorkspace(dir);
  const ext = framework === 'vue' ? 'vue' : framework === 'svelte' ? 'svelte' : 'tsx';
  const writes: Write[] = [
    [join(dir, 'package.json'), packageJson(framework, inWorkspace)],
    [join(dir, 'oxide.config.ts'), configTs(framework, template)],
    [join(dir, 'README.md'), readmeMd(framework, template)],
    // P0-3: a generated project without a tsconfig or .gitignore could not
    // typecheck and would commit dist/ + node_modules/. Emit both.
    [join(dir, 'tsconfig.json'), tsconfigJson(framework)],
    [join(dir, '.gitignore'), gitignore()],
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

export interface ParsedArgs {
  dir: string;
  framework: Framework;
  template: Shape;
  help: boolean;
  version: boolean;
}

export const USAGE = `Usage: create-oxide [project-name] [options]

Scaffold an Oxide starter project.

Arguments:
  [project-name]        Target directory (default: current directory)

Options:
  -f, --framework <name>   Framework adapter: react, vue, svelte, solid
  -t, --template <shape>   Starter shape: minimal, blog, dashboard, storefront
  -d, --dir <path>         Target directory (same as the positional argument)
  -h, --help               Show this help
  -V, --version            Show the create-oxide version

Examples:
  npm create oxide@latest my-app
  npm create oxide@latest my-app -- --framework react --template blog
`;

interface RawArgs {
  flags: Record<string, string | boolean>;
  positionals: string[];
}

/** Split argv into flags and positionals, accepting both `--flag value` and
 *  `--flag=value` plus the single-dash short forms. Unknown flags error. */
export function tokenize(argv: readonly string[]): RawArgs {
  const flags: Record<string, string | boolean> = {};
  const positionals: string[] = [];
  const VALUE_FLAGS = new Set(['--framework', '-f', '--template', '-t', '--dir', '-d']);
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--') {
      positionals.push(...argv.slice(i + 1));
      break;
    }
    if (arg.startsWith('-')) {
      let name = arg;
      let value: string | undefined;
      const eq = arg.indexOf('=');
      if (eq !== -1) {
        name = arg.slice(0, eq);
        value = arg.slice(eq + 1);
      }
      if (VALUE_FLAGS.has(name) && value === undefined) {
        value = argv[++i];
        if (value === undefined) {
          throw new Error(`flag ${name} requires a value\n\n${USAGE}`);
        }
      }
      if (name === '--help' || name === '-h') {
        flags['help'] = true;
      } else if (name === '--version' || name === '-V') {
        flags['version'] = true;
      } else if (name === '--framework' || name === '-f') {
        flags['framework'] = value!;
      } else if (name === '--template' || name === '-t') {
        flags['template'] = value!;
      } else if (name === '--dir' || name === '-d') {
        flags['dir'] = value!;
      } else {
        throw new Error(`unknown option "${arg}"\n\n${USAGE}`);
      }
    } else {
      positionals.push(arg);
    }
  }
  return { flags, positionals };
}

/** Parse argv into a scaffold request. Throws on unknown/invalid input with
 *  a message that includes the usage text. Flags win; the legacy positional
 *  order `[template, framework, dir]` is still honoured for backwards
 *  compatibility (with a warning on stderr). */
export function parseArgs(
  argv: readonly string[],
  defaults: { framework?: Framework; template?: Shape } = {},
): ParsedArgs {
  const { flags, positionals } = tokenize(argv);
  // --help / --version short-circuit before flag validation.
  if (flags['help'] === true || flags['version'] === true) {
    return {
      dir: '.',
      framework: defaults.framework ?? 'react',
      template: defaults.template ?? 'minimal',
      help: flags['help'] === true,
      version: flags['version'] === true,
    };
  }
  let framework = (flags['framework'] as string | undefined) ?? defaults.framework;
  let template = (flags['template'] as string | undefined) ?? defaults.template;
  let dir = (flags['dir'] as string | undefined) ?? '.';

  // Legacy order: [template, framework, dir?] — only when the first positional
  // is a known shape and the second (if any) a known framework. `my-app` is
  // not a shape, so the documented quick-start parses as a project name.
  const p0 = positionals[0];
  const p1 = positionals[1];
  if (
    p0 !== undefined &&
    isShape(p0) &&
    template === undefined &&
    (p1 === undefined || isFramework(p1))
  ) {
    template = p0;
    if (p1 !== undefined && framework === undefined) {
      framework = p1;
      console.error('note: positional <template> <framework> <dir> is deprecated; use --framework/--template');
    }
    if (positionals[2] !== undefined && flags['dir'] === undefined) dir = positionals[2];
  } else {
    if (p0 !== undefined && flags['dir'] === undefined) dir = p0;
    if (p1 !== undefined) {
      throw new Error(`unexpected extra argument "${p1}"\n\n${USAGE}`);
    }
  }

  // Non-interactive defaults: the README headline `npx create-oxide my-app`
  // must produce a runnable project with no questions asked. Explicit but
  // INVALID flag values still error below.
  template = template ?? 'minimal';
  framework = framework ?? 'react';
  if (!isShape(template)) {
    throw new Error(`unknown template "${template}" (expected one of ${SHAPES.join(', ')})\n\n${USAGE}`);
  }
  if (!isFramework(framework)) {
    throw new Error(`unknown framework "${framework}" (expected one of ${FRAMEWORKS.join(', ')})\n\n${USAGE}`);
  }
  return { dir, framework, template, help: flags['help'] === true, version: flags['version'] === true };
}

export async function main(argv: readonly string[] = process.argv.slice(2)): Promise<number> {
  let parsed: ParsedArgs;
  try {
    parsed = parseArgs(argv);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    console.error(`✖ ${message}`);
    return 2;
  }
  if (parsed.help) {
    console.log(USAGE.trim());
    return 0;
  }
  if (parsed.version) {
    console.log('create-oxide 0.1.0');
    return 0;
  }
  try {
    const files = await scaffold(parsed.template, parsed.framework, parsed.dir);
    console.log(`✔ scaffolded oxide ${parsed.template} (${parsed.framework}) into ${parsed.dir}`);
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

/** True when THIS module is the process entry point (bin execution), not an
 *  import (tests, library consumers).
 *
 *  The previous guard — `process.argv[1].endsWith('cli.ts')` — only matched
 *  the SOURCE filename, and tsdown compiles this file to `dist/index.js`, the
 *  path `package.json`'s `bin` points at. `pnpm create oxide` therefore
 *  evaluated the module and exited 0 without ever invoking `main()`: a
 *  silent no-op CLI. Comparing the resolved entry URL is the standard ESM
 *  "am I main?" check; `realpathSync` folds the `.bin/create-oxide` symlink
 *  so both sides point at the same file. */
function isMainModule(): boolean {
  const entry = process.argv[1];
  if (!entry) return false;
  try {
    return pathToFileURL(realpathSync(entry)).href === import.meta.url;
  } catch {
    return false;
  }
}

if (isMainModule()) {
  main().then((code) => process.exit(code));
}
