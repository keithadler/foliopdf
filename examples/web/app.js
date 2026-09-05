import init, { PdfDocument, runBatch, PresetStore, parsePageRanges, version, imagesToPdf } from "./pkg/foliopdf.js";
import { thumbnailer } from "./thumbs.js?v=dev";
import { batchStage } from "./batch.js?v=dev";
import { createEditor } from "./editor.js?v=dev";
export { PdfDocument, runBatch, PresetStore, parsePageRanges };

// ---------------------------------------------------------------- helpers
// Minimal ZIP writer (no compression): images are already compressed.
const CRC_TABLE = (() => { const t = new Uint32Array(256); for (let n = 0; n < 256; n++) { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; t[n] = c >>> 0; } return t; })();
function crc32(b) { let c = 0xffffffff; for (let i = 0; i < b.length; i++) c = CRC_TABLE[(c ^ b[i]) & 0xff] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; }
export function zip(items) {
  const enc = new TextEncoder(); const parts = []; const central = []; let offset = 0;
  const d = new Date(); const dosTime = ((d.getHours() << 11) | (d.getMinutes() << 5) | (d.getSeconds() >> 1)) & 0xffff; const dosDate = (((d.getFullYear() - 1980) << 9) | ((d.getMonth() + 1) << 5) | d.getDate()) & 0xffff;
  const u32 = (v) => [v & 255, (v >>> 8) & 255, (v >>> 16) & 255, (v >>> 24) & 255]; const u16 = (v) => [v & 255, (v >>> 8) & 255];
  for (const it of items) {
    const name = enc.encode(it.name); const crc = crc32(it.data);
    const local = new Uint8Array([...u32(0x04034b50), ...u16(20), ...u16(0x800), ...u16(0), ...u16(dosTime), ...u16(dosDate), ...u32(crc), ...u32(it.data.length), ...u32(it.data.length), ...u16(name.length), ...u16(0), ...name]);
    parts.push(local, it.data);
    central.push(new Uint8Array([...u32(0x02014b50), ...u16(20), ...u16(20), ...u16(0x800), ...u16(0), ...u16(dosTime), ...u16(dosDate), ...u32(crc), ...u32(it.data.length), ...u32(it.data.length), ...u16(name.length), ...u16(0), ...u16(0), ...u16(0), ...u16(0), ...u32(0), ...u32(offset), ...name]));
    offset += local.length + it.data.length;
  }
  const cdSize = central.reduce((a, c) => a + c.length, 0);
  const end = new Uint8Array([...u32(0x06054b50), ...u16(0), ...u16(0), ...u16(items.length), ...u16(items.length), ...u32(cdSize), ...u32(offset), ...u16(0)]);
  const total = offset + cdSize + end.length; const out = new Uint8Array(total); let p = 0;
  for (const c of [...parts, ...central, end]) { out.set(c, p); p += c.length; }
  return out;
}
const PAPER = [["Letter", 612, 792], ["Legal", 612, 1008], ["Tabloid", 792, 1224], ["A3", 841.89, 1190.55], ["A4", 595.28, 841.89], ["A5", 419.53, 595.28]];
/** "A4", "Letter (landscape)" or "8.5 × 11 in" for a page of w × h points. */
export function sizeLabel(w, h) {
  for (const [n, pw, ph] of PAPER) { if (Math.abs(w - pw) < 2 && Math.abs(h - ph) < 2) return n; if (Math.abs(w - ph) < 2 && Math.abs(h - pw) < 2) return n + " (landscape)"; }
  const inch = (v) => (Math.round(v / 72 * 100) / 100).toString();
  return `${inch(w)} × ${inch(h)} in`;
}
export const $ = (s, r = document) => r.querySelector(s);
export const el = (tag, attrs = {}, ...kids) => {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") e.className = v; else if (k === "html") e.innerHTML = v; else if (k.startsWith("on")) e.addEventListener(k.slice(2), v); else if (v !== false && v != null) e.setAttribute(k, v === true ? "" : v);
  }
  for (const k of kids.flat()) if (k != null && k !== false) e.append(k.nodeType ? k : document.createTextNode(k));
  return e;
};
export const kb = (n) => n >= 1048576 ? (n / 1048576).toFixed(1) + " MB" : n >= 1024 ? Math.round(n / 1024) + " KB" : n + " B";
export const stem = (n) => n.replace(/\.pdf$/i, "");
export const plural = (n, w) => `${n} ${w}${n === 1 ? "" : "s"}`;
// Yield to the browser so progress can paint. Uses a MessageChannel rather than
// setTimeout because hidden tabs throttle timers to once a second or worse.
const sleep = (ms) => ms > 50 ? new Promise((r) => setTimeout(r, ms)) : new Promise((r) => { const c = new MessageChannel(); c.port1.onmessage = () => r(); c.port2.postMessage(0); });
let toastTimer;
export function toast(msg) { const t = $("#toast"); t.textContent = msg; t.classList.add("show"); clearTimeout(toastTimer); toastTimer = setTimeout(() => t.classList.remove("show"), 2600); }
// Turn engine errors into sentences a person can act on.
export function friendly(e) {
  const m = (e && e.message) || String(e);
  let x;
  if ((x = m.match(/page index (\d+) out of range \(document has (\d+) pages\)/))) return `Page ${+x[1] + 1} doesn't exist. This file has ${plural(+x[2], "page")}.`;
  if (/invalid page range "(.*)"/.test(m)) return `“${m.match(/invalid page range "(.*)"/)[1]}” isn't a page list I understand. Try something like 1-3, 5, last.`;
  if (/password does not open/.test(m)) return "That password doesn't open this file.";
  if (/unsupported encryption/.test(m)) return "This file uses a kind of encryption that isn't supported yet.";
  if (/no startxref|invalid PDF|not a PDF|too small/.test(m)) return "This doesn't look like a PDF file, or it's too damaged to repair.";
  if (/unrecognised format/.test(m)) return "That image isn't a PNG or JPEG.";
  if (/interlaced PNG/.test(m)) return "That PNG is interlaced. Re-save it without interlacing and try again.";
  return m;
}
// Persist small preferences per tool.
// Never persisted: passwords, file handles, image bytes, and page selections (those belong to one document).
const NO_PERSIST = new Set(["user", "owner", "for", "img", "imgName", "which", "custom", "ranges", "extract"]);
const prefs = { get: (k, d) => { try { const v = localStorage.getItem("foliopdf." + k); return v ? { ...d, ...JSON.parse(v) } : d; } catch { return d; } }, set: (k, v) => { try { const rest = Object.fromEntries(Object.entries(v).filter(([key]) => !NO_PERSIST.has(key))); localStorage.setItem("foliopdf." + k, JSON.stringify(rest)); } catch {} } };
// One options object per tool visit: re-renders keep every choice, storage keeps only the persistable ones.
const prefCache = new Map();
function pref(id, defaults) {
  if (!prefCache.has(id)) prefCache.set(id, new Proxy(prefs.get(id, defaults), { set(t, k, v) { t[k] = v; prefs.set(id, t); return true; } }));
  return prefCache.get(id);
}

// ---------------------------------------------------------------- tools
const TOOLS = [
  { id: "sign",      ico: "✍️", name: "Fill & Sign",       desc: "Type on any PDF, tick boxes, add the date, and draw or type your signature." },
  { id: "forms",     ico: "📋", name: "Fill a form",       desc: "Fill in the fields of a PDF form, then keep it editable or flatten it." },
  { id: "annotate",  ico: "💬", name: "Comment & mark up", desc: "Highlight, underline, draw, add notes, shapes and stamps." },
  { id: "redact",    ico: "⬛", name: "Redact",            desc: "Black out words, numbers or areas so they are truly gone from the file, not just covered." },
  { id: "extract",   ico: "📄", name: "Extract text",      desc: "Pull the text out of a PDF as a plain text file you can search or paste anywhere.", multi: true },
  { id: "merge",     ico: "🧩", name: "Merge PDFs",        desc: "Combine several files into one, in the order you choose.", multi: true },
  { id: "images",    ico: "🖼️", name: "Images to PDF",     desc: "Turn photos, scans and screenshots (JPEG, PNG, WebP, HEIC…) into one PDF.", multi: true, accept: "image" },
  { id: "toimages",  ico: "🏞️", name: "PDF to images",     desc: "Save every page as a PNG or JPEG picture." },
  { id: "split",     ico: "✂️", name: "Split PDF",         desc: "Break a file into parts, or pull out just the pages you need." },
  { id: "compress",  ico: "🗜️", name: "Compress PDF",      desc: "Make files smaller. Keep images as they are, or shrink scans and photos too.", multi: true },
  { id: "protect",   ico: "🔒", name: "Protect PDF",       desc: "Add a password and stop copying, printing or editing.", multi: true },
  { id: "unlock",    ico: "🔓", name: "Unlock PDF",        desc: "Remove a password from a file you have the password for.", multi: true },
  { id: "rotate",    ico: "🔄", name: "Rotate PDF",        desc: "Turn every page, or just the odd or even ones.", multi: true },
  { id: "delete",    ico: "🗑️", name: "Delete pages",      desc: "Remove pages you don't want." },
  { id: "organize",  ico: "🗂️", name: "Organize pages",    desc: "Drag pages into a new order, rotate, remove or add blank pages." },
  { id: "resize",    ico: "📐", name: "Page size",         desc: "Change pages to A4, Letter or any size, or scale them up or down.", multi: true },
  { id: "crop",      ico: "⌗", name: "Crop",               desc: "Trim the margins, or keep just one part of the page." },
  { id: "bookmarks", ico: "🔖", name: "Bookmarks",         desc: "Add, edit and remove the bookmarks readers see in the sidebar." },
  { id: "watermark", ico: "💧", name: "Watermark",         desc: "Stamp text like DRAFT or CONFIDENTIAL, or a logo, on every page.", multi: true },
  { id: "numbers",   ico: "🔢", name: "Page numbers",      desc: "Add page numbers wherever you like.", multi: true },
  { id: "info",      ico: "📝", name: "Edit document info", desc: "Change the title, author and keywords, or wipe hidden metadata.", multi: true },
  { id: "batch",     ico: "⚙️", name: "Batch & presets",   desc: "Save a set of steps and run it on many files at once.", multi: true },
];

let current = null;
let files = [];      // {file, bytes, doc, pages, err, needsPw, pw, enc, repaired}
let stage;
export const getStage = () => stage;
export const getFiles = () => files;
let engineOk = false;

