import { mkdirSync } from "node:fs";
import { dirname } from "node:path";
import { chromium } from "playwright";

export interface RenderJob {
  html: string;
  width: number;
  height: number;
  outPath: string;
  /** Render at a multiple of the target size for crisp retina output (default 2). */
  scale?: number;
}

/**
 * Render an HTML document with Playwright/Chromium and screenshot it
 * at an exact pixel size. Fonts and images are inlined as data URIs,
 * so no network access is needed at render time.
 */
export async function renderToPng({ html, width, height, outPath, scale = 2 }: RenderJob): Promise<void> {
  mkdirSync(dirname(outPath), { recursive: true });

  const browser = await chromium.launch({ args: ["--force-color-profile=srgb", "--disable-lcd-text"] });
  try {
    const page = await browser.newPage({
      viewport: { width, height },
      deviceScaleFactor: scale,
    });
    await page.setContent(html, { waitUntil: "load" });
    // Let font rasterization and the noise filter settle before the shot.
    await page.evaluate(() => document.fonts.ready);
    await page.waitForTimeout(120);
    await page.screenshot({ path: outPath, type: "png" });
    await page.close();
  } finally {
    await browser.close();
  }
}
