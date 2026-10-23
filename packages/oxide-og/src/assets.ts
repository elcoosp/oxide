import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";

const require = createRequire(import.meta.url);

export interface OgAssets {
  logo: string;
  react: string;
  vue: string;
  svelte: string;
  solid: string;
}

const ICON_NAMES: Array<[keyof OgAssets, string]> = [
  ["logo", "logo.png"],
  ["react", "react.png"],
  ["vue", "vue.png"],
  ["svelte", "svelte.png"],
  ["solid", "solid.png"],
];

/** Read a PNG from disk and inline it as a base64 data URI. */
export function pngDataUri(path: string): string {
  const buf = readFileSync(path);
  return `data:image/png;base64,${buf.toString("base64")}`;
}

/**
 * Resolve the five brand PNGs from an input directory (defaults to ./assets).
 * Throws with a helpful message if one is missing.
 */
export function loadAssets(inputDir?: string): OgAssets {
  const dir = resolve(inputDir ?? join(dirname(require.main!.filename), "..", "assets"));
  const out = {} as OgAssets;
  for (const [key, file] of ICON_NAMES) {
    const p = join(dir, file);
    try {
      out[key] = pngDataUri(p);
    } catch {
      throw new Error(
        `Missing required asset: ${p}\n` +
          `Expected docs exports in the input dir: logo.png, react.png, vue.png, svelte.png, solid.png\n` +
          `Pass --input <dir> to point at the Oxide docs directory.`,
      );
    }
  }
  return out;
}

/** Resolve a @fontsource woff2 file to a base64 data URI for inlining into the template. */
function fontDataUri(pkg: string, file: string): string {
  const pkgJson = require.resolve(`${pkg}/package.json`);
  const woff = join(dirname(pkgJson), "files", file);
  const buf = readFileSync(woff);
  return `data:font/woff2;base64,${buf.toString("base64")}`;
}

export interface OgFonts {
  inter500: string;
  inter600: string;
  inter700: string;
  inter800: string;
  mono400: string;
  mono500: string;
}

export function loadFonts(): OgFonts {
  return {
    inter500: fontDataUri("@fontsource/inter", "inter-latin-500-normal.woff2"),
    inter600: fontDataUri("@fontsource/inter", "inter-latin-600-normal.woff2"),
    inter700: fontDataUri("@fontsource/inter", "inter-latin-700-normal.woff2"),
    inter800: fontDataUri("@fontsource/inter", "inter-latin-800-normal.woff2"),
    mono400: fontDataUri("@fontsource/jetbrains-mono", "jetbrains-mono-latin-400-normal.woff2"),
    mono500: fontDataUri("@fontsource/jetbrains-mono", "jetbrains-mono-latin-500-normal.woff2"),
  };
}
