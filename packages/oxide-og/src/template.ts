import type { OgAssets, OgFonts } from "./assets.js";

export interface TemplateOptions {
  width: number;
  height: number;
  headline: string;
  sub: string;
  repo: string;
  kicker: string;
  version: string;
  footerLeft: string;
  footerRight: string;
}

export interface TemplateData {
  assets: OgAssets;
  fonts: OgFonts;
  options: TemplateOptions;
  theme?: ThemeName;
}

export type ThemeName = "light" | "dark";

/**
 * All theme-dependent styling lives in these CSS custom properties.
 * Light (default): warm paper canvas — the transparent brand PNGs read
 * naturally on white surfaces. Dark: the original midnight variant.
 */
export const THEMES: Record<ThemeName, Record<string, string>> = {
  light: {
    "--bg": "linear-gradient(180deg, #fbf9f6 0%, #f8f6f2 55%, #f4f2ed 100%)",
    "--glow-a": "radial-gradient(circle, rgba(224,84,32,0.11) 0%, rgba(224,84,32,0.04) 36%, rgba(224,84,32,0) 64%)",
    "--glow-b": "radial-gradient(circle, rgba(224,84,32,0.08) 0%, rgba(224,84,32,0.03) 36%, rgba(224,84,32,0) 66%)",
    "--grid-line": "rgba(23,23,27,0.045)",
    "--noise-opacity": "0.035",
    "--vignette": "radial-gradient(ellipse 125% 105% at 50% 44%, rgba(0,0,0,0) 55%, rgba(56,34,22,0.08) 100%)",
    "--ink": "#17171b",
    "--ink-soft": "#57575f",
    "--ink-faint": "rgba(23,23,27,0.44)",
    "--hairline": "rgba(23,23,27,0.10)",
    "--accent": "#c9451d",
    "--pill-bg": "rgba(201,69,29,0.06)",
    "--pill-border": "rgba(201,69,29,0.32)",
    "--h1-accent": "linear-gradient(93deg, #c93e14 0%, #f26430 55%, #ff9a5a 100%)",
    "--card-bg": "#ffffff",
    "--card-border": "rgba(23,23,27,0.08)",
    "--card-shadow": "0 1px 2px rgba(40,25,15,0.05), 0 14px 34px rgba(40,25,15,0.09)",
    "--icon-bg": "transparent",
    "--icon-border": "none",
    "--icon-shadow": "none",
    "--logo-border": "rgba(23,23,27,0.10)",
    "--logo-shadow": "0 12px 32px rgba(180,70,25,0.18), 0 1px 2px rgba(40,25,15,0.08)",
    "--chip-name": "#1b1b20",
    "--chip-bridge": "rgba(23,23,27,0.42)",
  },
  dark: {
    "--bg": "linear-gradient(180deg, #0c0c10 0%, #0a0a0d 52%, #09090c 100%)",
    "--glow-a": "radial-gradient(circle, rgba(224,84,32,0.20) 0%, rgba(224,84,32,0.07) 34%, rgba(224,84,32,0) 64%)",
    "--glow-b": "radial-gradient(circle, rgba(224,84,32,0.13) 0%, rgba(224,84,32,0.045) 36%, rgba(224,84,32,0) 66%)",
    "--grid-line": "rgba(255,255,255,0.028)",
    "--noise-opacity": "0.05",
    "--vignette": "radial-gradient(ellipse 125% 105% at 50% 44%, rgba(0,0,0,0) 52%, rgba(0,0,0,0.46) 100%)",
    "--ink": "#f6f6f8",
    "--ink-soft": "rgba(255,255,255,0.56)",
    "--ink-faint": "rgba(255,255,255,0.42)",
    "--hairline": "rgba(255,255,255,0.08)",
    "--accent": "#e8642e",
    "--pill-bg": "rgba(240,122,74,0.08)",
    "--pill-border": "rgba(240,122,74,0.35)",
    "--h1-accent": "linear-gradient(93deg, #f26430 0%, #ff8a50 55%, #ffb98c 100%)",
    "--card-bg": "rgba(255,255,255,0.035)",
    "--card-border": "rgba(255,255,255,0.09)",
    "--card-shadow": "inset 0 1px 0 rgba(255,255,255,0.05), 0 12px 32px rgba(0,0,0,0.38)",
    "--icon-bg": "#ffffff",
    "--icon-border": "none",
    "--icon-shadow": "0 2px 10px rgba(0,0,0,0.35), inset 0 0 0 1px rgba(0,0,0,0.06)",
    "--logo-border": "rgba(255,255,255,0.16)",
    "--logo-shadow": "0 10px 34px rgba(224,84,32,0.22), inset 0 0 0 1px rgba(0,0,0,0.06)",
    "--chip-name": "#ececef",
    "--chip-bridge": "rgba(255,255,255,0.35)",
  },
};

