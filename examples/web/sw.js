// Service worker: makes the app installable and fully usable offline.
// VERSION is stamped at deploy time; a new deploy means a new cache, and the
// page offers a reload once the new worker has finished installing.
const VERSION = "__VERSION__";
const CACHE = "foliopdf-" + VERSION;
const V = VERSION === "__VERSION__" ? "dev" : VERSION;
const PRECACHE = [
  "./", "./index.html", "./manifest.webmanifest",
  `./app.js?v=${V}`, `./editor.js?v=${V}`, `./batch.js?v=${V}`, `./thumbs.js?v=${V}`,
  "./pkg/foliopdf.js", "./pkg/foliopdf_bg.wasm",
  "./vendor/pdf.min.mjs", "./vendor/pdf.worker.min.mjs",
  "./icons/icon-192.png", "./icons/icon-512.png", "./icons/maskable-512.png", "./icons/apple-touch-icon.png",
];
const DEV = V === "dev";

self.addEventListener("install", (e) => {
  e.waitUntil((async () => {
    const c = await caches.open(CACHE);
    // Best effort: a missing optional file (vendor) must not block installation.
    await Promise.all(PRECACHE.map((u) => c.add(new Request(u, { cache: "reload" })).catch(() => {})));
  })());
});
self.addEventListener("activate", (e) => {
  e.waitUntil((async () => {
    for (const k of await caches.keys()) if (k.startsWith("foliopdf-") && k !== CACHE) await caches.delete(k);
    await self.clients.claim();
  })());
});
self.addEventListener("message", (e) => { if (e.data === "SKIP_WAITING") self.skipWaiting(); });
self.addEventListener("fetch", (e) => {
  const req = e.request;
  if (req.method !== "GET" || new URL(req.url).origin !== location.origin) return;
  if (DEV) return; // local development: always hit the server
  e.respondWith((async () => {
    const cache = await caches.open(CACHE);
    if (req.mode === "navigate") {
      // Network first so a fresh deploy shows up; the cached shell when offline.
      try { const r = await fetch(req); if (r.ok) cache.put("./index.html", r.clone()).catch(() => {}); return r; } catch { return (await cache.match("./index.html")) || (await cache.match("./")) || Response.error(); }
    }
    const hit = await cache.match(req, { ignoreSearch: false });
    if (hit) return hit;
    try { const r = await fetch(req); if (r.ok) cache.put(req, r.clone()).catch(() => {}); return r; } catch (err) { const loose = await cache.match(req, { ignoreSearch: true }); if (loose) return loose; throw err; }
  })());
});
