#!/usr/bin/env node
/**
 * Builds the store and platform logos the Library's browse bar shows in its
 * Source and Platform modes: one white PNG per brand, the mark and the name
 * side by side, every one of them the same height.
 *
 *   node scripts/build-brand-logos.mjs
 *
 * The sources are in `assets/brands/source` (where each came from, and under
 * what terms, is in `assets/brands/SOURCES.md`) and `assets/brands/brands.json`
 * says how each logo is put together. Writes `public/media/brands/<id>.png` and
 * `src/brand-logos.generated.ts`. The results are committed, so only someone
 * changing a logo runs this; it renders in Playwright's Chromium, which the
 * end-to-end suite already installs. `BRAND_LOGOS_CHROMIUM` points it at
 * another Chromium binary.
 *
 * Every logo goes through the same steps, in the page, on a canvas:
 *
 * 1. Each part is rasterised tall, then made white. "Ink" — the default — turns
 *    whatever is darker than `inkMax` fully white and fades lighter colours to
 *    transparent, so a dark mark becomes a white one and the white details cut
 *    into it (Steam's piston, the text inside Epic's shield, the X in Xbox's
 *    sphere) stay cut out instead of filling in. "Silhouette" keeps every
 *    opaque pixel, for a mark drawn in light colours.
 * 2. Parts are laid out in a row and centred on one line. A mark's height is
 *    `scale` times the lockup's; a name's cap height is, so names with and
 *    without descenders read at the same size. `split: "row"` turns a logo
 *    stacked mark-over-name into a row, at the widest empty band between them;
 *    `nameScale` then enlarges the name, which a stacked logo sets small.
 * 3. The row is trimmed to its ink and scaled to `height`, so every logo is
 *    exactly as tall as every other.
 */
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const SOURCE = join(ROOT, "assets/brands/source");
const OUTPUT = join(ROOT, "public/media/brands");
const MODULE = join(ROOT, "src/brand-logos.generated.ts");

const manifest = JSON.parse(readFileSync(join(ROOT, "assets/brands/brands.json"), "utf8"));