export interface AdapterChip {
  name: string;
  bridge: string;
  icon: keyof OgAssets;
}

/** The real upstream renderers each adapter bridges to (from the README). */
export const ADAPTER_CHIPS: AdapterChip[] = [
  { name: "React", bridge: "react-dom/server", icon: "react" },
  { name: "Vue", bridge: "@vue/server-renderer", icon: "vue" },
  { name: "Svelte", bridge: "svelte/server", icon: "svelte" },
  { name: "Solid", bridge: "@solidjs/web", icon: "solid" },
];

function esc(s: string): string {
  return s
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

/** Split the headline on the first sentence boundary: line 1 ink, line 2 brand-gradient. */
function headlineLines(headline: string): [string, string] {
  const m = headline.match(/^(.+?[.!?])\s+(.+)$/);
  if (m) return [m[1], m[2]];
  const words = headline.split(" ");
  if (words.length < 2) return [headline, ""];
  const cut = Math.ceil(words.length / 2);
  return [words.slice(0, cut).join(" "), words.slice(cut).join(" ")];
}

const NOISE_SVG = `<svg xmlns='http://www.w3.org/2000/svg' width='160' height='160'><filter id='n'><feTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='2' stitchTiles='stitch'/><feColorMatrix type='saturate' values='0'/></filter><rect width='100%' height='100%' filter='url(%23n)'/></svg>`;

export function buildTemplate({ assets, fonts, options, theme = "light" }: TemplateData): string {
  const o = options;
  const vars = THEMES[theme];
  const cssVars = Object.entries(vars)
    .map(([k, v]) => `    ${k}:${v};`)
    .join("\n");
  const [line1, line2] = headlineLines(o.headline);

  const chips = ADAPTER_CHIPS.map(
    (c) => `
      <div class="chip">
        <img class="chip-icon" src="${assets[c.icon]}" alt="${esc(c.name)}"/>
        <div class="chip-text">
          <div class="chip-name">${esc(c.name)}</div>
          <div class="chip-bridge">${esc(c.bridge)}</div>
        </div>
      </div>`,
  ).join("\n");

  return `<!doctype html>
<html>
<head>
<meta charset="utf-8"/>
<style>
  :root {
${cssVars}
  }
  @font-face { font-family:'Inter'; font-weight:500; font-style:normal; font-display:block;
    src:url("${fonts.inter500}") format("woff2"); }
  @font-face { font-family:'Inter'; font-weight:600; font-style:normal; font-display:block;
    src:url("${fonts.inter600}") format("woff2"); }
  @font-face { font-family:'Inter'; font-weight:700; font-style:normal; font-display:block;
    src:url("${fonts.inter700}") format("woff2"); }
  @font-face { font-family:'Inter'; font-weight:800; font-style:normal; font-display:block;
    src:url("${fonts.inter800}") format("woff2"); }
  @font-face { font-family:'JetBrains Mono'; font-weight:400; font-style:normal; font-display:block;
    src:url("${fonts.mono400}") format("woff2"); }
  @font-face { font-family:'JetBrains Mono'; font-weight:500; font-style:normal; font-display:block;
    src:url("${fonts.mono500}") format("woff2"); }

  * { margin:0; padding:0; box-sizing:border-box; }
  html, body { width:${o.width}px; height:${o.height}px; overflow:hidden; }
  body { font-family:'Inter', sans-serif; -webkit-font-smoothing:antialiased; text-rendering:optimizeLegibility; }

  .canvas {
    position:relative; width:100%; height:100%; overflow:hidden;
    background-color:#0a0a0d;
    background-image:var(--bg);
  }

  /* ---------- atmosphere ---------- */
  .glow-a { position:absolute; left:-240px; top:-280px; width:860px; height:860px;
    background:var(--glow-a); }
  .glow-b { position:absolute; right:-300px; bottom:-340px; width:940px; height:940px;
    background:var(--glow-b); }
  .grid { position:absolute; inset:0;
    background-image:
      linear-gradient(var(--grid-line) 1px, transparent 1px),
      linear-gradient(90deg, var(--grid-line) 1px, transparent 1px);
    background-size:54px 54px; background-position:center center;
    -webkit-mask-image:radial-gradient(ellipse 92% 82% at 50% 42%, #000 26%, transparent 76%);
    mask-image:radial-gradient(ellipse 92% 82% at 50% 42%, #000 26%, transparent 76%); }
  .noise { position:absolute; inset:0; opacity:var(--noise-opacity);
    background-image:url("data:image/svg+xml,${encodeURIComponent(NOISE_SVG).replaceAll("%2523", "%23")}"); background-size:160px 160px; }
  .vignette { position:absolute; inset:0; background:var(--vignette); }
  .topline { position:absolute; top:0; left:0; right:0; height:3px;
    background:linear-gradient(90deg, rgba(224,84,32,0) 0%, rgba(242,100,48,0.9) 30%, rgba(255,154,90,0.9) 50%, rgba(242,100,48,0.9) 70%, rgba(224,84,32,0) 100%); }

  /* ---------- layout ---------- */
  .content { position:relative; z-index:1; height:100%;
    display:flex; flex-direction:column; justify-content:space-between;
    padding:50px 58px 42px; }

  /* ---------- header ---------- */
  .header { display:flex; align-items:center; justify-content:space-between; }
  .brand { display:flex; align-items:center; gap:18px; }
  .logo { width:62px; height:62px; border-radius:15px; background:#ffffff;
    border:1px solid var(--logo-border);
    box-shadow:var(--logo-shadow); }
  .wordmark { font-size:33px; font-weight:700; letter-spacing:-0.022em; color:var(--ink); }
  .pill { margin-left:4px; font-family:'JetBrains Mono', monospace; font-size:13px; font-weight:500;
    color:var(--accent); letter-spacing:0.02em;
    border:1px solid var(--pill-border); background:var(--pill-bg);
    border-radius:999px; padding:5px 13px; }
  .repo { font-family:'JetBrains Mono', monospace; font-size:15px; font-weight:400;
    color:var(--ink-faint); letter-spacing:-0.01em; }

  /* ---------- hero ---------- */
  .hero { margin-top:-6px; }
  .kicker { font-family:'JetBrains Mono', monospace; font-size:13.5px; font-weight:500;
    color:var(--accent); letter-spacing:0.24em; text-transform:uppercase; }
  h1 { margin-top:18px; font-size:92px; line-height:1.0; font-weight:800;
    letter-spacing:-0.035em; color:var(--ink); }
  h1 .accent {
    background:var(--h1-accent);
    -webkit-background-clip:text; background-clip:text; color:transparent; }
  .sub { margin-top:26px; max-width:840px; font-size:21px; font-weight:500;
    line-height:1.55; color:var(--ink-soft); letter-spacing:-0.005em; }

  /* ---------- adapter chips ---------- */
  .chips { display:flex; gap:14px; }
  .chip { display:flex; align-items:center; gap:14px;
    padding:11px 22px 11px 11px; border-radius:16px;
    background:var(--card-bg);
    border:1px solid var(--card-border);
    box-shadow:var(--card-shadow); }
  .chip-icon { width:44px; height:44px; border-radius:11px;
    background:var(--icon-bg); border:var(--icon-border); box-shadow:var(--icon-shadow); }
  .chip-name { font-size:18px; font-weight:600; color:var(--chip-name); letter-spacing:-0.01em; }
  .chip-bridge { margin-top:2px; font-family:'JetBrains Mono', monospace; font-size:11.5px;
    color:var(--chip-bridge); }

  /* ---------- footer ---------- */
  .footer { display:flex; align-items:center; justify-content:space-between;
    border-top:1px solid var(--hairline); padding-top:21px; }
  .footer span { font-family:'JetBrains Mono', monospace; font-size:13.5px;
    color:var(--ink-faint); letter-spacing:-0.01em; }
  .footer .dot { color:rgba(240,122,74,0.65); padding:0 9px; }
</style>
</head>
<body>
  <div class="canvas">
    <div class="glow-a"></div>
    <div class="glow-b"></div>
    <div class="grid"></div>
    <div class="noise"></div>
    <div class="vignette"></div>
    <div class="topline"></div>

    <div class="content">
      <div class="header">
        <div class="brand">
          <img class="logo" src="${assets.logo}" alt="Oxide"/>
          <span class="wordmark">oxide</span>
          <span class="pill">${esc(o.version)}</span>
        </div>
        <span class="repo">${esc(o.repo)}</span>
      </div>

      <div class="hero">
        <div class="kicker">${esc(o.kicker)}</div>
        <h1>${esc(line1)}<br/><span class="accent">${esc(line2)}</span></h1>
        <p class="sub">${esc(o.sub)}</p>
      </div>

      <div class="chips">${chips}</div>

      <div class="footer">
        <span>${esc(o.footerLeft)}</span>
        <span>${esc(o.footerRight)}</span>
      </div>
    </div>
  </div>
</body>
</html>`;
}