// ---------------------------------------------------------------- boot
const BUILD = "__VERSION__"; // stamped at deploy time (short commit hash)
try {
  await init();
  engineOk = true;
  $("#status").textContent = "Ready · works offline · v" + version();
  $("#ver").textContent = "v" + version();
  if (BUILD !== "__VERSION__") $("#build").textContent = "· build " + BUILD;
} catch (e) {
  $("#status").textContent = "The PDF engine could not load. Try a current browser (Chrome, Edge, Firefox, Safari 15+).";
  $("#status").classList.add("bad");
}
// Installable app: register the service worker, offer "Install", and offer a
// reload when a newer build has been deployed.
if ("serviceWorker" in navigator && (location.protocol === "https:" || location.hostname === "localhost" || location.hostname === "127.0.0.1")) {
  navigator.serviceWorker.register("./sw.js").then((reg) => {
    const offer = () => {
      if (!reg.waiting || $("#update")) return;
      const bar = el("div", { class: "update", id: "update", role: "status" }, el("span", {}, "A new version of foliopdf is ready."), el("button", { class: "btn small primary", style: "font-size:14px;padding:8px 14px", onclick: () => { reg.waiting?.postMessage("SKIP_WAITING"); } }, "Update now"), el("button", { class: "iconbtn", "aria-label": "Later", onclick: () => bar.remove() }, "✕"));
      document.body.append(bar);
    };
    if (reg.waiting && navigator.serviceWorker.controller) offer();
    reg.addEventListener("updatefound", () => { const w = reg.installing; w?.addEventListener("statechange", () => { if (w.state === "installed" && navigator.serviceWorker.controller) offer(); }); });
    // Check for a new build now and whenever the app comes back to the foreground.
    reg.update().catch(() => {});
    document.addEventListener("visibilitychange", () => { if (document.visibilityState === "visible") reg.update().catch(() => {}); });
  }).catch(() => {});
  let reloading = false;
  navigator.serviceWorker.addEventListener("controllerchange", () => { if (reloading) return; reloading = true; location.reload(); });
}
let installPrompt = null;
window.addEventListener("beforeinstallprompt", (e) => { e.preventDefault(); installPrompt = e; $("#install").hidden = false; });
$("#install").onclick = async () => { if (!installPrompt) return; installPrompt.prompt(); const r = await installPrompt.userChoice; if (r?.outcome === "accepted") { $("#install").hidden = true; toast("Installed. foliopdf now opens like any other app, offline too."); } installPrompt = null; };
window.addEventListener("appinstalled", () => { $("#install").hidden = true; });
// Files opened with the installed app (file_handlers in the manifest).
if ("launchQueue" in window) window.launchQueue.setConsumer(async (params) => { const fs = []; for (const h of params.files || []) { try { fs.push(await h.getFile()); } catch {} } if (fs.length) { if (!current) open("sign"); addFiles(fs); } });

// Theme: Auto (follow system) → Light → Dark.
const THEMES = [["auto", "◐", "Auto"], ["light", "☀︎", "Light"], ["dark", "☾", "Dark"]];
function applyTheme(t) {
  const dark = t === "dark" || (t === "auto" && matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
  const [, ico, label] = THEMES.find((x) => x[0] === t) || THEMES[0];
  $("#theme-ico").textContent = ico; $("#theme-label").textContent = label;
  $("#theme").setAttribute("aria-label", `Colour theme: ${label}. Click to change.`);
  try { if (t === "auto") localStorage.removeItem("foliopdf.theme"); else localStorage.setItem("foliopdf.theme", t); } catch {}
}
let theme = (() => { try { return localStorage.getItem("foliopdf.theme") || "auto"; } catch { return "auto"; } })();
applyTheme(theme);
$("#theme").onclick = () => { theme = THEMES[(THEMES.findIndex((x) => x[0] === theme) + 1) % THEMES.length][0]; applyTheme(theme); };
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => { if (theme === "auto") applyTheme("auto"); });

const grid = $("#tools");
for (const t of TOOLS) grid.append(el("button", { class: "tool", "data-tool": t.id, onclick: () => open(t.id) }, el("div", { class: "ico", "aria-hidden": true }, t.ico), el("b", {}, t.name), el("span", {}, t.desc)));
$("#home-link").onclick = (e) => { e.preventDefault(); home(); };
$("#back").onclick = home;
window.addEventListener("hashchange", route);
document.addEventListener("keydown", (e) => { if (e.key === "Escape" && current && !(e.target instanceof Element && e.target.closest("input,textarea,select"))) home(); });
// Drop anywhere on a tool page.
let dragDepth = 0;
document.addEventListener("dragenter", (e) => { if (!current || !e.dataTransfer?.types?.includes("Files")) return; dragDepth++; document.body.classList.add("dragging"); });
document.addEventListener("dragleave", () => { if (--dragDepth <= 0) { dragDepth = 0; document.body.classList.remove("dragging"); } });
document.addEventListener("dragover", (e) => { if (current) e.preventDefault(); });
document.addEventListener("drop", (e) => { dragDepth = 0; document.body.classList.remove("dragging"); if (!current) return; e.preventDefault(); addFiles(e.dataTransfer.files); });

function route() { const id = location.hash.slice(1); if (TOOLS.some((t) => t.id === id)) { if (!current || current.id !== id) open(id, true); } else home(true); }
function freeAll() { files.forEach((f) => { try { f.doc?.free?.(); } catch {} f.thumbs?.destroy(); }); files = []; }
function home(fromRoute) { current = null; freeAll(); $("#view-tool").classList.remove("active"); $("#view-home").classList.add("active"); if (fromRoute !== true && location.hash) history.pushState(null, "", location.pathname); document.title = "foliopdf — free PDF tools that stay on your device"; }
function open(id, fromRoute) {
  current = TOOLS.find((t) => t.id === id); freeAll(); prefCache.clear();
  $("#view-home").classList.remove("active"); $("#view-tool").classList.add("active");
  $("#t-ico").textContent = current.ico; $("#t-title").textContent = current.name; $("#t-desc").textContent = current.desc;
  document.title = current.name + " — foliopdf";
  if (fromRoute !== true) history.pushState(null, "", "#" + id);
  stage = $("#stage"); renderStage(); window.scrollTo(0, 0);
}

// ---------------------------------------------------------------- files
const isMulti = () => !!current.multi;
export function dropzone() {
  const multi = isMulti();
  const has = files.length > 0;
  const dz = el("div", { class: "drop" + (has ? " compact" : ""), role: "button", tabindex: 0, "aria-label": has ? (multi ? "Add more PDF files" : "Choose a different PDF") : (multi ? "Drop PDF files here or choose files" : "Drop a PDF here or choose a file") });
  const noun = current?.accept === "image" ? "images" : multi ? "PDFs" : "PDF";
  if (!has) dz.append(el("div", { class: "big" }, `Drop your ${noun} here`), el("div", { class: "sub" }, multi ? "or choose files from your computer" : "or choose a file from your computer"), el("button", { class: "btn", tabindex: -1 }, multi ? "Choose files" : "Choose file"));
  else dz.append(el("div", { class: "big" }, multi ? "Drop more files anywhere on this page" : "Drop another file to replace this one"), el("button", { class: "btn small", tabindex: -1 }, multi ? "Add files" : "Choose a different file"));
  const pick = () => { if (!engineOk) return toast("The PDF engine isn't loaded."); const p = $("#picker"); p.multiple = multi; p.accept = current?.accept === "image" ? "image/*,.heic,.heif" : "application/pdf,.pdf"; p.value = ""; p.click(); };
  dz.onclick = pick; dz.onkeydown = (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); pick(); } };
  dz.ondragover = (e) => { e.preventDefault(); dz.classList.add("over"); };
  dz.ondragleave = () => dz.classList.remove("over");
  dz.ondrop = (e) => { e.preventDefault(); e.stopPropagation(); dz.classList.remove("over"); document.body.classList.remove("dragging"); addFiles(e.dataTransfer.files); };
  $("#picker").onchange = (e) => addFiles(e.target.files);
  return dz;
}
async function addFiles(list) {
  const all = [...list];
  const wantImages = current?.accept === "image";
  const isImage = (f) => /\.(jpe?g|png|webp|gif|bmp|heic|heif|avif|tiff?)$/i.test(f.name) || /^image\//.test(f.type);
  const pdfs = wantImages ? all.filter(isImage) : all.filter((f) => /\.pdf$/i.test(f.name) || f.type === "application/pdf");
  const rejected = all.length - pdfs.length;
  if (rejected) toast(wantImages ? (rejected === 1 ? "That file isn't an image." : `${rejected} files weren't images and were skipped.`) : rejected === 1 ? "Only PDF files can be added here." : `${rejected} files weren't PDFs and were skipped.`);
  if (!pdfs.length) return;
  const multi = isMulti();
  if (!multi) freeAll();
  const dupes = pdfs.filter((p) => files.some((f) => f.file.name === p.name && f.file.size === p.size));
  if (dupes.length && multi) toast(`${dupes[0].name} is already in the list.`);
  const fresh = multi ? pdfs.filter((p) => !dupes.includes(p)) : pdfs.slice(0, 1);
  for (const file of fresh) {
    const entry = { file, bytes: new Uint8Array(await file.arrayBuffer()), pw: "" };
    files.push(entry);
    if (wantImages) loadImageEntry(entry); else loadEntry(entry);
  }
  renderStage();
}
// Images are decoded by the browser (so HEIC/WebP/GIF work wherever the browser shows them) and handed to the engine as PNG or JPEG.
async function loadImageEntry(entry) {
  entry.image = true; entry.err = null; entry.doc = null;
  const url = URL.createObjectURL(entry.file);
  try {
    const img = await new Promise((res, rej) => { const i = new Image(); i.onload = () => res(i); i.onerror = () => rej(new Error("This image couldn't be opened. Browsers can't read some formats (for example HEIC on Windows); try converting it to JPEG or PNG first.")); i.src = url; });
    entry.w = img.naturalWidth; entry.h = img.naturalHeight;
    const isJpeg = /^image\/jpe?g$/.test(entry.file.type) || /\.jpe?g$/i.test(entry.file.name);
    if (isJpeg && !entry.file.name.match(/\.(heic|heif)$/i)) { entry.png = entry.bytes; entry.kind = "JPEG"; }
    else {
      const cv = el("canvas", { width: img.naturalWidth, height: img.naturalHeight }); cv.getContext("2d").drawImage(img, 0, 0);
      const blob = await new Promise((r) => cv.toBlob(r, "image/png")); entry.png = new Uint8Array(await blob.arrayBuffer()); entry.kind = "PNG";
    }
    entry.preview = url; entry.pages = 1; entry.doc = { free() {} };
  } catch (e) { entry.err = e.message; URL.revokeObjectURL(url); }
  renderStage();
}
function loadEntry(entry) {
  try { entry.doc?.free?.(); } catch {}
  entry.doc = null; entry.err = null; entry.needsPw = false;
  if (!entry.bytes.length) { entry.err = "This file is empty."; return; }
  try {
    entry.doc = entry.pw ? PdfDocument.loadWithPassword(entry.bytes, entry.pw) : PdfDocument.load(entry.bytes);
    entry.pages = entry.doc.pageCount();
    entry.enc = entry.doc.wasEncrypted() ? entry.doc.encryptionDescription() : null;
    entry.repaired = entry.doc.wasReconstructed();
    if (entry.pages === 0) { entry.doc.free(); entry.doc = null; entry.err = "This PDF has no pages."; }
    else { entry.thumbs?.destroy(); entry.thumbs = thumbnailer(entry.bytes, entry.pw); }
  } catch (e) {
    if (/password/i.test(e.message)) { entry.needsPw = true; entry.wrongPw = !!entry.pw; } else entry.err = friendly(e);
  }
}
export function fileList(opts = {}) {
  if (!files.length) return null;
  const ul = el("ul", { class: "files", "aria-label": "Files" });
  let dragFrom = null;
  files.forEach((f, i) => {
    let info;
    if (f.err) info = el("div", { class: "info err" }, f.err);
    else if (f.needsPw) {
      const inp = el("input", { type: "password", placeholder: "Password", "aria-label": `Password for ${f.file.name}`, autocomplete: "off" });
      const go = () => { f.pw = inp.value; loadEntry(f); renderStage(); };
      inp.onkeydown = (e) => { if (e.key === "Enter") go(); };
      info = el("div", { class: "info" + (f.wrongPw ? " err" : "") }, f.wrongPw ? "That password didn't work. " : "This file is password protected. ", el("div", { class: "pwrow" }, inp, el("button", { class: "btn small", onclick: go }, "Unlock")));
      setTimeout(() => { if (document.activeElement === document.body) inp.focus(); }, 0);
    } else if (f.image) info = el("div", { class: "info" }, f.w ? `${f.w} × ${f.h} px · ${kb(f.bytes.length)}` : "Reading…");
    else info = el("div", { class: "info" }, `${plural(f.pages, "page")} · ${kb(f.bytes.length)}` + (f.enc ? ` · protected (${f.enc})` : "") + (f.repaired ? " · repaired a damaged file" : ""));
    const acts = el("div", { class: "actions" });
    if (opts.reorder) {
      acts.append(el("button", { class: "iconbtn", title: "Move up", "aria-label": "Move up", disabled: i === 0, onclick: () => { [files[i - 1], files[i]] = [files[i], files[i - 1]]; renderStage(); } }, "↑"));
      acts.append(el("button", { class: "iconbtn", title: "Move down", "aria-label": "Move down", disabled: i === files.length - 1, onclick: () => { [files[i + 1], files[i]] = [files[i], files[i + 1]]; renderStage(); } }, "↓"));
    }
    acts.append(el("button", { class: "iconbtn", title: "Remove from list", "aria-label": `Remove ${f.file.name}`, onclick: () => { try { f.doc?.free?.(); } catch {} files.splice(i, 1); renderStage(); } }, "✕"));
    const fico = el("div", { class: "fico", "aria-hidden": true }, f.image ? "IMG" : "PDF");
    if (f.image && f.preview) { fico.textContent = ""; fico.classList.add("thumb"); fico.append(el("img", { src: f.preview, alt: "", style: "width:100%;height:100%;object-fit:cover;display:block" })); }
    if (f.thumbs) f.thumbs.render(0, 44).then((cv) => { if (cv && fico.isConnected) { fico.textContent = ""; fico.classList.add("thumb"); fico.append(cv); } });
    const li = el("li", { class: "file", draggable: opts.reorder ? "true" : null }, fico, el("div", { class: "meta" }, el("div", { class: "name", title: f.file.name }, f.file.name), info), acts);
    if (opts.reorder) {
      li.ondragstart = (e) => { dragFrom = i; e.dataTransfer.effectAllowed = "move"; e.dataTransfer.setData("text/plain", String(i)); };
      li.ondragover = (e) => { if (dragFrom == null) return; e.preventDefault(); e.stopPropagation(); li.classList.add("drag-over"); };
      li.ondragleave = () => li.classList.remove("drag-over");
      li.ondrop = (e) => { if (dragFrom == null) return; e.preventDefault(); e.stopPropagation(); const [m] = files.splice(dragFrom, 1); files.splice(i, 0, m); dragFrom = null; renderStage(); };
    }
    ul.append(li);
  });
  return ul;
}
export const ready = () => files.filter((f) => f.doc);
export function summaryLine() {
  const r = ready(); if (r.length < 2) return null;
  return el("div", { class: "summary" }, `${plural(r.length, "file")} · ${plural(r.reduce((a, f) => a + f.pages, 0), "page")} · ${kb(r.reduce((a, f) => a + f.bytes.length, 0))} total`);
}