/** Runs in the page: one lockup, as a PNG data URL plus its size. */
async function renderLockup({ parts, gap, height, font }) {
  // The height of a scale-1 part while working. Large, so the final scale-down
  // is the only resampling a logo goes through.
  const BASIS = 480;
  const INK_MAX = 0.72;

  const canvasOf = (width, tall) => {
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.ceil(width));
    canvas.height = Math.max(1, Math.ceil(tall));
    return canvas;
  };

  const loadSvg = (svg) =>
    new Promise((done, fail) => {
      const image = new Image();
      image.onload = () => done(image);
      image.onerror = () => fail(new Error("an SVG did not decode"));
      image.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
    });

  /** Rows and columns that carry ink, as a bounding box. */
  const inkBounds = (canvas) => {
    const { data, width, height: tall } = canvas.getContext("2d").getImageData(0, 0, canvas.width, canvas.height);
    let top = tall;
    let bottom = -1;
    let left = width;
    let right = -1;
    for (let y = 0; y < tall; y += 1) {
      for (let x = 0; x < width; x += 1) {
        if (data[(y * width + x) * 4 + 3] > 6) {
          if (y < top) top = y;
          if (y > bottom) bottom = y;
          if (x < left) left = x;
          if (x > right) right = x;
        }
      }
    }
    return bottom < 0 ? null : { left, top, width: right - left + 1, height: bottom - top + 1 };
  };

  const crop = (canvas, box) => {
    const out = canvasOf(box.width, box.height);
    out.getContext("2d").drawImage(canvas, box.left, box.top, box.width, box.height, 0, 0, box.width, box.height);
    return out;
  };

  const trim = (canvas) => {
    const box = inkBounds(canvas);
    if (!box) throw new Error("a part has no ink at all");
    return crop(canvas, box);
  };

  const whiten = (canvas, mode, inkMax) => {
    const context = canvas.getContext("2d");
    const image = context.getImageData(0, 0, canvas.width, canvas.height);
    const { data } = image;
    for (let index = 0; index < data.length; index += 4) {
      const alpha = data[index + 3] / 255;
      if (alpha === 0) continue;
      // Colours arrive premultiplied-then-unpremultiplied, so an edge pixel
      // keeps its hue; luminance alone decides how much of it is ink.
      const luminance = (0.2126 * data[index] + 0.7152 * data[index + 1] + 0.0722 * data[index + 2]) / 255;
      const ink = mode === "silhouette" ? 1 : Math.min(1, Math.max(0, (1 - luminance) / (1 - inkMax)));
      data[index] = 255;
      data[index + 1] = 255;
      data[index + 2] = 255;
      data[index + 3] = Math.round(255 * alpha * ink);
    }
    context.putImageData(image, 0, 0);
    return canvas;
  };

  /** A logo stacked mark-over-name, cut at its widest empty band. */
  const splitRow = (canvas) => {
    const { data, width, height: tall } = canvas.getContext("2d").getImageData(0, 0, canvas.width, canvas.height);
    const empty = (y) => {
      for (let x = 0; x < width; x += 1) if (data[(y * width + x) * 4 + 3] > 6) return false;
      return true;
    };
    let best = null;
    let start = -1;
    for (let y = 0; y < tall; y += 1) {
      if (empty(y)) {
        if (start < 0) start = y;
      } else if (start >= 0) {
        if (start > 0 && (!best || y - start > best.end - best.start)) best = { start, end: y };
        start = -1;
      }
    }
    if (!best) throw new Error("split: no empty band to cut at");
    const top = trim(crop(canvas, { left: 0, top: 0, width, height: best.start }));
    const bottom = trim(crop(canvas, { left: 0, top: best.end, width, height: tall - best.end }));
    return [top, bottom];
  };

  // Each piece: a canvas, how tall it must end up, and where its centre line is.
  const pieces = [];
  for (const part of parts) {
    if (part.svg) {
      const image = await loadSvg(part.svg);
      const ratio = image.naturalWidth / image.naturalHeight || 1;
      const raster = canvasOf(1600 * ratio, 1600);
      raster.getContext("2d").drawImage(image, 0, 0, raster.width, raster.height);
      const white = trim(whiten(raster, part.mode ?? "ink", part.inkMax ?? INK_MAX));
      if (part.split === "row") {
        // Keep the two halves at the sizes they had to each other.
        const [mark, name] = splitRow(white);
        const unit = (BASIS * (part.scale ?? 1)) / mark.height;
        pieces.push({ canvas: mark, height: mark.height * unit, centre: 0.5 });
        pieces.push({ canvas: name, height: name.height * unit * (part.nameScale ?? 1), centre: 0.5 });
      } else {
        pieces.push({ canvas: white, height: BASIS * (part.scale ?? 1), centre: 0.5 });
      }
    } else {
      const size = 600;
      const probe = canvasOf(1, 1).getContext("2d");
      probe.font = `${part.weight ?? 600} ${size}px ${font}`;
      const capHeight = probe.measureText("H").actualBoundingBoxAscent;
      const measured = probe.measureText(part.text);
      const raster = canvasOf(measured.width + size, size * 2);
      const context = raster.getContext("2d");
      context.font = probe.font;
      context.fillStyle = "#fff";
      const baseline = size * 1.4;
      context.fillText(part.text, size / 2, baseline);
      const box = inkBounds(raster);
      const name = crop(raster, box);
      // Centre the capital letters on the line, and let descenders hang.
      const capTop = baseline - capHeight - box.top;
      const unit = (BASIS * (part.scale ?? 0.56)) / capHeight;
      pieces.push({ canvas: name, height: name.height * unit, centre: (capTop + capHeight / 2) / name.height });
    }
  }

  // Lay out on a row, every centre on one horizontal line.
  const gapPx = BASIS * gap;
  const widths = pieces.map((piece) => (piece.canvas.width * piece.height) / piece.canvas.height);
  const above = Math.max(...pieces.map((piece) => piece.height * piece.centre));
  const below = Math.max(...pieces.map((piece) => piece.height * (1 - piece.centre)));
  const row = canvasOf(widths.reduce((sum, width) => sum + width, 0) + gapPx * (pieces.length - 1), above + below);
  const context = row.getContext("2d");
  context.imageSmoothingQuality = "high";
  let x = 0;
  pieces.forEach((piece, index) => {
    context.drawImage(piece.canvas, x, above - piece.height * piece.centre, widths[index], piece.height);
    x += widths[index] + gapPx;
  });

  // Trim, then step down to the final height: halving keeps thin strokes from
  // breaking up the way one large jump would.
  let current = trim(row);
  while (current.height / 2 > height) {
    const half = canvasOf(current.width / 2, current.height / 2);
    const halfContext = half.getContext("2d");
    halfContext.imageSmoothingQuality = "high";
    halfContext.drawImage(current, 0, 0, half.width, half.height);
    current = half;
  }
  const out = canvasOf((current.width * height) / current.height, height);
  const outContext = out.getContext("2d");
  outContext.imageSmoothingQuality = "high";
  outContext.drawImage(current, 0, 0, out.width, out.height);
  return { png: out.toDataURL("image/png"), width: out.width, height: out.height };
}

const browser = await chromium.launch({ executablePath: process.env.BRAND_LOGOS_CHROMIUM || undefined });
try {
  const page = await browser.newPage();
  await page.setContent("<!doctype html><title>logos</title>");
  mkdirSync(OUTPUT, { recursive: true });
  const entries = [];
  for (const logo of manifest.logos) {
    const parts = logo.parts.map((part) =>
      part.file ? { ...part, svg: readFileSync(join(SOURCE, part.file), "utf8") } : part,
    );
    const result = await page.evaluate(renderLockup, {
      parts,
      gap: logo.gap ?? 0.3,
      height: manifest.height,
      font: manifest.font,
    });
    writeFileSync(join(OUTPUT, `${logo.id}.png`), Buffer.from(result.png.split(",")[1], "base64"));
    entries.push({ id: logo.id, label: logo.label, width: result.width, height: result.height });
    console.log(`${logo.id.padEnd(16)} ${result.width}x${result.height}`);
  }
  const lines = entries.map(
    (entry) =>
      `  ${JSON.stringify(entry.id)}: { src: "/media/brands/${entry.id}.png", width: ${entry.width}, height: ${entry.height}, label: ${JSON.stringify(entry.label)} },`,
  );
  writeFileSync(
    MODULE,
    [
      "// Generated by scripts/build-brand-logos.mjs from assets/brands/brands.json. Do not edit.",
      "export const BRAND_LOGOS = {",
      ...lines,
      "} as const;",
      "",
      "export type BrandLogoId = keyof typeof BRAND_LOGOS;",
      "",
    ].join("\n"),
  );
} finally {
  await browser.close();
}
