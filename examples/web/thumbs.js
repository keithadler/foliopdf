// Page thumbnails via pdf.js (Mozilla, Apache-2.0), loaded lazily from ./vendor.
// This is presentation only: the editing engine never depends on it, and when
// pdf.js is missing or a file cannot be rendered, callers fall back to plain tiles.

let libPromise = null;
function lib() {
  if (!libPromise) {
    libPromise = import("./vendor/pdf.min.mjs")
      .then((p) => { p.GlobalWorkerOptions.workerSrc = new URL("./vendor/pdf.worker.min.mjs", import.meta.url).href; return p; })
      .catch(() => null);
  }
  return libPromise;
}

/** Whether thumbnails can be produced at all (resolves once). */
export async function available() { return !!(await lib()); }

/** The pdf.js module itself (null when unavailable). */
export const pdfjs = () => lib();

/** Opens a PDF with pdf.js for on-screen rendering; null when pdf.js is missing or the file cannot be opened. */
export async function openPdf(bytes, password) {
  const p = await lib(); if (!p) return null;
  try { return await p.getDocument({ data: bytes.slice(), password: password || undefined, isEvalSupported: false }).promise; } catch { return null; }
}

const MAX_CONCURRENT = 2;
let active = 0;
const queue = [];
function schedule(task) {
  return new Promise((resolve) => {
    queue.push(async () => { try { resolve(await task()); } catch { resolve(null); } });
    pump();
  });
}
function pump() {
  while (active < MAX_CONCURRENT && queue.length) {
    const t = queue.shift(); active++;
    t().finally(() => { active--; pump(); });
  }
}

/**
 * Creates a thumbnailer for one PDF. `render(pageIndex, cssWidth)` resolves to a
 * <canvas> (device-pixel aware) or null. Results are cached per page and width.
 */
export function thumbnailer(bytes, password) {
  let docPromise = null;
  const cache = new Map();
  let destroyed = false;
  const doc = () => {
    if (!docPromise) {
      docPromise = lib().then((p) => p ? p.getDocument({ data: bytes.slice(), password: password || undefined, isEvalSupported: false, disableFontFace: false }).promise : null).catch(() => null);
    }
    return docPromise;
  };
  return {
    render(index, width) {
      const key = index + "@" + width;
      if (cache.has(key)) return cache.get(key);
      const p = schedule(async () => {
        if (destroyed) return null;
        const d = await doc(); if (!d) return null;
        const page = await d.getPage(index + 1);
        const scale = window.devicePixelRatio > 1 ? 2 : 1;
        const vp0 = page.getViewport({ scale: 1 });
        const s = (width / vp0.width) * scale;
        const vp = page.getViewport({ scale: s });
        const canvas = document.createElement("canvas");
        canvas.width = Math.ceil(vp.width); canvas.height = Math.ceil(vp.height);
        canvas.style.width = Math.round(vp.width / scale) + "px"; canvas.style.height = Math.round(vp.height / scale) + "px";
        await page.render({ canvasContext: canvas.getContext("2d", { alpha: false }), viewport: vp, background: "#ffffff" }).promise;
        page.cleanup();
        return canvas;
      });
      cache.set(key, p);
      return p;
    },
    destroy() { destroyed = true; cache.clear(); docPromise?.then((d) => d?.loadingTask?.destroy?.()).catch(() => {}); },
  };
}
