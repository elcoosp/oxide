#!/usr/bin/env node
import { parseArgs } from "node:util";
import { loadAssets, loadFonts } from "./assets.js";
import { buildTemplate } from "./template.js";
import { renderToPng } from "./render.js";
import { join, resolve } from "node:path";

const SIZE_PRESETS = {
  og: { width: 1200, height: 630, name: "og.png" },
  github: { width: 1280, height: 640, name: "og-github.png" },
} as const;

type SizeKey = keyof typeof SIZE_PRESETS;

const DEFAULTS = {
  headline: "One Rust core. Every framework.",
  sub: "A framework-agnostic full-stack meta-framework — one shared Rust engine for routing, data, caching, bundling and serving.",
  repo: "github.com/elcoosp/oxide",
  kicker: "The Rust-core meta-framework",
  version: "v0.1",
  footerLeft: "HTTP/1+2+3 · Streaming SSR · ODF byte-parity · Single static binary",
  footerRight: "MIT OR Apache-2.0",
};

/** Extract a required string from parseArgs values; throws with a clear
 *  message when the flag is missing (or non-string). Runtime check is the
 *  narrowing primitive -- no `!`, no cast. */
function requireString(values: Record<string, string | boolean | undefined>, key: string): string {
  const v = values[key]
  if (typeof v !== "string") throw new Error(`oxide-og: --${key} is required (got ${typeof v})`)
  return v
}

function main(): Promise<void> {
  return run();
}

async function run(): Promise<void> {
  const { values } = parseArgs({
    options: {
      input: { type: "string", default: join(process.cwd(), "assets") },
      out: { type: "string", default: join(process.cwd(), "out") },
      sizes: { type: "string", default: "all" },
      theme: { type: "string", default: "light" },
      scale: { type: "string", default: "2" },
      headline: { type: "string", default: DEFAULTS.headline },
      sub: { type: "string", default: DEFAULTS.sub },
      repo: { type: "string", default: DEFAULTS.repo },
      kicker: { type: "string", default: DEFAULTS.kicker },
      version: { type: "string", default: DEFAULTS.version },
      "footer-left": { type: "string", default: DEFAULTS.footerLeft },
      "footer-right": { type: "string", default: DEFAULTS.footerRight },
      help: { type: "boolean", default: false },
    },
    strict: true,
  });

  if (values.help) {
    console.log(`oxide-og — premium OpenGraph image generator for the Oxide project

Usage: oxide-og [options]

Options:
  --input <dir>      Directory with logo.png, react.png, vue.png, svelte.png, solid.png
                     (default: ./assets; point it at the repo's docs/ directory)
  --out <dir>        Output directory                    (default: ./out)
  --sizes <list>     og | github | all                   (default: all)
                       og     -> 1200x630  og.png          (standard OpenGraph / link previews)
                       github -> 1280x640  og-github.png  (GitHub social preview)
  --theme <name>     light | dark                        (default: light; dark outputs get a -dark suffix)
  --scale <n>        Supersampling factor, 1 = exact px  (default: 2, retina-crisp)
  --headline <text>  First '.' splits white/gradient lines
  --sub <text>       Sub-headline
  --repo <text>      Repo link shown top-right
  --kicker <text>    Small tracked-out line above the headline
  --version <text>   Header pill text
  --footer-left <text>  --footer-right <text>
  --help`);
    return;
  }

  const sizes: SizeKey[] =
    values.sizes === "all"
      ? ["og", "github"]
      : (values.sizes.split(",").map((s) => s.trim()) as SizeKey[]);
  for (const s of sizes) {
    if (!(s in SIZE_PRESETS)) throw new Error(`Unknown size "${s}" (expected og | github | all)`);
  }

  const scale = Math.max(1, Math.min(4, Number(values.scale) || 2));

  const theme = values.theme === "dark" ? "dark" : values.theme === "light" ? "light" : null;
  if (!theme) throw new Error(`Unknown theme "${values.theme}" (expected light | dark)`);

  const assets = loadAssets(values.input);
  const fonts = loadFonts();

  for (const size of sizes) {
    const preset = SIZE_PRESETS[size];
    const html = buildTemplate({
      assets,
      fonts,
      theme,
      options: {
        width: preset.width,
        height: preset.height,
        headline: requireString(values, "headline"),
        sub: requireString(values, "sub"),
        repo: requireString(values, "repo"),
        kicker: requireString(values, "kicker"),
        version: requireString(values, "version"),
        footerLeft: requireString(values, "footer-left"),
        footerRight: requireString(values, "footer-right"),
      },
    });
    const outName = theme === "dark" ? preset.name.replace(/\.png$/, "-dark.png") : preset.name;
    const outPath = join(resolve(requireString(values, "out")), outName);
    await renderToPng({ html, width: preset.width, height: preset.height, outPath, scale });
    console.log(`✓ ${outPath}  (${preset.width}×${preset.height} @${scale}x, ${theme})`);
  }
}

main().catch((err: unknown) => {
  console.error(`oxide-og: ${err instanceof Error ? err.message : String(err)}`);
  process.exitCode = 1;
});