// ---------------------------------------------------------------- ui bits
export function field(label, input, hint) { const h = hint == null ? null : (hint.nodeType ? hint : el("span", { class: "hint" }, hint)); return el("label", { class: "field" }, el("span", {}, label), input, h); }
export function check(label, checked, onchange) { const i = el("input", { type: "checkbox" }); i.checked = checked; i.onchange = () => onchange(i.checked); return el("label", { class: "check" }, i, el("span", {}, label)); }
export function segmented(options, value, onchange, label) {
  const s = el("div", { class: "seg", role: "group", "aria-label": label });
  const render = () => { s.innerHTML = ""; for (const [v, l] of options) s.append(el("button", { type: "button", class: v === value ? "on" : "", "aria-pressed": String(v === value), onclick: () => { value = v; onchange(v); render(); } }, l)); };
  render(); return s;
}
export const ANCHORS = ["top-left", "top-center", "top-right", "center-left", "center", "center-right", "bottom-left", "bottom-center", "bottom-right"];
export function anchorPicker(value, onchange) {
  const a = el("div", { class: "anchor", role: "group", "aria-label": "Position on the page" });
  const render = () => { a.innerHTML = ""; for (const n of ANCHORS) a.append(el("button", { type: "button", class: n === value ? "on" : "", title: n.replace("-", " "), "aria-label": n.replace("-", " "), "aria-pressed": String(n === value), onclick: () => { value = n; onchange(n); render(); } })); };
  render(); return a;
}
export const COLORS = [["#7a7a7a", [0.48, 0.48, 0.48], "grey"], ["#c0392b", [0.75, 0.22, 0.17], "red"], ["#0b63d6", [0.04, 0.39, 0.84], "blue"], ["#1a7f4b", [0.1, 0.5, 0.29], "green"], ["#111111", [0.07, 0.07, 0.07], "black"]];
export function swatches(value, onchange) {
  const w = el("div", { class: "swatches", role: "group", "aria-label": "Colour" });
  const render = () => { w.innerHTML = ""; for (const [hex, rgb, name] of COLORS) w.append(el("button", { type: "button", class: "swatch" + (hex === value ? " on" : ""), style: `background:${hex}`, title: name, "aria-label": name, "aria-pressed": String(hex === value), onclick: () => { value = hex; onchange(rgb, hex); render(); } })); };
  render(); return w;
}
export function passwordInput(value, oninput, label) {
  const i = el("input", { type: "password", value, autocomplete: "new-password", "aria-label": label, oninput: (e) => oninput(e.target.value) });
  const b = el("button", { type: "button", "aria-label": "Show password", onclick: () => { i.type = i.type === "password" ? "text" : "password"; b.textContent = i.type === "password" ? "Show" : "Hide"; } }, "Show");
  return el("div", { class: "pwfield" }, i, b);
}
export function cta(label, onclick, enabled = true, why) {
  const b = el("button", { class: "btn primary", disabled: !enabled, onclick }, label);
  return el("div", { class: "cta" }, b, !enabled && why ? el("p", { class: "why" }, why) : null);
}
export function notice(kind, text) { return el("div", { class: "notice " + kind }, text); }
// Live, plain-English feedback for a page-range input.
export function rangeField(label, value, pageCount, oninput, placeholder = "e.g. 1-3, 5, last") {
  const hint = el("span", { class: "hint" });
  const input = el("input", { type: "text", placeholder, value, spellcheck: "false", autocapitalize: "off" });
  const update = () => {
    const v = input.value.trim();
    input.classList.remove("bad"); hint.className = "hint";
    if (!v) { hint.textContent = pageCount ? `This file has ${plural(pageCount, "page")}. You can also type odd, even, or 4- for “page 4 to the end”.` : ""; return null; }
    try {
      const idx = Array.from(parsePageRanges(v, pageCount));
      if (!idx.length) { hint.textContent = "No pages selected."; return []; }
      const shown = idx.slice(0, 12).map((i) => i + 1).join(", ") + (idx.length > 12 ? ` … (${idx.length} pages)` : "");
      hint.textContent = `${plural(idx.length, "page")}: ${shown}`; hint.classList.add("ok");
      return idx;
    } catch (e) { hint.textContent = friendly(e); hint.classList.add("bad"); input.classList.add("bad"); return null; }
  };
  input.oninput = () => { oninput(input.value); update(); };
  update();
  return field(label, input, hint);
}

// ---------------------------------------------------------------- thumbnail size
const SIZES = [["s", "Small", 120], ["m", "Medium", 170], ["l", "Large", 250]];
let thumbSize = (() => { try { return localStorage.getItem("foliopdf.thumb") || "m"; } catch { return "m"; } })();
export const tilePx = () => (SIZES.find((x) => x[0] === thumbSize) || SIZES[1])[2];
export function sizeControl() {
  return el("div", { class: "sizectl" }, el("span", {}, "Page size"), segmented(SIZES.map(([k, l]) => [k, l]), thumbSize, (v) => { thumbSize = v; try { localStorage.setItem("foliopdf.thumb", v); } catch {} renderStage(); }, "Thumbnail size"));
}

// ---------------------------------------------------------------- page grid
/** Turns a set of 0-based indices into a compact 1-based range string. */
export function indicesToSpec(idx) {
  const a = [...new Set(idx)].sort((x, y) => x - y); const parts = [];
  for (let i = 0; i < a.length; i++) { let j = i; while (j + 1 < a.length && a[j + 1] === a[j] + 1) j++; parts.push(j > i + 1 ? `${a[i] + 1}-${a[j] + 1}` : j === i + 1 ? `${a[i] + 1}, ${a[j] + 1}` : `${a[i] + 1}`); i = j; }
  return parts.join(", ");
}
/**
 * A clickable grid of page thumbnails bound to a range input: clicking pages
 * updates the text, typing updates the highlights. `ordered` keeps click order
 * (for extraction); otherwise the spec is compacted into ranges.
 */
export function pageGrid(entry, rangeInput, onchange, opts = {}) {
  const grid = el("div", { class: "tiles select", role: "group", "aria-label": "Click pages to select them", style: `--tile:${tilePx()}px` });
  let selected = []; // 0-based, in click order
  const sync = () => { grid.querySelectorAll(".tile").forEach((t, i) => { const on = selected.includes(i); t.classList.toggle("on", on); t.setAttribute("aria-pressed", String(on)); const badge = t.querySelector(".order"); if (badge) badge.textContent = opts.ordered && on ? String(selected.indexOf(i) + 1) : ""; }); };
  const fromText = () => { try { const idx = rangeInput.value.trim() ? Array.from(parsePageRanges(rangeInput.value, entry.pages)) : []; selected = [...new Set(idx)]; } catch { selected = []; } sync(); };
  const infos = entry.doc.pages();
  for (let i = 0; i < entry.pages; i++) {
    const p = infos[i]; const w0 = p.mediaBox.x1 - p.mediaBox.x0, h0 = p.mediaBox.y1 - p.mediaBox.y0; const swap = p.rotation === "90" || p.rotation === "270";
    const [w, h] = swap ? [h0, w0] : [w0, h0]; const scale = (tilePx() - 24) / Math.max(w, h);
    const pg = el("div", { class: "pg", style: `width:${Math.round(w * scale)}px;height:${Math.round(h * scale)}px` }, String(i + 1));
    const tile = el("button", { type: "button", class: "tile", "aria-label": `Page ${i + 1}`, "aria-pressed": "false" }, pg, el("span", { class: "order" }), el("div", { class: "num" }, `Page ${i + 1}`));
    tile.onclick = () => { if (selected.includes(i)) selected = selected.filter((x) => x !== i); else selected.push(i); rangeInput.value = opts.ordered ? selected.map((x) => x + 1).join(", ") : indicesToSpec(selected); rangeInput.dispatchEvent(new Event("input", { bubbles: true })); sync(); };
    grid.append(tile);
    entry.thumbs?.render(i, Math.round(w * scale)).then((cv) => { if (cv && pg.isConnected) { pg.textContent = ""; pg.classList.add("thumb"); pg.append(cv); } });
  }
  rangeInput.addEventListener("input", fromText);
  fromText();
  const bar = el("div", { class: "row", style: "margin:10px 0 4px;gap:8px" },
    el("button", { type: "button", class: "btn small", onclick: () => { selected = [...Array(entry.pages).keys()]; rangeInput.value = opts.ordered ? selected.map((x) => x + 1).join(", ") : "all"; rangeInput.dispatchEvent(new Event("input", { bubbles: true })); } }, "Select all"),
    el("button", { type: "button", class: "btn small", onclick: () => { selected = []; rangeInput.value = ""; rangeInput.dispatchEvent(new Event("input", { bubbles: true })); } }, "Clear"),
    el("span", { class: "hint", style: "align-self:center" }, opts.ordered ? "Click pages in the order you want them." : "Click pages to select or deselect them, or type a range above."));
  void onchange;
  return el("div", {}, bar, grid, sizeControl());
}

// ---------------------------------------------------------------- run + results
let progressEl = null;
export function setProgress(i, n, name) { if (!progressEl) return; progressEl.bar.classList.add("det"); progressEl.bar.style.width = `${Math.round(100 * i / n)}%`; progressEl.text.textContent = `File ${i} of ${n}: ${name}`; }
export async function runJob(label, work) {
  stage.innerHTML = "";
  const bar = el("i"), text = el("p", {}, "Working on this device…");
  progressEl = { bar, text };
  put(el("div", { class: "panel result" }, el("div", { class: "done", "aria-hidden": true }, "⏳"), el("h3", {}, label + "…"), text, el("div", { class: "progress" }, bar)));
  await sleep(40);
  try {
    const t0 = performance.now();
    const outs = await work();
    showResults(outs, Math.round(performance.now() - t0));
  } catch (e) {
    stage.innerHTML = "";
    put(el("div", { class: "panel result" }, el("div", { class: "done", "aria-hidden": true }, "😕"), el("h3", {}, "That didn't work"), el("p", {}, friendly(e)), el("div", { class: "again" }, el("button", { class: "btn primary", onclick: () => renderStage() }, "Back to options"))));
  } finally { progressEl = null; }
}
function showResults(outs, ms) {
  stage.innerHTML = "";
  const list = el("div", { class: "outs" });
  const links = [];
  for (const o of outs) {
    const url = URL.createObjectURL(new Blob([o.data], { type: o.mime || "application/pdf" })); links.push([url, o.name]);
    const size = o.before != null
      ? el("div", { class: "info" }, `${plural(o.pages, "page")} · ${kb(o.before)} → ${kb(o.data.length)} `, el("b", {}, o.before > o.data.length * 1.02 ? `saved ${Math.round(100 - 100 * o.data.length / o.before)}%` : "already as small as it gets"))
      : el("div", { class: "info" }, `${plural(o.pages, "page")} · ${kb(o.data.length)}`);
    const meta = el("div", { class: "meta" }, el("div", { class: "name", title: o.name }, o.name), size);
    if (o.note) meta.append(el("div", { class: "info" }, o.note));
    const btns = el("div", { class: "obtns" });
    if (o.preview != null) btns.append(el("button", { class: "btn small", onclick: () => { navigator.clipboard?.writeText(o.preview).then(() => toast("Copied to the clipboard."), () => toast("Couldn't copy.")); } }, "Copy text"));
    else btns.append(el("a", { class: "btn small", href: url, target: "_blank", rel: "noopener", title: "Open in a new tab" }, "Preview"));
    btns.append(el("a", { class: "btn small primary", style: "font-size:14px;padding:8px 14px", href: url, download: o.name }, "Download"));
    const card = el("div", { class: "out" }, el("div", { class: "fico", "aria-hidden": true }, o.mime === "text/plain" ? "TXT" : o.mime === "application/zip" ? "ZIP" : /^image\//.test(o.mime || "") ? "IMG" : "PDF"), meta, btns);
    if (o.preview != null) card.append(el("pre", { class: "tpreview" }, o.preview.length > 4000 ? o.preview.slice(0, 4000) + "\n…" : o.preview));
    list.append(card);
  }
  const again = el("div", { class: "again" });
  if (outs.length > 1) again.append(el("button", { class: "btn primary", onclick: () => { links.forEach(([u, n], i) => setTimeout(() => { const a = el("a", { href: u, download: n }); document.body.append(a); a.click(); a.remove(); }, i * 300)); toast(`Downloading ${outs.length} files. Your browser may ask once to allow multiple downloads.`); } }, `Download all ${outs.length} files`));
  again.append(el("button", { class: "btn", onclick: () => { freeAll(); renderStage(); } }, "Do another"), el("button", { class: "btn", onclick: () => { renderStage(); } }, "Adjust options"), el("button", { class: "btn", onclick: home }, "All tools"));
  const total = outs.reduce((a, o) => a + o.data.length, 0);
  const tip = el("p", { class: "tip" }, "This tool is free and always will be. If it saved you a subscription, you're welcome to leave a tip: ", el("a", { href: "https://venmo.com/u/Keith-Adler-1", target: "_blank", rel: "noopener" }, "Venmo @Keith-Adler-1"), ".");
  put(el("div", { class: "panel result" }, el("div", { class: "done", "aria-hidden": true }, "✅"), el("h3", {}, outs.length === 1 ? "Your file is ready" : `${outs.length} files are ready`), el("p", {}, `Done in ${ms < 1000 ? ms + " ms" : (ms / 1000).toFixed(1) + " s"}, entirely on this device.${outs.length > 1 ? ` ${kb(total)} in total.` : ""}`), list, again, tip));
  window.scrollTo({ top: 0, behavior: "smooth" });
}
// Apply fn(doc, entry) to every loaded file; save each; collect outputs.
async function perFile(suffix, fn, saveOpts = {}, opts = {}) {
  const outs = []; const list = ready(); let n = 0;
  for (const f of list) {
    setProgress(++n, list.length, f.file.name);
    await sleep(0);
    const doc = f.pw ? PdfDocument.loadWithPassword(f.bytes, f.pw) : PdfDocument.load(f.bytes);
    try {
      const out = {};
      const r = await fn(doc, f, out);
      if (r === false) continue;
      const data = doc.save(saveOpts);
      outs.push({ name: `${stem(f.file.name)}${suffix}.pdf`, data, pages: doc.pageCount(), before: opts.showSavings ? f.bytes.length : undefined, ...out });
    } finally { doc.free(); }
  }
  if (!outs.length) throw new Error("Nothing to do.");
  return outs;
}

// ---------------------------------------------------------------- stages
// Append children to the stage, skipping null/false (DOM append would render the text "null").
export const put = (...nodes) => stage.append(...nodes.filter((x) => x != null && x !== false));
export function renderStage() { stage.innerHTML = ""; STAGES[current.id](); }
const allOrSpec = (which, custom) => which === "all" ? null : which === "custom" ? custom : which;

const STAGES = {
  merge() {
    put(dropzone(), fileList({ reorder: true }), summaryLine());
    const n = ready().length;
    if (files.length) put(el("p", { class: "summary" }, "Files are merged top to bottom. Drag a file, or use the arrows, to change the order."));
    put(cta(n >= 2 ? `Merge ${n} files` : "Merge PDFs", () => runJob("Merging", async () => {
      const list = ready(); const first = list[0];
      const out = new PdfDocument();
      try {
        let i = 0; for (const f of list) { setProgress(++i, list.length, f.file.name); await sleep(0); out.importPages(f.doc, null, null); }
        out.setMetadata(first.doc.metadata());
        return [{ name: stem(first.file.name) + "-merged.pdf", data: out.save({}), pages: out.pageCount() }];
      } finally { out.free(); }
    }), n >= 2, files.length ? (n === 1 ? "Add at least one more file to merge." : files.some((f) => f.needsPw) ? "Unlock the protected files first, or remove them." : null) : null));
  },
  split() {
    put(dropzone(), fileList());
    const f = ready()[0]; if (!f) return;
    const o = pref("split", { mode: "every", every: 1, ranges: "", extract: "" });
    const p = el("div", { class: "panel" }, el("h3", {}, "How do you want to split it?"), el("div", { class: "row" }, segmented([["every", "Into equal parts"], ["ranges", "Custom ranges"], ["extract", "Extract pages"]], o.mode, (v) => { o.mode = v; renderStage(); }, "Split mode")));
    const detail = el("div", { class: "row" });
    if (o.mode === "every") {
      const hint = el("span", { class: "hint ok" });
      const upd = () => { const e = Math.max(1, Math.min(f.pages, o.every || 1)); const parts = Math.ceil(f.pages / e); hint.textContent = parts === 1 ? "That's the whole document in one file. Use a smaller number." : `${plural(f.pages, "page")} → ${plural(parts, "file")} of up to ${plural(e, "page")} each`; hint.className = parts === 1 ? "hint bad" : "hint ok"; };
      detail.append(field("Pages per file", el("input", { type: "number", min: 1, max: f.pages, value: Math.min(o.every, f.pages), oninput: (e) => { o.every = Math.max(1, +e.target.value || 1); upd(); } }), hint)); upd();
    }
    if (o.mode === "ranges") detail.append(field("One range per file, separated by commas", el("input", { type: "text", placeholder: "e.g. 1-3, 4-9, 10-", value: o.ranges, spellcheck: "false", oninput: (e) => (o.ranges = e.target.value) }), `This file has ${plural(f.pages, "page")}. “10-” means page 10 to the end; “last” is the final page.`));
    let extractGrid = null;
    if (o.mode === "extract") { const rf = rangeField("Pages to keep, in one new file", o.extract, f.pages, (v) => (o.extract = v)); detail.append(rf); extractGrid = pageGrid(f, rf.querySelector("input"), null, { ordered: true }); }
    p.append(detail); if (extractGrid) p.append(extractGrid);
    put(p, cta("Split PDF", () => runJob("Splitting", async () => {
      const outs = [];
      const make = (spec, i, total) => { const d = PdfDocument.load(f.bytes); try { d.selectPages(spec); outs.push({ name: `${stem(f.file.name)}-${total > 1 ? "part" + (i + 1) : "pages"}.pdf`, data: d.save({}), pages: d.pageCount() }); } finally { d.free(); } };
      if (o.mode === "every") { const e = Math.max(1, Math.min(f.pages, o.every || 1)); const total = Math.ceil(f.pages / e); if (total < 2) throw new Error("Choose fewer pages per file, otherwise there is nothing to split."); for (let i = 0; i < total; i++) { const a = i * e + 1, b = Math.min(f.pages, a + e - 1); setProgress(i + 1, total, `part ${i + 1}`); await sleep(0); make(a === b ? `${a}` : `${a}-${b}`, i, total); } }
      else if (o.mode === "ranges") { const rs = o.ranges.split(",").map((s) => s.trim()).filter(Boolean); if (!rs.length) throw new Error("Type at least one page range, like 1-3."); rs.forEach((r, i) => make(r, i, rs.length)); }
      else { if (!o.extract.trim()) throw new Error("Type the pages to keep, like 1, 4-6."); make(o.extract, 0, 1); }
      return outs;
    })));
  },
  compress() {
    put(dropzone(), fileList(), summaryLine());
    if (!ready().length) return;
    const o = pref("compress", { level: 6, strip: false, images: "none" });
    const IMG = { none: null, print: { maxDpi: 300, quality: 85 }, ebook: { maxDpi: 150, quality: 75 }, screen: { maxDpi: 96, quality: 60 } };
    put(el("div", { class: "panel" }, el("h3", {}, "Options"),
      el("div", { class: "row" }, field("Pictures and scans", segmented([["none", "Keep as they are"], ["print", "Print quality (300 dpi)"], ["ebook", "Good (150 dpi)"], ["screen", "Small (96 dpi)"]], o.images, (v) => (o.images = v), "Image quality"), o.images === "none" ? "Lossless: images are untouched, so quality never drops." : "Oversized images are scaled down to the chosen resolution and saved as JPEG. This is what makes scanned documents small.")),
      el("div", { class: "row" }, segmented([[6, "Balanced (fast)"], [10, "Smallest (slower)"]], o.level, (v) => (o.level = v), "Compression level")), el("div", { class: "row" }, check("Also remove hidden metadata (author, editing history, thumbnails)", o.strip, (v) => (o.strip = v))),
      el("p", { class: "summary", style: "margin:12px 0 0" }, "Text and graphics are always repacked losslessly. Files that are already tight may not shrink much.")),
      cta(`Compress ${ready().length > 1 ? plural(ready().length, "file") : "PDF"}`, () => runJob("Compressing", () => perFile("-compressed", (doc, f, out) => { if (IMG[o.images]) { const r = doc.compressImages(IMG[o.images]); if (r.recompressed) out.note = `${r.recompressed} of ${plural(r.images, "image")} made smaller (${r.downsampled} scaled down).`; else if (r.images) out.note = "Images were already small enough."; } }, { compress: true, compressionLevel: o.level, stripMetadata: o.strip }, { showSavings: true }))));
  },
  protect() {
    put(dropzone(), fileList(), summaryLine());
    if (!ready().length) return;
    const o = pref("protect", { copy: true, print: true, modify: true }); o.user ??= ""; o.owner ??= ""; // passwords live only in memory
    const already = ready().filter((f) => f.enc).length;
    put(already ? notice("warn", already === 1 ? "This file is already protected. The new password will replace the old one." : `${already} of these files are already protected. New passwords will replace the old ones.`) : null,
      el("div", { class: "panel" }, el("h3", {}, "Passwords"), el("div", { class: "row" }, field("Password to open the file", passwordInput(o.user, (v) => (o.user = v), "Password to open"), "Leave empty if anyone may open it but you still want to limit what they can do."), field("Owner password (optional)", passwordInput(o.owner, (v) => (o.owner = v), "Owner password"), "Lets you remove the restrictions later. If empty, the open password is used.")),
        el("h3", { style: "margin-top:18px" }, "What readers may do"), el("div", { class: "row" }, check("Copy text and images", o.copy, (v) => (o.copy = v)), check("Print", o.print, (v) => (o.print = v)), check("Edit, add or remove pages", o.modify, (v) => (o.modify = v))),
        el("p", { class: "summary", style: "margin:12px 0 0" }, "Uses AES-256, the current standard supported by every modern reader. Note that permissions are honoured by well-behaved readers; only the open password truly keeps the contents private.")),
      cta("Protect PDF", () => runJob("Encrypting", () => { if (!o.user && !o.owner) throw new Error("Enter at least one password."); return perFile("-protected", () => {}, { encryption: { userPassword: o.user, ownerPassword: o.owner, permissions: { copy: o.copy, accessibility: o.copy, print: o.print, printHighQuality: o.print, modify: o.modify, annotate: o.modify, assemble: o.modify, fillForms: o.modify } } }); })));
  },
  unlock() {
    put(dropzone(), fileList(), summaryLine());
    const locked = files.filter((f) => f.needsPw);
    if (locked.length) put(notice("warn", locked.length === 1 ? "Type the password for the locked file and press Unlock." : "Type the password next to each locked file and press Unlock."));
    if (!ready().length) return;
    const n = ready().filter((f) => f.enc).length;
    const restricted = ready().filter((f) => f.enc && !f.doc.hasOwnerAccess?.()).length;
    if (!n) put(notice("warn", ready().length === 1 ? "This file isn't password protected, so there's nothing to remove." : "None of these files are password protected."));
    else put(notice("ok", n === 1 ? "Ready. The saved copy will open without a password and without restrictions." : `Ready. ${n} protected ${n === 1 ? "file" : "files"} will be saved without passwords or restrictions.`));
    void restricted;
    put(cta("Remove protection", () => runJob("Unlocking", () => perFile("-unlocked", (doc, f) => (f.enc ? undefined : false), {})), n > 0, n ? null : "Add a protected file first."));
  },
  rotate() {
    put(dropzone(), fileList(), summaryLine());
    if (!ready().length) return;
    const o = pref("rotate", { deg: 90, which: "all", custom: "" });
    const single = ready().length === 1 ? ready()[0] : null;
    const pgs = el("div", { class: "row" }, field("Which pages", segmented([["all", "All pages"], ["odd", "Odd pages"], ["even", "Even pages"], ["custom", "Specific pages"]], o.which, (v) => { o.which = v; renderStage(); }, "Which pages")));
    let grid = null;
    if (o.which === "custom") { const rf = rangeField("Pages", o.custom, single ? single.pages : 0, (v) => (o.custom = v)); pgs.append(rf); if (single) grid = pageGrid(single, rf.querySelector("input"), null); }
    put(el("div", { class: "panel" }, el("h3", {}, "Rotate"), el("div", { class: "row" }, field("Direction", segmented([[90, "↻ 90° clockwise"], [180, "180°"], [270, "↺ 90° anticlockwise"]], o.deg, (v) => (o.deg = v), "Rotation"))), pgs, grid),
      cta("Rotate PDF", () => runJob("Rotating", () => { if (o.which === "custom" && !o.custom.trim()) throw new Error("Type which pages to rotate, like 1, 3-5."); return perFile("-rotated", (doc) => { doc.rotatePages(allOrSpec(o.which, o.custom), o.deg); }); })));
  },
  delete() {
    put(dropzone(), fileList());
    const f = ready()[0]; if (!f) return;
    const o = { pages: STAGES.delete.last ?? "" };
    const rf = rangeField("Pages to delete", o.pages, f.pages, (v) => { o.pages = v; STAGES.delete.last = v; }, "e.g. 2, 5-7, last");
    put(el("div", { class: "panel" }, el("h3", {}, "Which pages should go?"), el("div", { class: "row" }, rf), pageGrid(f, rf.querySelector("input"), null)),
      cta("Delete pages", () => runJob("Removing pages", () => {
        if (!o.pages.trim()) throw new Error("Type the pages to delete, like 2, 5-7.");
        const idx = Array.from(parsePageRanges(o.pages, f.pages));
        if (new Set(idx).size >= f.pages) throw new Error("That would delete every page. Keep at least one.");
        return perFile("-edited", (doc) => { doc.deletePages(o.pages); });
      })));
  },
  organize() {
    put(dropzone(), fileList());
    const f = ready()[0]; if (!f) return;
    if (!STAGES.organize.o || STAGES.organize.o.for !== f) STAGES.organize.o = { for: f, tiles: f.doc.pages().map((p, i) => ({ src: i, rot: 0, del: false, w: p.mediaBox.x1 - p.mediaBox.x0, h: p.mediaBox.y1 - p.mediaBox.y0, swap: p.rotation === "90" || p.rotation === "270" })) };
    const addBlank = (after) => { const ref = o.tiles[Math.max(0, Math.min(o.tiles.length - 1, after))]; const w = ref ? (ref.swap ? ref.h : ref.w) : 612, h = ref ? (ref.swap ? ref.w : ref.h) : 792; o.tiles.splice(after + 1, 0, { src: -1, blank: true, rot: 0, del: false, w, h, swap: false }); renderStage(); };
    const o = STAGES.organize.o;
    const grid = el("div", { class: "tiles", role: "list", "aria-label": "Pages", style: `--tile:${tilePx()}px` });
    let dragFrom = null;
    const move = (from, to) => { if (to < 0 || to >= o.tiles.length) return; const [m] = o.tiles.splice(from, 1); o.tiles.splice(to, 0, m); renderStage(); };
    o.tiles.forEach((t, i) => {
      const [w, h] = t.swap ? [t.h, t.w] : [t.w, t.h];
      const scale = (tilePx() - 24) / Math.max(w, h);
      const label = t.blank ? "Blank" : `Page ${t.src + 1}`;
      const pg = el("div", { class: "pg" + (t.blank ? " blank" : ""), style: `width:${Math.round(w * scale)}px;height:${Math.round(h * scale)}px;transform:rotate(${t.rot}deg)` }, t.blank ? "" : String(t.src + 1));
      if (!t.blank) f.thumbs?.render(t.src, Math.round(w * scale)).then((cv) => { if (cv && pg.isConnected) { pg.textContent = ""; pg.classList.add("thumb"); pg.append(cv); } });
      const tile = el("div", { class: "tile" + (t.del ? " deleted" : ""), draggable: "true", role: "listitem", "aria-label": `${label}${t.rot ? `, rotated ${t.rot} degrees` : ""}${t.del ? ", removed" : ""}` }, pg, el("div", { class: "num" }, t.del ? "Removed" : `${label}${t.rot ? ` · ${t.rot}°` : ""}`),
        el("div", { class: "tacts" },
          el("button", { class: "iconbtn", title: "Move left", "aria-label": "Move left", disabled: i === 0, onclick: () => move(i, i - 1) }, "◀"),
          el("button", { class: "iconbtn", title: "Rotate", "aria-label": "Rotate 90 degrees", onclick: () => { t.rot = (t.rot + 90) % 360; renderStage(); } }, "↻"),
          el("button", { class: "iconbtn", title: t.del ? "Restore" : "Remove", "aria-label": t.del ? "Restore page" : "Remove page", onclick: () => { if (t.blank) o.tiles.splice(i, 1); else t.del = !t.del; renderStage(); } }, t.del ? "↩" : "✕"),
          el("button", { class: "iconbtn", title: "Insert a blank page after this one", "aria-label": "Insert a blank page after this one", onclick: () => addBlank(i) }, "＋"),
          el("button", { class: "iconbtn", title: "Move right", "aria-label": "Move right", disabled: i === o.tiles.length - 1, onclick: () => move(i, i + 1) }, "▶")));
      tile.ondragstart = (e) => { dragFrom = i; tile.classList.add("dragging"); e.dataTransfer.effectAllowed = "move"; e.dataTransfer.setData("text/plain", String(i)); };
      tile.ondragend = () => tile.classList.remove("dragging");
      tile.ondragover = (e) => { if (dragFrom == null) return; e.preventDefault(); e.stopPropagation(); tile.classList.add("over"); };
      tile.ondragleave = () => tile.classList.remove("over");
      tile.ondrop = (e) => { if (dragFrom == null) return; e.preventDefault(); e.stopPropagation(); if (dragFrom !== i) move(dragFrom, i); dragFrom = null; };
      grid.append(tile);
    });
    const keep = o.tiles.filter((t) => !t.del);
    const real = keep.filter((t) => !t.blank);
    const changed = o.tiles.some((t, i) => t.src !== i || t.rot || t.del || t.blank);
    put(el("div", { class: "panel" }, el("h3", {}, "Drag pages into order. Rotate, remove or add blank pages with the buttons under each page."), grid,
      el("div", { class: "row", style: "margin-top:14px" }, el("button", { class: "btn small", onclick: () => { o.tiles.reverse(); renderStage(); } }, "Reverse order"), el("button", { class: "btn small", onclick: () => { o.tiles.forEach((t) => (t.rot = (t.rot + 90) % 360)); renderStage(); } }, "Rotate all"), el("button", { class: "btn small", onclick: () => addBlank(o.tiles.length - 1) }, "Add a blank page at the end"), el("button", { class: "btn small", onclick: () => { STAGES.organize.o = null; renderStage(); } }, "Start over")), sizeControl()),
      cta(keep.length === o.tiles.length ? `Save ${plural(keep.length, "page")}` : `Save ${keep.length} of ${plural(o.tiles.length, "page")}`, () => runJob("Rebuilding", () => perFile("-organized", (doc) => {
        doc.selectPages(real.map((t) => t.src + 1).join(","));
        keep.forEach((t, k) => { if (t.blank) doc.insertBlankPages(k, 1, t.w, t.h); });
        keep.forEach((t, k) => { if (t.rot) doc.rotatePages(String(k + 1), t.rot); });
      })), real.length > 0 && changed, real.length === 0 ? "Keep at least one page from the file." : !changed ? "Nothing has changed yet." : null));
  },
  resize() {
    put(dropzone(), fileList(), summaryLine());
    if (!ready().length) return;
    const o = pref("resize", { kind: "size", size: "a4", landscape: false, mode: "fit", percent: 100, which: "all", custom: "" });
    const single = ready().length === 1 ? ready()[0] : null;
    const first = ready()[0];
    const sizes = first.doc.pages().map((p) => { const w = p.mediaBox.x1 - p.mediaBox.x0, h = p.mediaBox.y1 - p.mediaBox.y0; return p.rotation === "90" || p.rotation === "270" ? [h, w] : [w, h]; });
    const distinct = [...new Set(sizes.map(([w, h]) => sizeLabel(w, h)))];
    const p = el("div", { class: "panel" }, el("h3", {}, "New size"),
      el("p", { class: "summary", style: "margin:0 0 12px" }, distinct.length === 1 ? `${single ? "This file" : first.file.name} is currently ${distinct[0]}.` : `${single ? "This file" : first.file.name} mixes ${distinct.length} page sizes (${distinct.slice(0, 3).join(", ")}${distinct.length > 3 ? ", …" : ""}).`),
      el("div", { class: "row" }, field("Change", segmented([["size", "To a standard size"], ["scale", "By a percentage"]], o.kind, (v) => { o.kind = v; renderStage(); }, "Resize mode"))));
    if (o.kind === "size") p.append(
      el("div", { class: "row" }, field("Size", segmented([["a4", "A4"], ["letter", "Letter"], ["legal", "Legal"], ["a3", "A3"], ["a5", "A5"], ["tabloid", "Tabloid"]], o.size, (v) => (o.size = v), "Page size")), field("Orientation", segmented([[false, "Portrait"], [true, "Landscape"]], o.landscape, (v) => (o.landscape = v), "Orientation"))),
      el("div", { class: "row" }, field("If the shape is different", segmented([["fit", "Fit (keep everything, may add margins)"], ["fill", "Fill (crop the edges)"], ["stretch", "Stretch"]], o.mode, (v) => (o.mode = v), "Fit mode"))));
    else p.append(el("div", { class: "row" }, field("Scale", el("input", { type: "number", min: 10, max: 400, value: o.percent, oninput: (e) => (o.percent = Math.min(400, Math.max(10, +e.target.value || 100))) }), "In percent. 50 halves the page and everything on it; 200 doubles it.")));
    const pgs = el("div", { class: "row" }, field("Which pages", segmented([["all", "All pages"], ["custom", "Specific pages"]], o.which, (v) => { o.which = v; renderStage(); }, "Which pages")));
    let grid = null;
    if (o.which === "custom") { const rf = rangeField("Pages", o.custom, single ? single.pages : 0, (v) => (o.custom = v)); pgs.append(rf); if (single) grid = pageGrid(single, rf.querySelector("input"), null); }
    p.append(pgs, grid);
    put(p, cta(o.kind === "size" ? `Resize to ${o.size.toUpperCase().replace("LETTER", "Letter").replace("LEGAL", "Legal").replace("TABLOID", "Tabloid")}` : `Scale to ${o.percent}%`, () => runJob("Resizing", () => {
      if (o.which === "custom" && !o.custom.trim()) throw new Error("Type which pages to resize, like 1, 3-5.");
      return perFile("-resized", (doc) => {
        const spec = o.which === "all" ? null : o.custom;
        if (o.kind === "size") doc.resizePages(spec, { size: o.size + (o.landscape ? "-landscape" : ""), mode: o.mode });
        else doc.scalePages(spec, o.percent / 100);
      });
    })));
  },
  watermark() {
    put(dropzone(), fileList(), summaryLine());
    if (!ready().length) return;
    const o = pref("watermark", { kind: "text", text: "DRAFT", size: 72, opacity: 0.3, rotation: 45, color: [0.48, 0.48, 0.48], hex: "#7a7a7a", pos: "center", width: 120, under: false, which: "all", custom: "" });
    o.img ??= STAGES.watermark.img ?? null; o.imgName ??= STAGES.watermark.imgName ?? "";
    const pv = el("div", { class: "preview", "aria-hidden": true }, el("div", { class: "lines" }));
    const wm = el("div", { class: "wm" });
    pv.append(wm);
    const updatePreview = () => {
      const [ax, ay] = { "top-left": [8, 8], "top-center": [50, 8], "top-right": [92, 8], "center-left": [8, 50], center: [50, 50], "center-right": [92, 50], "bottom-left": [8, 92], "bottom-center": [50, 92], "bottom-right": [92, 92] }[o.pos];
      const tx = ax < 50 ? "0%" : ax > 50 ? "-100%" : "-50%", ty = ay < 50 ? "0%" : ay > 50 ? "-100%" : "-50%";
      wm.style.left = ax + "%"; wm.style.top = ay + "%"; wm.style.opacity = o.opacity;
      if (o.kind === "text") { wm.textContent = o.text.replace("{page}", "1").replace("{pages}", "9") || " "; wm.style.color = o.hex; wm.style.fontSize = Math.max(6, o.size / 4.5) + "px"; wm.style.transform = `translate(${tx}, ${ty}) rotate(${-o.rotation}deg)`; wm.style.background = ""; wm.style.width = ""; wm.style.height = ""; }
      else { wm.textContent = ""; wm.style.background = o.img ? "var(--accent)" : "#ccc"; wm.style.width = Math.min(110, o.width / 4.5) + "px"; wm.style.height = Math.min(110, o.width / 4.5) * 0.6 + "px"; wm.style.borderRadius = "3px"; wm.style.transform = `translate(${tx}, ${ty})`; }
    };
    const p = el("div", { class: "panel" }, el("h3", {}, "Watermark"), el("div", { class: "row" }, field("Type", segmented([["text", "Text"], ["image", "Logo or image"]], o.kind, (v) => { o.kind = v; renderStage(); }, "Watermark type"))));
    const controls = el("div", { style: "flex:1;min-width:0" });
    if (o.kind === "text") {
      controls.append(el("div", { class: "row" }, field("Text", el("input", { type: "text", value: o.text, maxlength: 80, oninput: (e) => { o.text = e.target.value; updatePreview(); } }), "Tip: {page} and {pages} become the page number and total."), field(`Size`, el("input", { type: "range", min: 12, max: 160, value: o.size, "aria-label": "Text size", oninput: (e) => { o.size = +e.target.value; updatePreview(); } })), field("Opacity", el("input", { type: "range", min: 0.05, max: 1, step: 0.05, value: o.opacity, "aria-label": "Opacity", oninput: (e) => { o.opacity = +e.target.value; updatePreview(); } }))),
        el("div", { class: "row" }, field("Angle", segmented([[0, "Straight"], [45, "Diagonal"], [90, "Vertical"]], o.rotation, (v) => { o.rotation = v; updatePreview(); }, "Angle")), field("Colour", swatches(o.hex, (rgb, hex) => { o.color = rgb; o.hex = hex; updatePreview(); })), field("Position", anchorPicker(o.pos, (v) => { o.pos = v; updatePreview(); }))));
    } else {
      const pick = el("button", { class: "btn", onclick: () => { $("#imgpicker").value = ""; $("#imgpicker").onchange = async (e) => { const f = e.target.files[0]; if (!f) return; STAGES.watermark.img = new Uint8Array(await f.arrayBuffer()); STAGES.watermark.imgName = f.name; o.img = STAGES.watermark.img; o.imgName = f.name; renderStage(); }; $("#imgpicker").click(); } }, o.img ? `Change image (${o.imgName})` : "Choose a PNG or JPEG");
      controls.append(el("div", { class: "row" }, field("Image", pick, "PNG transparency is kept."), field("Width on the page", el("input", { type: "range", min: 30, max: 600, value: o.width, "aria-label": "Image width", oninput: (e) => { o.width = +e.target.value; updatePreview(); } }), "Drag right for larger. 72 = one inch."), field("Opacity", el("input", { type: "range", min: 0.05, max: 1, step: 0.05, value: o.opacity, "aria-label": "Opacity", oninput: (e) => { o.opacity = +e.target.value; updatePreview(); } }))),
        el("div", { class: "row" }, field("Position", anchorPicker(o.pos, (v) => { o.pos = v; updatePreview(); }))));
    }
    const single = ready().length === 1 ? ready()[0] : null;
    const pgs = el("div", { class: "row" }, field("Which pages", segmented([["all", "All pages"], ["first", "First page only"], ["custom", "Specific pages"]], o.which, (v) => { o.which = v; renderStage(); }, "Which pages")));
    let wgrid = null;
    if (o.which === "custom") { const rf = rangeField("Pages", o.custom, single ? single.pages : 0, (v) => (o.custom = v)); pgs.append(rf); if (single) wgrid = pageGrid(single, rf.querySelector("input"), null); }
    controls.append(pgs, wgrid, el("div", { class: "row" }, check("Place behind the page content instead of on top", o.under, (v) => (o.under = v))));
    p.append(el("div", { class: "row", style: "margin-top:14px;flex-wrap:nowrap;align-items:flex-start" }, controls, pv));
    updatePreview();
    put(p, cta("Add watermark", () => runJob("Stamping", () => perFile("-watermarked", (doc) => {
      const spec = o.which === "all" ? null : o.which === "first" ? "1" : o.custom;
      if (o.which === "custom" && !o.custom.trim()) throw new Error("Type which pages to stamp, like 1, 3-5.");
      if (o.kind === "text") { if (!o.text.trim()) throw new Error("Type the watermark text."); doc.stampText(spec, { text: o.text, size: o.size, opacity: o.opacity, rotation: o.rotation, color: o.color, position: o.pos, under: o.under }); }
      else { if (!o.img) throw new Error("Choose an image first."); doc.stampImage(spec, o.img, { width: o.width, opacity: o.opacity, position: o.pos, under: o.under }); }
    }))));
  },
  numbers() {
    put(dropzone(), fileList(), summaryLine());
    if (!ready().length) return;
    const o = pref("numbers", { format: "{page} / {pages}", pos: "bottom-center", size: 10, start: 1, which: "all", custom: "" });
    const single = ready().length === 1 ? ready()[0] : null;
    const pv = el("div", { class: "preview", "aria-hidden": true }, el("div", { class: "lines" }));
    const wm = el("div", { class: "wm", style: "font-weight:500" }); pv.append(wm);
    const updatePreview = () => {
      const [ax, ay] = { "top-left": [8, 5], "top-center": [50, 5], "top-right": [92, 5], "center-left": [8, 50], center: [50, 50], "center-right": [92, 50], "bottom-left": [8, 95], "bottom-center": [50, 95], "bottom-right": [92, 95] }[o.pos];
      wm.style.left = ax + "%"; wm.style.top = ay + "%"; wm.style.transform = `translate(${ax < 50 ? "0%" : ax > 50 ? "-100%" : "-50%"}, ${ay < 50 ? "0%" : ay > 50 ? "-100%" : "-50%"})`;
      wm.style.fontSize = Math.max(6, o.size / 1.6) + "px"; wm.style.color = "#333";
      wm.textContent = o.format.replace("{page}", String(o.start)).replace("{pages}", String(o.start + (single ? single.pages : 9) - 1));
    };
    const pgs = el("div", { class: "row" }, field("Which pages", segmented([["all", "All pages"], ["skipfirst", "All except the first"], ["custom", "Specific pages"]], o.which, (v) => { o.which = v; renderStage(); }, "Which pages")));
    let ngrid = null;
    if (o.which === "custom") { const rf = rangeField("Pages", o.custom, single ? single.pages : 0, (v) => (o.custom = v)); pgs.append(rf); if (single) ngrid = pageGrid(single, rf.querySelector("input"), null); }
    const controls = el("div", { style: "flex:1;min-width:0" },
      el("div", { class: "row" }, field("Style", segmented([["{page}", "1"], ["{page} / {pages}", "1 / 12"], ["Page {page} of {pages}", "Page 1 of 12"], ["- {page} -", "- 1 -"]], o.format, (v) => { o.format = v; updatePreview(); }, "Number style")), field("Position", anchorPicker(o.pos, (v) => { o.pos = v; updatePreview(); }))),
      el("div", { class: "row" }, field("Size", el("input", { type: "number", min: 6, max: 36, value: o.size, oninput: (e) => { o.size = Math.min(36, Math.max(6, +e.target.value || 10)); updatePreview(); } })), field("First number", el("input", { type: "number", min: 0, value: o.start, oninput: (e) => { o.start = Math.max(0, +e.target.value || 0); updatePreview(); } }), "Useful when a cover page shouldn't count.")),
      pgs, ngrid);
    updatePreview();
    put(el("div", { class: "panel" }, el("h3", {}, "Page numbers"), el("div", { class: "row", style: "flex-wrap:nowrap;align-items:flex-start" }, controls, pv)),
      cta("Add page numbers", () => runJob("Numbering", () => perFile("-numbered", (doc) => {
        const n = doc.pageCount();
        const spec = o.which === "all" ? null : o.which === "skipfirst" ? (n > 1 ? "2-" : null) : o.custom;
        if (o.which === "custom" && !o.custom.trim()) throw new Error("Type which pages to number, like 2-.");
        if (o.which === "skipfirst" && n === 1) return false;
        doc.addPageNumbers(spec, { format: o.format, position: o.pos, size: o.size, startAt: o.start });
      }))));
  },
  info() {
    put(dropzone(), fileList(), summaryLine());
    const list = ready(); const f = list[0]; if (!f) return;
    const m = f.doc.metadata();
    const o = (STAGES.info.o ??= {}); if (o.for !== f) Object.assign(o, { for: f, title: m.title ?? "", author: m.author ?? "", subject: m.subject ?? "", keywords: m.keywords ?? "", strip: false });
    const inp = (k, ph) => el("input", { type: "text", value: o[k], placeholder: ph, oninput: (e) => (o[k] = e.target.value) });
    put(el("div", { class: "panel" }, el("h3", {}, list.length > 1 ? `Document information (applied to all ${list.length} files; fields shown are from ${f.file.name})` : "Document information"),
      el("div", { class: "row" }, field("Title", inp("title", "Untitled")), field("Author", inp("author", ""))), el("div", { class: "row" }, field("Subject", inp("subject", "")), field("Keywords", inp("keywords", "comma, separated"))),
      el("div", { class: "row" }, check("Wipe all hidden metadata first (creating app, dates, XMP, thumbnails)", o.strip, (v) => (o.strip = v))),
      (m.creator || m.producer) ? el("p", { class: "summary", style: "margin:10px 0 0" }, `Currently: created with ${m.creator || "an unknown app"}, produced by ${m.producer || "an unknown library"}.`) : null,
      el("p", { class: "summary", style: "margin:6px 0 0" }, "Clearing a field removes it from the file.")),
      cta("Save changes", () => runJob("Updating", () => perFile("-edited", (doc) => { if (o.strip) doc.stripMetadata(); doc.setMetadata({ title: o.title, author: o.author, subject: o.subject, keywords: o.keywords }); }, { stripMetadata: false }))));
  },
  images() {
    put(dropzone(), fileList({ reorder: true }));
    const list = ready(); if (!files.length) return;
    const o = pref("images", { size: "auto", margin: 0 });
    if (files.length) put(el("p", { class: "summary" }, "Pages are added top to bottom. Drag to reorder."));
    put(el("div", { class: "panel" }, el("h3", {}, "Page size"), el("div", { class: "row" }, field("Size", segmented([["auto", "Same as each image"], ["a4", "A4"], ["letter", "Letter"], ["legal", "Legal"]], o.size, (v) => { o.size = v; renderStage(); }, "Page size"), o.size === "auto" ? "Each page is exactly the image, at 150 dpi (a 1500 px wide photo makes a 10 inch wide page)." : "Images are fitted inside the page, turned to landscape when they are wider than tall."),
      o.size === "auto" ? null : field("Margin", segmented([[0, "None"], [36, "½ inch"], [72, "1 inch"]], o.margin, (v) => (o.margin = v), "Margin")))),
      cta(list.length > 1 ? `Make a PDF from ${plural(list.length, "image")}` : "Make PDF", () => runJob("Converting", async () => {
        const doc = imagesToPdf(list.map((f) => f.png), o.size === "auto" ? { dpi: 150 } : { size: o.size, margin: o.margin });
        try { return [{ name: (list.length === 1 ? stem(list[0].file.name) : "images") + ".pdf", data: doc.save({}), pages: doc.pageCount() }]; } finally { doc.free(); }
      }), list.length > 0, files.some((f) => !f.doc && !f.err) ? "Still reading the images…" : null));
  },
  toimages() {
    put(dropzone(), fileList());
    const f = ready()[0]; if (!f) return;
    const o = pref("toimages", { format: "png", dpi: 150, which: "all", custom: "" });
    const pgs = el("div", { class: "row" }, field("Which pages", segmented([["all", "All pages"], ["custom", "Specific pages"]], o.which, (v) => { o.which = v; renderStage(); }, "Which pages")));
    let grid = null;
    if (o.which === "custom") { const rf = rangeField("Pages", o.custom, f.pages, (v) => (o.custom = v)); pgs.append(rf); grid = pageGrid(f, rf.querySelector("input"), null); }
    put(el("div", { class: "panel" }, el("h3", {}, "Options"), el("div", { class: "row" }, field("Format", segmented([["png", "PNG (sharp, larger)"], ["jpeg", "JPEG (smaller)"]], o.format, (v) => (o.format = v), "Image format")), field("Resolution", segmented([[72, "72 dpi (screen)"], [150, "150 dpi"], [300, "300 dpi (print)"]], o.dpi, (v) => (o.dpi = v), "Resolution"))), pgs, grid,
      el("p", { class: "summary", style: "margin:12px 0 0" }, "Several pages are delivered together as a ZIP file; a single page as one image.")),
      cta("Convert to images", () => runJob("Rendering", async () => {
        if (o.which === "custom" && !o.custom.trim()) throw new Error("Type which pages to convert, like 1, 3-5.");
        const idx = o.which === "all" ? [...Array(f.pages).keys()] : Array.from(parsePageRanges(o.custom, f.pages));
        const { openPdf } = await import("./thumbs.js?v=dev"); const pdf = await openPdf(f.bytes, f.pw); if (!pdf) throw new Error("This PDF couldn't be rendered.");
        const items = []; let k = 0;
        try {
          for (const i of idx) {
            setProgress(++k, idx.length, `page ${i + 1}`); await sleep(0);
            const page = await pdf.getPage(i + 1); const vp = page.getViewport({ scale: o.dpi / 72 });
            const cv = el("canvas", { width: Math.ceil(vp.width), height: Math.ceil(vp.height) });
            await page.render({ canvasContext: cv.getContext("2d", { alpha: false }), viewport: vp, background: "#ffffff", annotationMode: 1 }).promise; page.cleanup();
            const blob = await new Promise((r) => cv.toBlob(r, o.format === "png" ? "image/png" : "image/jpeg", 0.9));
            items.push({ name: `${stem(f.file.name)}-page-${String(i + 1).padStart(String(f.pages).length, "0")}.${o.format === "png" ? "png" : "jpg"}`, data: new Uint8Array(await blob.arrayBuffer()) });
          }
        } finally { try { pdf.loadingTask?.destroy?.(); } catch {} }
        if (items.length === 1) return [{ name: items[0].name, data: items[0].data, pages: 1, mime: o.format === "png" ? "image/png" : "image/jpeg", note: "1 image" }];
        return [{ name: `${stem(f.file.name)}-pages.zip`, data: zip(items), pages: items.length, mime: "application/zip", note: `${plural(items.length, "image")} in a ZIP file` }];
      })));
  },
  sign() { editorStage("sign", "fill", "-signed", "Sign & save"); },
  redact() { editorStage("redact", "redact", "-redacted", "Apply redactions"); },
  crop() { editorStage("crop", "crop", "-cropped", "Crop"); },
  bookmarks() {
    put(dropzone(), fileList());
    const f = ready()[0]; if (!f) return;
    const st = (STAGES.bookmarks.o ??= {}); if (st.for !== f) Object.assign(st, { for: f, items: f.doc.bookmarks() });
    const items = st.items;
    const panel = el("div", { class: "panel" }, el("h3", {}, items.length ? "Bookmarks" : "This file has no bookmarks yet"));
    const render = (list, depth, parent) => {
      const ul = el("ul", { class: "bmlist", style: `margin-left:${depth ? 18 : 0}px` });
      list.forEach((b, i) => {
        const li = el("li", { class: "bm" },
          el("input", { type: "text", value: b.title, placeholder: "Title", "aria-label": "Bookmark title", oninput: (e) => (b.title = e.target.value) }),
          el("label", { class: "bmpage" }, "page ", el("input", { type: "number", min: 1, max: f.pages, value: b.page != null ? b.page + 1 : "", "aria-label": "Page number", oninput: (e) => { const v = +e.target.value; b.page = v >= 1 && v <= f.pages ? v - 1 : null; } })),
          el("div", { class: "actions" },
            el("button", { class: "iconbtn", title: "Move up", "aria-label": "Move up", disabled: i === 0, onclick: () => { [list[i - 1], list[i]] = [list[i], list[i - 1]]; renderStage(); } }, "↑"),
            el("button", { class: "iconbtn", title: "Move down", "aria-label": "Move down", disabled: i === list.length - 1, onclick: () => { [list[i + 1], list[i]] = [list[i], list[i + 1]]; renderStage(); } }, "↓"),
            el("button", { class: "iconbtn", title: "Add a bookmark underneath", "aria-label": "Add child", onclick: () => { b.children ??= []; b.children.push({ title: "New bookmark", page: b.page ?? 0, open: true, children: [] }); renderStage(); } }, "＋"),
            el("button", { class: "iconbtn", title: "Remove", "aria-label": "Remove", onclick: () => { list.splice(i, 1); renderStage(); } }, "✕")));
        if (b.children?.length) li.append(render(b.children, depth + 1, b));
        ul.append(li);
      });
      void parent; return ul;
    };
    panel.append(render(items, 0, null),
      el("div", { class: "row", style: "margin-top:12px" },
        el("button", { class: "btn small", onclick: () => { items.push({ title: "New bookmark", page: 0, open: true, children: [] }); renderStage(); } }, "+ Add bookmark"),
        el("button", { class: "btn small", onclick: () => { for (let p = 0; p < f.pages; p++) items.push({ title: `Page ${p + 1}`, page: p, open: true, children: [] }); renderStage(); } }, "Add one per page"),
        el("button", { class: "btn small", disabled: !items.length, onclick: () => { items.length = 0; renderStage(); } }, "Remove all")),
      el("p", { class: "summary", style: "margin:10px 0 0" }, "Bookmarks appear in the reader's sidebar and jump to a page. Nest them with the + button to make sections."));
    put(panel, cta("Save bookmarks", () => runJob("Saving", () => perFile("-bookmarked", (doc) => { const clean = (l) => l.filter((b) => b.title.trim()).map((b) => ({ ...b, title: b.title.trim(), children: clean(b.children || []) })); doc.setBookmarks(clean(items)); }))));
  },
  extract() {
    put(dropzone(), fileList(), summaryLine());
    if (!ready().length) return;
    const o = pref("extract", { breaks: true });
    put(el("div", { class: "panel" }, el("h3", {}, "Options"), el("div", { class: "row" }, check("Mark page breaks with a line of dashes", o.breaks, (v) => (o.breaks = v))),
      el("p", { class: "summary", style: "margin:12px 0 0" }, "Text is read in the order it appears on each page. Scanned pages contain no real text; they come out empty.")),
      cta(`Extract text from ${ready().length > 1 ? plural(ready().length, "file") : "PDF"}`, () => runJob("Extracting", async () => {
        const outs = []; const list = ready(); let k = 0;
        for (const f of list) {
          setProgress(++k, list.length, f.file.name); await sleep(0);
          const parts = []; let empty = 0;
          for (let p = 0; p < f.pages; p++) { const t = f.doc.pageText(p); if (!t.trim()) empty++; parts.push(t); }
          const text = parts.join(o.breaks ? "\n\n" + "-".repeat(40) + "\n\n" : "\n\n");
          outs.push({ name: stem(f.file.name) + ".txt", data: new TextEncoder().encode(text), pages: f.pages, mime: "text/plain", preview: text, note: empty ? `${plural(empty, "page")} had no text (probably scanned images).` : null });
        }
        return outs;
      })));
  },
  forms() { editorStage("forms", "forms", "-filled", "Save filled form"); },
  annotate() { editorStage("annotate", "annotate", "-annotated", "Save"); },
  batch() { batchStage(); },
};

// A tool built on the page editor. State (items, form values, signature) is kept per file so "Adjust" returns to the same edits.
let editor = null;
export const getEditor = () => editor?.api;
function editorStage(toolId, mode, suffix, label) {
  put(dropzone(), fileList());
  const f = ready()[0];
  if (editor && (editor.file !== f || editor.mode !== mode)) { editor.api?.destroy(); editor = null; }
  if (!f) return;
  if (mode === "forms" && !f.doc.hasFields()) { put(notice("warn", "This PDF has no form fields. Use Fill & Sign to type on it instead."), el("div", { class: "cta" }, el("button", { class: "btn primary", onclick: () => open("sign") }, "Open in Fill & Sign"))); return; }
  const o = pref(toolId, { flatten: mode !== "annotate", author: "" });
  const host = el("div", { class: "panel editorpanel" }, el("p", { class: "summary" }, "Loading pages…"));
  put(host);
  const saveBar = el("div", { class: "panel" });
  put(saveBar);
  const mount = async () => {
    try {
      if (!editor) { const api = await createEditor(f, mode); editor = { file: f, mode, api }; }
      host.innerHTML = ""; host.append(editor.api.root);
      editor.api.mounted();
    } catch (e) { host.innerHTML = ""; host.append(notice("err", friendly(e))); }
  };
  mount();
  const opts = el("div", { class: "row" });
  if (mode === "crop") { o.cropAll ??= true; opts.append(field("Apply to", segmented([[true, "Every page"], [false, "Only the page I marked"]], o.cropAll, (v) => (o.cropAll = v), "Apply to"), "The same area is used on every page; pages of a different size are clipped to it.")); }
  else if (mode === "redact") { o.fill ??= "#000000"; o.strip ??= true; opts.append(field("Box colour", segmented([["#000000", "Black"], ["#ffffff", "White"], ["none", "No box, just remove"]], o.fill, (v) => (o.fill = v), "Box colour")), check("Also wipe hidden metadata (author, title, editing history)", o.strip, (v) => (o.strip = v))); saveBar.append(el("p", { class: "summary", style: "margin:0 0 10px" }, "Redaction is permanent: the text, drawings and image pixels under each box are deleted from the file, not hidden. Check the result before sharing it.")); }
  else if (mode === "annotate") opts.append(check("Flatten: burn the comments into the page so they can't be edited or removed", o.flatten, (v) => (o.flatten = v)), field("Your name (shown on comments, optional)", el("input", { type: "text", value: o.author, placeholder: "e.g. Ada", oninput: (e) => (o.author = e.target.value) })));
  else if (mode === "forms") opts.append(check("Flatten: make the filled form permanent (fields can no longer be changed)", o.flatten, (v) => (o.flatten = v)));
  else opts.append(el("p", { class: "summary", style: "margin:0" }, "Everything you add becomes a permanent part of the page, like ink on paper."));
  saveBar.append(el("h3", {}, "Save"), opts,
    cta(label, () => runJob(mode === "redact" ? "Redacting" : "Saving", () => perFile(suffix, (doc, f, out) => {
      const n = editor.api.apply(doc, { flatten: o.flatten, author: o.author, fill: o.fill, strip: o.strip, cropAll: o.cropAll });
      if (!n) throw new Error(mode === "forms" ? "Nothing was filled in yet." : mode === "redact" ? "Nothing is marked yet. Drag a box over the page or find a word first." : mode === "crop" ? "Drag the area to keep on a page first." : "Nothing has been added yet. Use the tools above the page first.");
      if (mode === "redact" && editor.api.lastReport) { const r = editor.api.lastReport; out.note = `Removed ${plural(r.glyphsRemoved, "character")}, ${plural(r.pathsRemoved, "drawing")}, ${r.imagesRemoved + r.imagesEdited} image${r.imagesRemoved + r.imagesEdited === 1 ? "" : "s"} and ${plural(r.annotationsRemoved, "annotation")}.` + (r.warnings.length ? " " + r.warnings[0] : ""); }
    }))));
}

// Route last, once every tool is defined, so deep links like #merge work on a cold load.
route();
