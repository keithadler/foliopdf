// The page editor behind Fill & Sign, Fill a form, and Comment & mark up.
//
// Pages are drawn by pdf.js; everything the user adds lives in an overlay as
// plain items in page points (origin top-left of the displayed page), and is
// written into the PDF by the foliopdf engine only when the user saves.
import { $, el, toast, segmented } from "./app.js?v=dev";
import { openPdf, pdfjs } from "./thumbs.js?v=dev";

const MODES = {
  fill: {
    title: "Fill & Sign",
    tools: [["select", "Select", "↖"], ["text", "Text", "T"], ["check", "Check", "✓"], ["cross", "Cross", "✕"], ["dot", "Dot", "●"], ["date", "Date", "📅"], ["sign", "Signature", "✍︎"], ["initials", "Initials", "AB"], ["image", "Image", "🖼"]],
    hint: "Click anywhere on the page to type. Tick boxes with the check tools. Add your signature once, then place it wherever it's needed.",
  },
  annotate: {
    title: "Comment & mark up",
    tools: [["select", "Select", "↖"], ["highlight", "Highlight", "🖍"], ["underline", "Underline", "U̲"], ["strike", "Strike", "S̶"], ["pen", "Draw", "✎"], ["text", "Text box", "T"], ["note", "Note", "💬"], ["rect", "Box", "▭"], ["ellipse", "Circle", "◯"], ["line", "Line", "╱"], ["image", "Image", "🖼"], ["sign", "Signature", "✍︎"]],
    hint: "Select text with the highlight tools, draw freehand, or drop notes and shapes. Comments stay editable in other apps unless you flatten them.",
  },
  redact: {
    title: "Redact",
    tools: [["select", "Select", "↖"], ["area", "Mark area", "▮"], ["find", "Find text", "🔍"]],
    hint: "Drag boxes over anything that must disappear, or find a word and mark every occurrence. Applying removes the text, graphics and image pixels underneath for good.",
  },
  crop: {
    title: "Crop",
    tools: [["select", "Select", "↖"], ["area", "Crop area", "⌗"]],
    hint: "Drag the area to keep on one page. Apply it to that page only or to every page. Cropping hides the margins; it does not delete anything.",
  },
  forms: {
    title: "Fill a form",
    tools: [["select", "Select", "↖"], ["sign", "Signature", "✍︎"], ["text", "Text", "T"]],
    hint: "Type into the blue fields. Signature fields open the signature panel when clicked. Add your own text anywhere with the Text tool.",
  },
};
const DEFAULT_COLORS = { text: "#111111", pen: "#c0392b", highlight: "#ffea3b", underline: "#1a7f4b", strike: "#c0392b", rect: "#0b63d6", ellipse: "#0b63d6", line: "#0b63d6", note: "#f2c744", check: "#111111", cross: "#111111", dot: "#111111" };
const hexToRgb = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16) / 255);
const MARKUP_COLORS = [["#ffea3b", "yellow"], ["#8ef58e", "green"], ["#8ed1ff", "blue"], ["#ffb0e0", "pink"], ["#ffb347", "orange"]];
const INK_COLORS = [["#111111", "black"], ["#c0392b", "red"], ["#0b63d6", "blue"], ["#1a7f4b", "green"], ["#7a3fb8", "purple"]];
const todayText = () => { const d = new Date(); return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`; };
const pdfDate = () => { const d = new Date(); const p = (n) => String(n).padStart(2, "0"); return `D:${d.getUTCFullYear()}${p(d.getUTCMonth() + 1)}${p(d.getUTCDate())}${p(d.getUTCHours())}${p(d.getUTCMinutes())}${p(d.getUTCSeconds())}Z`; };
let nextId = 1;

/**
 * Builds an editor for `entry` ({bytes, pw, doc}) in `mode`. Resolves to an
 * object with `root` (the element to mount), `items()`, `values()` (form
 * values), and `apply(doc, opts)` which writes everything into a PdfDocument.
 */
export async function createEditor(entry, mode, opts = {}) {
  const cfg = MODES[mode];
  const pdf = await openPdf(entry.bytes, entry.pw);
  if (!pdf) throw new Error("This PDF couldn't be displayed. The file may be damaged; the other tools may still work on it.");
  const lib = await pdfjs();
  const fields = mode === "forms" ? entry.doc.fields() : [];
  const state = {
    items: opts.items ? JSON.parse(JSON.stringify(opts.items)) : [],
    values: opts.values ? { ...opts.values } : Object.fromEntries(fields.map((f) => [f.name, f.kind === "list" ? f.values : f.value])),
    tool: mode === "forms" ? "select" : mode === "fill" ? "text" : mode === "redact" || mode === "crop" ? "area" : "highlight",
    redactFill: "#000000", findText: "", findCase: false,
    selected: null, zoom: 1, undo: [], redo: [], color: {}, penWidth: 2, textSize: 12,
    signature: opts.signature || null, initials: opts.initials || null,
  };
  const pages = []; // {index, w, h (points, display), el, canvas, ovl, textLayer, rendered, renderTask}
  const root = el("div", { class: "editor" });
  const toolbar = el("div", { class: "etoolbar", role: "toolbar", "aria-label": "Editing tools" });
  const propbar = el("div", { class: "eprops" });
  const column = el("div", { class: "epages", tabindex: 0, "aria-label": "Pages" });
  const hint = el("div", { class: "ehint" }, cfg.hint);
  root.append(toolbar, propbar, hint, column);
  const api = { root, state, pages, mode, fields, items: () => state.items, values: () => state.values, destroy, setZoom, select, addItem, deleteSelected, undo, redo, refresh: renderAll, apply, openSignature, fitWidth };

  // ------------------------------------------------------------ toolbar
  const toolBtns = {};
  const zoomLabel = el("span", { class: "ezoom" });
  function renderToolbar() {
    toolbar.innerHTML = "";
    const group = el("div", { class: "etools" });
    for (const [id, label, ico] of cfg.tools) {
      const b = el("button", { type: "button", class: "etool" + (state.tool === id ? " on" : ""), "aria-pressed": String(state.tool === id), title: label, onclick: () => setTool(id) }, el("span", { class: "eico", "aria-hidden": true }, ico), el("span", { class: "elabel" }, label));
      toolBtns[id] = b; group.append(b);
    }
    const right = el("div", { class: "eright" },
      el("button", { type: "button", class: "iconbtn", title: "Undo (Ctrl+Z)", "aria-label": "Undo", disabled: !state.undo.length, onclick: undo }, "↶"),
      el("button", { type: "button", class: "iconbtn", title: "Redo", "aria-label": "Redo", disabled: !state.redo.length, onclick: redo }, "↷"),
      el("span", { class: "esep" }),
      el("button", { type: "button", class: "iconbtn", title: "Zoom out", "aria-label": "Zoom out", onclick: () => setZoom(state.zoom / 1.25) }, "−"),
      zoomLabel,
      el("button", { type: "button", class: "iconbtn", title: "Zoom in", "aria-label": "Zoom in", onclick: () => setZoom(state.zoom * 1.25) }, "+"),
      el("button", { type: "button", class: "btn small", title: "Fit to width", onclick: fitWidth }, "Fit"));
    toolbar.append(group, right);
    zoomLabel.textContent = Math.round(state.zoom * 100) + "%";
  }
  function setTool(id) {
    if (id === "sign" || id === "initials") { openSignature(id === "initials").then((sig) => { if (sig) { state.tool = id; renderToolbar(); renderProps(); updateCursor(); } }); return; }
    if (id === "image") { pickImage().then((img) => { if (img) { state.pendingImage = img; state.tool = "image"; renderToolbar(); renderProps(); updateCursor(); toast("Click on the page to place the image."); } }); return; }
    state.tool = id; select(null); renderToolbar(); renderProps(); updateCursor();
  }
  function updateCursor() { column.dataset.tool = state.tool; }

  // ------------------------------------------------------------ properties bar
  function renderProps() {
    propbar.innerHTML = "";
    const it = state.selected ? state.items.find((i) => i.id === state.selected) : null;
    const kind = it ? it.kind : state.tool;
    const colorFor = (k) => state.color[k] || DEFAULT_COLORS[k] || "#111111";
    const setColor = (k, hex) => { state.color[k] = hex; if (it) { snapshot(); it.color = hex; renderItem(it); } };
    if (["text", "date"].includes(kind)) {
      const size = it ? it.size : state.textSize;
      propbar.append(el("label", { class: "eprop" }, "Size", el("input", { type: "number", min: 6, max: 72, value: size, "aria-label": "Text size", oninput: (e) => { const v = Math.min(72, Math.max(6, +e.target.value || 12)); state.textSize = v; if (it) { it.size = v; renderItem(it); } } })));
      propbar.append(el("label", { class: "eprop" }, "Font", segmented([["Helvetica", "Sans"], ["Times-Roman", "Serif"], ["Courier", "Mono"]], it ? it.font || "Helvetica" : state.font || "Helvetica", (v) => { state.font = v; if (it) { snapshot(); it.font = v; renderItem(it); } }, "Font")));
      propbar.append(el("span", { class: "eprop" }, "Colour", swatchRow(INK_COLORS, colorFor(it ? "text" : "text"), (hex) => setColor("text", hex))));
    } else if (["pen", "rect", "ellipse", "line", "check", "cross", "dot"].includes(kind)) {
      propbar.append(el("span", { class: "eprop" }, "Colour", swatchRow(INK_COLORS, it ? it.color : colorFor(kind), (hex) => setColor(kind, hex))));
      if (["pen", "rect", "ellipse", "line"].includes(kind)) propbar.append(el("label", { class: "eprop" }, "Width", el("input", { type: "range", min: 1, max: 12, value: it ? it.width : state.penWidth, "aria-label": "Line width", oninput: (e) => { state.penWidth = +e.target.value; if (it) { it.width = +e.target.value; renderItem(it); } } })));
      if (["rect", "ellipse"].includes(kind)) propbar.append(el("label", { class: "check eprop" }, el("input", { type: "checkbox", checked: it ? !!it.fill : !!state.fillShapes, onchange: (e) => { state.fillShapes = e.target.checked; if (it) { snapshot(); it.fill = e.target.checked; renderItem(it); } } }), el("span", {}, "Filled")));
    } else if (["highlight", "underline", "strike"].includes(kind)) {
      propbar.append(el("span", { class: "eprop" }, "Colour", swatchRow(kind === "highlight" ? MARKUP_COLORS : INK_COLORS, it ? it.color : colorFor(kind), (hex) => setColor(kind, hex))));
      if (!it) propbar.append(el("span", { class: "hint" }, "Drag across text on the page to select it."));
    } else if (kind === "note") {
      if (it) {
        const ta = el("textarea", { rows: 2, placeholder: "Note text", "aria-label": "Note text", oninput: (e) => { it.contents = e.target.value; } }, it.contents || "");
        propbar.append(el("label", { class: "eprop grow" }, "Note", ta));
      } else propbar.append(el("span", { class: "hint" }, "Click on the page to drop a note, then type its text here."));
    } else if (kind === "image" || kind === "sign" || kind === "initials") {
      if (it) propbar.append(el("span", { class: "hint" }, "Drag to move, use the corner to resize."));
      else propbar.append(el("span", { class: "hint" }, state.tool === "image" ? "Click on the page to place the image." : "Click on the page to place it. Click again to place another copy."), el("button", { type: "button", class: "btn small", onclick: () => openSignature(state.tool === "initials") }, "Change"));
    } else if (mode === "crop") {
      const c = state.items.find((i) => i.kind === "crop");
      propbar.append(el("span", { class: "hint" }, c ? `Crop on page ${c.page + 1}: ${Math.round(c.w)} × ${Math.round(c.h)} pt. Drag the corner to adjust.` : "Drag on a page to mark the area to keep."));
      if (c) propbar.append(el("button", { type: "button", class: "btn small", onclick: () => { snapshot(); state.items = []; state.selected = null; renderAll(); } }, "Clear"));
    } else if (mode === "redact" && (kind === "area" || kind === "find" || kind === "redact" || kind === "select")) {
      const marks = state.items.filter((i) => i.kind === "redact").length;
      if (kind === "find" || (kind === "select" && !it)) {
        const inp = el("input", { type: "text", placeholder: "Word or phrase", value: state.findText, "aria-label": "Text to find", oninput: (e) => (state.findText = e.target.value), onkeydown: (e) => { if (e.key === "Enter") { e.preventDefault(); markAll(); } } });
        propbar.append(el("label", { class: "eprop" }, "Find", inp), el("button", { type: "button", class: "btn small", onclick: markAll }, "Mark all matches"), el("label", { class: "check eprop" }, el("input", { type: "checkbox", checked: state.findCase, onchange: (e) => (state.findCase = e.target.checked) }), el("span", {}, "Match case")));
      }
      if (kind === "area") propbar.append(el("span", { class: "hint" }, "Drag on the page to mark an area."));
      propbar.append(el("span", { class: "hint" }, marks ? `${marks} area${marks === 1 ? "" : "s"} marked.` : "Nothing marked yet."));
      if (marks) propbar.append(el("button", { type: "button", class: "btn small", onclick: () => { snapshot(); state.items = state.items.filter((i) => i.kind !== "redact"); state.selected = null; renderAll(); } }, "Clear all"));
    } else if (kind === "select" && mode === "forms") {
      propbar.append(el("span", { class: "hint" }, `${fields.length} form field${fields.length === 1 ? "" : "s"}. Type into the blue boxes.`), el("button", { type: "button", class: "btn small", onclick: () => { for (const f of fields) state.values[f.name] = f.kind === "checkbox" || f.kind === "radio" ? "Off" : f.kind === "list" ? [] : ""; renderAll(); } }, "Clear all fields"));
    } else if (kind === "select") {
      propbar.append(el("span", { class: "hint" }, state.items.length ? "Click an item to move, resize or delete it." : "Nothing added yet. Pick a tool above."));
    }
    if (it) {
      propbar.append(el("button", { type: "button", class: "btn small danger", onclick: deleteSelected }, "Delete"));
      if (it.kind === "sign" || it.kind === "initials" || it.kind === "image") propbar.append(el("button", { type: "button", class: "btn small", onclick: () => { snapshot(); const c = { ...it, id: nextId++, x: it.x + 12, y: it.y + 12 }; state.items.push(c); renderItem(c); select(c.id); } }, "Duplicate"));
    }
  }
  function markAll() {
    const needle = (state.findText || "").trim();
    if (!needle) { toast("Type a word or phrase to find."); return; }
    let n = 0;
    snapshot();
    for (const p of pages) {
      let hits = [];
      try { hits = entry.doc.search(p.index, needle, { caseInsensitive: !state.findCase }); } catch (e) { console.warn(e); }
      for (const h of hits) for (const r of h.rects) {
        if (state.items.some((i) => i.kind === "redact" && i.page === p.index && Math.abs(i.x - r.x0) < 0.5 && Math.abs(i.y - r.y0) < 0.5 && Math.abs(i.w - (r.x1 - r.x0)) < 0.5)) continue;
        const it = { id: nextId++, kind: "redact", page: p.index, x: r.x0 - 1, y: r.y0 - 1, w: r.x1 - r.x0 + 2, h: r.y1 - r.y0 + 2, text: h.text };
        state.items.push(it); renderItem(it); n++;
      }
    }
    toast(n ? `Marked ${n} occurrence${n === 1 ? "" : "s"} of “${needle}”.` : `“${needle}” was not found. The text may be an image (scan) rather than real text.`);
    renderProps();
  }
  function swatchRow(colors, value, onpick) {
    const w = el("span", { class: "swatches" });
    for (const [hex, name] of colors) w.append(el("button", { type: "button", class: "swatch" + (hex === value ? " on" : ""), style: `background:${hex}`, title: name, "aria-label": name, "aria-pressed": String(hex === value), onclick: () => { onpick(hex); renderProps(); } }));
    return w;
  }

  // ------------------------------------------------------------ pages
  for (let i = 0; i < pdf.numPages; i++) {
    const pg = await pdf.getPage(i + 1);
    const vp = pg.getViewport({ scale: 1 });
    const p = { index: i, pg, w: vp.width, h: vp.height, rendered: 0 };
    p.el = el("div", { class: "epage", "data-page": i, "aria-label": `Page ${i + 1}` });
    p.canvas = el("canvas");
    p.textLayer = el("div", { class: "textLayer" });
    p.fieldsLayer = el("div", { class: "efields" });
    p.ovl = el("div", { class: "eovl" });
    p.el.append(p.canvas, p.textLayer, p.fieldsLayer, p.ovl);
    column.append(p.el);
    pages.push(p);
  }
  const io = new IntersectionObserver((entries) => { for (const e of entries) { const p = pages[+e.target.dataset.page]; p.visible = e.isIntersecting; if (e.isIntersecting) renderPage(p); } }, { root: null, rootMargin: "600px 0px" });
  pages.forEach((p) => io.observe(p.el));

  async function renderPage(p) {
    const z = state.zoom;
    if (p.rendered === z || p.rendering === z) return;
    p.rendering = z;
    try { p.renderTask?.cancel(); } catch {}
    const vp = p.pg.getViewport({ scale: z });
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    p.canvas.width = Math.ceil(vp.width * dpr); p.canvas.height = Math.ceil(vp.height * dpr);
    p.canvas.style.width = Math.round(vp.width) + "px"; p.canvas.style.height = Math.round(vp.height) + "px";
    const task = p.pg.render({ canvasContext: p.canvas.getContext("2d", { alpha: false }), viewport: p.pg.getViewport({ scale: z * dpr }), background: "#ffffff", annotationMode: mode === "forms" ? 2 : 1 });
    p.renderTask = task;
    // The selectable text layer does not wait for the pixels.
    if (mode === "annotate" && lib?.TextLayer && p.textZoom !== z) {
      p.textZoom = z;
      p.textLayer.innerHTML = "";
      p.textLayer.style.setProperty("--scale-factor", String(z));
      new lib.TextLayer({ textContentSource: p.pg.streamTextContent({ includeMarkedContent: false }), container: p.textLayer, viewport: vp }).render()
        .then(() => p.textLayer.append(el("div", { class: "endOfContent" })), (e) => console.warn("text layer", e));
    }
    try { await task.promise; } catch (e) { if (!/cancel/i.test(String(e))) console.warn(e); p.rendering = null; return; }
    p.rendered = z; p.rendering = null;
  }
  function layoutPages() {
    const z = state.zoom;
    for (const p of pages) {
      p.el.style.width = Math.round(p.w * z) + "px"; p.el.style.height = Math.round(p.h * z) + "px";
      p.el.style.setProperty("--z", z);
      if (p.rendered !== z) { p.rendered = 0; if (p.visible || p.index < 2) renderPage(p); }
    }
  }
  function setZoom(z) { state.zoom = Math.min(4, Math.max(0.25, z)); zoomLabel.textContent = Math.round(state.zoom * 100) + "%"; layoutPages(); renderAll(); }
  function fitWidth() { const cw = column.clientWidth || root.clientWidth; if (cw < 200) { setZoom(state.zoom); return; } const w = Math.max(...pages.map((p) => p.w)); setZoom(Math.min(1.6, Math.max(0.3, (cw - 32) / w))); }

  // ------------------------------------------------------------ items
  function snapshot() { state.undo.push(JSON.stringify(state.items)); if (state.undo.length > 100) state.undo.shift(); state.redo = []; renderToolbar(); }
  function undo() { if (!state.undo.length) return; state.redo.push(JSON.stringify(state.items)); state.items = JSON.parse(state.undo.pop()); state.selected = null; renderAll(); renderToolbar(); }
  function redo() { if (!state.redo.length) return; state.undo.push(JSON.stringify(state.items)); state.items = JSON.parse(state.redo.pop()); state.selected = null; renderAll(); renderToolbar(); }
  function addItem(item) { snapshot(); item.id = item.id || nextId++; state.items.push(item); renderItem(item); return item; }
  function select(id) { state.selected = id; for (const p of pages) p.ovl.querySelectorAll(".eitem").forEach((n) => n.classList.toggle("sel", +n.dataset.id === id)); renderProps(); }
  function deleteSelected() { if (state.selected == null) return; snapshot(); const it = state.items.find((i) => i.id === state.selected); state.items = state.items.filter((i) => i.id !== state.selected); if (it) pages[it.page]?.ovl.querySelector(`.eitem[data-id="${it.id}"]`)?.remove(); state.selected = null; renderProps(); }
  function renderAll() { for (const p of pages) { p.ovl.innerHTML = ""; if (mode === "forms") renderFields(p); } for (const it of state.items) renderItem(it); renderProps(); }

  function itemNode(it) {
    const p = pages[it.page]; if (!p) return null;
    let n = p.ovl.querySelector(`.eitem[data-id="${it.id}"]`);
    if (!n) { n = el("div", { class: "eitem " + it.kind, "data-id": it.id, tabindex: 0, role: "group", "aria-label": it.kind }); p.ovl.append(n); wireItem(n, it); }
    return n;
  }
  function renderItem(it) {
    const n = itemNode(it); if (!n) return;
    const z = state.zoom; n.className = "eitem " + it.kind + (state.selected === it.id ? " sel" : "");
    const box = (x, y, w, h) => { n.style.left = x * z + "px"; n.style.top = y * z + "px"; n.style.width = w * z + "px"; n.style.height = h * z + "px"; };
    n.innerHTML = "";
    switch (it.kind) {
      case "text": case "date": {
        box(it.x, it.y, it.w, it.h);
        const ta = el("div", { class: "etext", contenteditable: "plaintext-only", spellcheck: "false", style: `font-size:${it.size * z}px;color:${it.color};font-family:${fontFamily(it.font)};padding:${2 * z}px;line-height:1.18` });
        ta.textContent = it.text || "";
        ta.oninput = () => { it.text = ta.textContent; fitText(it, ta); };
        ta.onkeydown = (e) => { e.stopPropagation(); if (e.key === "Escape") { ta.blur(); } };
        ta.onblur = () => { if (!it.text?.trim()) { state.items = state.items.filter((i) => i !== it); n.remove(); if (state.selected === it.id) { state.selected = null; renderProps(); } } };
        ta.onpointerdown = (e) => { e.stopPropagation(); select(it.id); };
        n.append(ta, handle("e"));
        break;
      }
      case "check": case "cross": case "dot": {
        box(it.x, it.y, it.w, it.h);
        n.append(el("div", { class: "eglyph", style: `color:${it.color};font-size:${it.h * z * 0.9}px;line-height:${it.h * z}px` }, it.kind === "check" ? "✓" : it.kind === "cross" ? "✕" : "●"), handle("se"));
        break;
      }
      case "sign": case "initials": case "image": {
        box(it.x, it.y, it.w, it.h);
        n.append(el("img", { src: it.src, alt: it.kind, draggable: "false" }), handle("se"));
        break;
      }
      case "rect": case "ellipse": {
        box(it.x, it.y, it.w, it.h);
        n.append(el("div", { class: "eshape", style: `border:${it.width * z}px solid ${it.color};border-radius:${it.kind === "ellipse" ? "50%" : "0"};background:${it.fill ? it.color + "44" : "transparent"}` }), handle("se"));
        break;
      }
      case "line": {
        const x0 = Math.min(it.x1, it.x2), y0 = Math.min(it.y1, it.y2);
        const w = Math.abs(it.x2 - it.x1) || 1, h = Math.abs(it.y2 - it.y1) || 1;
        box(x0 - 4, y0 - 4, w + 8, h + 8);
        const svg = `<svg width="100%" height="100%" viewBox="0 0 ${(w + 8) * z} ${(h + 8) * z}" style="position:absolute;inset:0"><line x1="${(it.x1 - x0 + 4) * z}" y1="${(it.y1 - y0 + 4) * z}" x2="${(it.x2 - x0 + 4) * z}" y2="${(it.y2 - y0 + 4) * z}" stroke="${it.color}" stroke-width="${it.width * z}" stroke-linecap="round"/></svg>`;
        n.innerHTML = svg;
        break;
      }
      case "pen": {
        const xs = it.points.map((q) => q[0]), ys = it.points.map((q) => q[1]);
        const x0 = Math.min(...xs) - it.width, y0 = Math.min(...ys) - it.width, x1 = Math.max(...xs) + it.width, y1 = Math.max(...ys) + it.width;
        it.x = x0; it.y = y0; it.w = x1 - x0; it.h = y1 - y0;
        box(x0, y0, it.w, it.h);
        const d = it.points.map((q, i) => (i ? "L" : "M") + ((q[0] - x0) * z).toFixed(1) + " " + ((q[1] - y0) * z).toFixed(1)).join(" ");
        n.innerHTML = `<svg width="100%" height="100%" viewBox="0 0 ${it.w * z} ${it.h * z}" style="position:absolute;inset:0"><path d="${d}" fill="none" stroke="${it.color}" stroke-width="${it.width * z}" stroke-linecap="round" stroke-linejoin="round"/></svg>`;
        break;
      }
      case "highlight": case "underline": case "strike": {
        const x0 = Math.min(...it.quads.map((q) => q.x0)), y0 = Math.min(...it.quads.map((q) => q.y0)), x1 = Math.max(...it.quads.map((q) => q.x1)), y1 = Math.max(...it.quads.map((q) => q.y1));
        it.x = x0; it.y = y0; it.w = x1 - x0; it.h = y1 - y0;
        box(x0, y0, it.w, it.h);
        for (const q of it.quads) {
          const s = `left:${(q.x0 - x0) * z}px;top:${(q.y0 - y0) * z}px;width:${(q.x1 - q.x0) * z}px;height:${(q.y1 - q.y0) * z}px;`;
          if (it.kind === "highlight") n.append(el("div", { class: "equad", style: s + `background:${it.color};mix-blend-mode:multiply;opacity:.9` }));
          else if (it.kind === "underline") n.append(el("div", { class: "equad", style: s + `border-bottom:${Math.max(1, (q.y1 - q.y0) * 0.07 * z)}px solid ${it.color}` }));
          else n.append(el("div", { class: "equad", style: s }, el("div", { style: `position:absolute;left:0;right:0;top:50%;border-top:${Math.max(1, (q.y1 - q.y0) * 0.07 * z)}px solid ${it.color}` })));
        }
        break;
      }
      case "crop": {
        box(it.x, it.y, it.w, it.h);
        n.append(el("div", { class: "ecrop" }), handle("se"));
        break;
      }
      case "redact": {
        box(it.x, it.y, it.w, it.h);
        n.title = it.text ? `Will remove: ${it.text}` : "Will be removed";
        n.append(el("div", { class: "eredact" }), handle("se"));
        break;
      }
      case "note": {
        box(it.x, it.y, 20, 20);
        n.append(el("div", { class: "enote", style: `background:${it.color}`, title: it.contents || "Note" }, "💬"));
        break;
      }
    }
  }
  function fontFamily(f) { return f === "Times-Roman" ? "'Times New Roman', Times, serif" : f === "Courier" ? "'Courier New', Courier, monospace" : "Helvetica, Arial, sans-serif"; }
  function fitText(it, ta) { const z = state.zoom; const h = ta.scrollHeight / z; it.h = Math.max(it.size * 1.18 + 4, h); const n = itemNode(it); if (n) n.style.height = it.h * z + "px"; }
  function handle(dir) { return el("div", { class: "ehandle " + dir, "data-dir": dir }); }

  // Dragging and resizing of an existing item.
  function wireItem(n, it) {
    n.onpointerdown = (e) => {
      if (state.tool !== "select" && !["text", "date", "sign", "initials", "image", "check", "cross", "dot", "area", "find"].includes(state.tool)) return;
      if (it.kind === "crop" && e.target.dataset.dir !== "se" && state.tool === "area") { /* allow re-drag */ }
      if (e.target.classList.contains("etext")) return;
      e.stopPropagation(); e.preventDefault();
      select(it.id);
      if (["highlight", "underline", "strike"].includes(it.kind)) return;
      const z = state.zoom; const dir = e.target.dataset.dir;
      const start = { x: e.clientX, y: e.clientY, it: JSON.parse(JSON.stringify(it)) };
      let moved = false;
      n.setPointerCapture(e.pointerId);
      n.onpointermove = (ev) => {
        const dx = (ev.clientX - start.x) / z, dy = (ev.clientY - start.y) / z;
        if (!moved && Math.hypot(dx, dy) < 1) return;
        if (!moved) { moved = true; snapshot(); }
        if (dir === "se") {
          const keep = ["sign", "initials", "image", "check", "cross", "dot"].includes(it.kind);
          let w = Math.max(6, start.it.w + dx), h = Math.max(6, start.it.h + dy);
          if (keep) { const r = start.it.w / start.it.h; if (w / h > r) w = h * r; else h = w / r; }
          it.w = w; it.h = h;
        } else if (dir === "e") { it.w = Math.max(30, start.it.w + dx); }
        else if (it.kind === "line") { it.x1 = start.it.x1 + dx; it.y1 = start.it.y1 + dy; it.x2 = start.it.x2 + dx; it.y2 = start.it.y2 + dy; }
        else if (it.kind === "pen") { it.points = start.it.points.map((q) => [q[0] + dx, q[1] + dy]); }
        else { it.x = start.it.x + dx; it.y = start.it.y + dy; }
        renderItem(it);
        if (dir === "e" && it.kind === "text") { const ta = n.querySelector(".etext"); if (ta) fitText(it, ta); }
      };
      n.onpointerup = n.onpointercancel = () => { n.onpointermove = null; try { n.releasePointerCapture(e.pointerId); } catch {} };
    };
    n.ondblclick = () => { if (it.kind === "text" || it.kind === "date") n.querySelector(".etext")?.focus(); };
  }

  // ------------------------------------------------------------ creating items with the pointer
  function pagePoint(p, e) { const r = p.ovl.getBoundingClientRect(); const z = state.zoom; return [(e.clientX - r.left) / z, (e.clientY - r.top) / z]; }
  for (const p of pages) {
    p.ovl.onpointerdown = (e) => {
      if (e.button !== 0) return;
      const tool = state.tool;
      const [x, y] = pagePoint(p, e);
      const color = state.color[tool] || DEFAULT_COLORS[tool] || "#111111";
      if (tool === "select") { select(null); return; }
      if (["highlight", "underline", "strike"].includes(tool)) return; // handled by text selection
      e.preventDefault();
      if (tool === "text" || tool === "date") {
        const size = state.textSize;
        const it = addItem({ kind: "text", page: p.index, x, y: y - size * 0.7, w: Math.min(220, p.w - x - 4), h: size * 1.18 + 4, size, color: state.color.text || DEFAULT_COLORS.text, font: state.font || "Helvetica", text: tool === "date" ? todayText() : "" });
        select(it.id);
        const ta = itemNode(it)?.querySelector(".etext"); if (ta) { ta.focus(); if (tool === "date") fitText(it, ta); }
        if (tool === "date") state.tool = "select", renderToolbar();
        return;
      }
      if (["check", "cross", "dot"].includes(tool)) { const s = 14; const it = addItem({ kind: tool, page: p.index, x: x - s / 2, y: y - s / 2, w: s, h: s, color }); select(it.id); return; }
      if (tool === "sign" || tool === "initials") {
        const sig = tool === "sign" ? state.signature : state.initials; if (!sig) { openSignature(tool === "initials"); return; }
        const w = tool === "sign" ? 160 : 60; const h = w / sig.ratio;
        const it = addItem({ kind: tool, page: p.index, x: x - w / 2, y: y - h / 2, w, h, src: sig.src, png: sig.png }); select(it.id); return;
      }
      if (tool === "image" && state.pendingImage) {
        const img = state.pendingImage; const w = Math.min(200, img.w * 0.75); const h = w / img.ratio;
        const it = addItem({ kind: "image", page: p.index, x: x - w / 2, y: y - h / 2, w, h, src: img.src, png: img.png }); select(it.id); state.tool = "select"; renderToolbar(); return;
      }
      if (tool === "note") { const it = addItem({ kind: "note", page: p.index, x, y: y - 10, color, contents: "" }); select(it.id); setTimeout(() => propbar.querySelector("textarea")?.focus(), 0); return; }
      if (tool === "pen") {
        const it = { kind: "pen", page: p.index, points: [[x, y]], color, width: state.penWidth };
        addItem(it); p.ovl.setPointerCapture(e.pointerId);
        p.ovl.onpointermove = (ev) => { const [px, py] = pagePoint(p, ev); const last = it.points[it.points.length - 1]; if (Math.hypot(px - last[0], py - last[1]) > 0.7) { it.points.push([px, py]); renderItem(it); } };
        p.ovl.onpointerup = p.ovl.onpointercancel = () => { p.ovl.onpointermove = null; if (it.points.length < 2) it.points.push([x + 0.5, y + 0.5]); renderItem(it); select(it.id); };
        return;
      }
      if (tool === "find") { select(null); return; }
      if (tool === "area" && mode === "crop") {
        snapshot(); state.items = state.items.filter((i) => i.kind !== "crop"); renderAll();
        const it = { kind: "crop", page: p.index, x, y, w: 1, h: 1 };
        addItem(it); p.ovl.setPointerCapture(e.pointerId);
        p.ovl.onpointermove = (ev) => { const [px, py] = pagePoint(p, ev); it.x = Math.max(0, Math.min(x, px)); it.y = Math.max(0, Math.min(y, py)); it.w = Math.max(1, Math.min(p.w - it.x, Math.abs(px - x))); it.h = Math.max(1, Math.min(p.h - it.y, Math.abs(py - y))); renderItem(it); };
        p.ovl.onpointerup = p.ovl.onpointercancel = () => { p.ovl.onpointermove = null; if (it.w < 10 || it.h < 10) { state.items = state.items.filter((i) => i !== it); itemNode(it)?.remove(); } else select(it.id); renderProps(); };
        return;
      }
      if (tool === "area") {
        const it = { kind: "redact", page: p.index, x, y, w: 1, h: 1 };
        addItem(it); p.ovl.setPointerCapture(e.pointerId);
        p.ovl.onpointermove = (ev) => { const [px, py] = pagePoint(p, ev); it.x = Math.min(x, px); it.y = Math.min(y, py); it.w = Math.max(1, Math.abs(px - x)); it.h = Math.max(1, Math.abs(py - y)); renderItem(it); };
        p.ovl.onpointerup = p.ovl.onpointercancel = () => { p.ovl.onpointermove = null; if (it.w < 3 || it.h < 3) { state.items = state.items.filter((i) => i !== it); itemNode(it)?.remove(); } else select(it.id); renderProps(); };
        return;
      }
      if (["rect", "ellipse", "line"].includes(tool)) {
        const it = tool === "line" ? { kind: "line", page: p.index, x1: x, y1: y, x2: x, y2: y, color, width: state.penWidth } : { kind: tool, page: p.index, x, y, w: 1, h: 1, color, width: state.penWidth, fill: !!state.fillShapes };
        addItem(it); p.ovl.setPointerCapture(e.pointerId);
        p.ovl.onpointermove = (ev) => { const [px, py] = pagePoint(p, ev); if (tool === "line") { it.x2 = px; it.y2 = py; } else { it.x = Math.min(x, px); it.y = Math.min(y, py); it.w = Math.max(1, Math.abs(px - x)); it.h = Math.max(1, Math.abs(py - y)); } renderItem(it); };
        p.ovl.onpointerup = p.ovl.onpointercancel = () => { p.ovl.onpointermove = null; if ((tool !== "line" && (it.w < 3 || it.h < 3)) || (tool === "line" && Math.hypot(it.x2 - it.x1, it.y2 - it.y1) < 3)) { state.items = state.items.filter((i) => i !== it); itemNode(it)?.remove(); } else select(it.id); };
      }
    };
  }
  // Text markup from the native selection over the pdf.js text layer.
  column.addEventListener("pointerup", () => { if (["highlight", "underline", "strike"].includes(state.tool)) setTimeout(markupFromSelection, 0); });
  function markupFromSelection() {
    const sel = window.getSelection(); if (!sel || sel.isCollapsed || !sel.rangeCount) return;
    const range = sel.getRangeAt(0);
    const rects = [...range.getClientRects()];
    if (!rects.length) return;
    const z = state.zoom; const byPage = new Map();
    for (const r of rects) {
      if (r.width < 1 || r.height < 1) continue;
      const pe = document.elementFromPoint(r.left + 1, r.top + r.height / 2)?.closest?.(".epage") || [...pages].find((p) => { const b = p.el.getBoundingClientRect(); return r.top >= b.top - 1 && r.bottom <= b.bottom + 1 && r.left >= b.left - 1; })?.el;
      if (!pe) continue;
      const p = pages[+pe.dataset.page]; const b = p.el.getBoundingClientRect();
      const q = { x0: (r.left - b.left) / z, y0: (r.top - b.top) / z, x1: (r.right - b.left) / z, y1: (r.bottom - b.top) / z };
      if (!byPage.has(p.index)) byPage.set(p.index, []);
      byPage.get(p.index).push(q);
    }
    sel.removeAllRanges();
    for (const [page, quads] of byPage) {
      const merged = mergeQuads(quads);
      if (merged.length) addItem({ kind: state.tool, page, quads: merged, color: state.color[state.tool] || DEFAULT_COLORS[state.tool] });
    }
  }
  function mergeQuads(quads) {
    // Join rects that sit on the same line (similar vertical extent) and touch or overlap.
    quads.sort((a, b) => a.y0 - b.y0 || a.x0 - b.x0);
    const out = [];
    for (const q of quads) {
      const last = out[out.length - 1];
      if (last && Math.abs(last.y0 - q.y0) < (q.y1 - q.y0) * 0.5 && q.x0 <= last.x1 + 3) { last.x1 = Math.max(last.x1, q.x1); last.y0 = Math.min(last.y0, q.y0); last.y1 = Math.max(last.y1, q.y1); }
      else out.push({ ...q });
    }
    return out;
  }
  api.markupFromSelection = markupFromSelection;

  // ------------------------------------------------------------ form fields
  function renderFields(p) {
    p.fieldsLayer.innerHTML = "";
    const z = state.zoom;
    for (const f of fields) {
      for (const w of f.widgets) {
        if (w.page !== p.index) continue;
        const r = w.rect; const style = `left:${r.x0 * z}px;top:${r.y0 * z}px;width:${(r.x1 - r.x0) * z}px;height:${(r.y1 - r.y0) * z}px;`;
        const fs = Math.max(8, Math.min(16, (r.y1 - r.y0) * 0.62)) * z;
        let node;
        if (f.kind === "text") {
          node = f.multiline ? el("textarea", { class: "efield", style: style + `font-size:${fs}px`, "aria-label": f.name, title: f.name, oninput: (e) => (state.values[f.name] = e.target.value) }, state.values[f.name] || "")
            : el("input", { type: f.password ? "password" : "text", class: "efield", style: style + `font-size:${fs}px`, "aria-label": f.name, title: f.name, value: state.values[f.name] || "", maxlength: f.maxLen || null, oninput: (e) => (state.values[f.name] = e.target.value) });
        } else if (f.kind === "checkbox") {
          const on = w.onState || "Yes";
          node = el("label", { class: "efield ebox", style, title: f.name }, el("input", { type: "checkbox", "aria-label": f.name, checked: state.values[f.name] === on, onchange: (e) => (state.values[f.name] = e.target.checked ? on : "Off") }));
        } else if (f.kind === "radio") {
          const on = w.onState || "";
          node = el("label", { class: "efield ebox", style, title: f.name }, el("input", { type: "radio", name: "efr-" + f.object, "aria-label": `${f.name}: ${on}`, checked: state.values[f.name] === on, onchange: (e) => { if (e.target.checked) state.values[f.name] = on; } }));
        } else if (f.kind === "combo" || f.kind === "list") {
          node = el("select", { class: "efield", style: style + `font-size:${fs}px`, "aria-label": f.name, title: f.name, multiple: f.kind === "list" ? "multiple" : null, onchange: (e) => { state.values[f.name] = f.kind === "list" ? [...e.target.selectedOptions].map((o) => o.value) : e.target.value; } });
          if (f.kind === "combo") node.append(el("option", { value: "" }, ""));
          const cur = f.kind === "list" ? (state.values[f.name] || []) : [state.values[f.name] || ""];
          for (const o of f.options) node.append(el("option", { value: o.value, selected: cur.includes(o.value) }, o.label));
        } else if (f.kind === "signature") {
          node = el("button", { type: "button", class: "efield esig", style, title: "Sign here", onclick: async () => { const sig = await openSignature(false); if (!sig) return; const h = r.y1 - r.y0, ww = Math.min(r.x1 - r.x0, h * sig.ratio); const it = addItem({ kind: "sign", page: p.index, x: r.x0 + ((r.x1 - r.x0) - ww) / 2, y: r.y0 + (h - ww / sig.ratio) / 2, w: ww, h: ww / sig.ratio, src: sig.src, png: sig.png }); select(it.id); } }, "Sign here");
        } else continue;
        if (f.readOnly && node.tagName !== "LABEL") node.disabled = true;
        p.fieldsLayer.append(node);
      }
    }
  }

  // ------------------------------------------------------------ signature panel
  function openSignature(initials) {
    return new Promise((resolve) => {
      const dlg = el("dialog", { class: "esigdlg", "aria-label": initials ? "Your initials" : "Your signature" });
      let tab = "draw"; let result = null;
      const body = el("div", { class: "esigbody" });
      const tabs = segmented([["draw", "Draw"], ["type", "Type"], ["upload", "Upload image"]], tab, (v) => { tab = v; renderTab(); }, "Signature style");
      const remember = el("input", { type: "checkbox" });
      const pen = { color: "#101010", w: 2.2 };
      function renderTab() {
        body.innerHTML = "";
        if (tab === "draw") {
          const cv = el("canvas", { class: "esigpad", width: 800, height: 300, "aria-label": "Drawing area" });
          const ctx = cv.getContext("2d"); let strokes = []; let cur = null;
          const redraw = () => { ctx.clearRect(0, 0, cv.width, cv.height); ctx.lineCap = ctx.lineJoin = "round"; ctx.strokeStyle = pen.color; ctx.lineWidth = pen.w * 2.4; for (const s of strokes) { ctx.beginPath(); s.forEach((q, i) => (i ? ctx.lineTo(q[0], q[1]) : ctx.moveTo(q[0], q[1]))); if (s.length === 1) ctx.lineTo(s[0][0] + 0.5, s[0][1]); ctx.stroke(); } };
          const pt = (e) => { const r = cv.getBoundingClientRect(); return [(e.clientX - r.left) * cv.width / r.width, (e.clientY - r.top) * cv.height / r.height]; };
          cv.onpointerdown = (e) => { e.preventDefault(); cur = [pt(e)]; strokes.push(cur); cv.setPointerCapture(e.pointerId); redraw(); };
          cv.onpointermove = (e) => { if (!cur) return; cur.push(pt(e)); redraw(); };
          cv.onpointerup = cv.onpointercancel = () => { cur = null; };
          body.append(cv, el("div", { class: "row", style: "align-items:center" }, el("span", { class: "hint" }, "Draw with your mouse, finger or pen."), swatchRow([["#101010", "black"], ["#1a3c8a", "blue"]], pen.color, (h) => { pen.color = h; redraw(); }), el("button", { type: "button", class: "btn small", onclick: () => { strokes = []; redraw(); } }, "Clear")));
          body.getResult = () => strokes.length ? trimCanvas(cv) : null;
        } else if (tab === "type") {
          const inp = el("input", { type: "text", placeholder: initials ? "Your initials" : "Your name", "aria-label": "Text", style: "font-size:18px" });
          const fonts = [["'Snell Roundhand','Apple Chancery','Brush Script MT','Segoe Script',cursive", "Script"], ["'Apple Chancery','Lucida Handwriting','Segoe Print',cursive", "Handwriting"], ["Georgia, 'Times New Roman', serif", "Serif"], ["Helvetica, Arial, sans-serif", "Plain"]];
          let font = fonts[0][0];
          const preview = el("div", { class: "esigpreview" });
          const upd = () => { preview.style.fontFamily = font; preview.textContent = inp.value || (initials ? "AB" : "Your name"); preview.style.color = pen.color; };
          inp.oninput = upd;
          body.append(inp, el("div", { class: "row", style: "margin-top:8px" }, segmented(fonts.map(([f, l]) => [f, l]), font, (v) => { font = v; upd(); }, "Style"), swatchRow([["#101010", "black"], ["#1a3c8a", "blue"]], pen.color, (h) => { pen.color = h; upd(); })), preview);
          upd(); setTimeout(() => inp.focus(), 0);
          body.getResult = () => { const text = inp.value.trim(); if (!text) return null; const cv = el("canvas"); const ctx = cv.getContext("2d"); const px = 96; ctx.font = `${px}px ${font}`; const m = ctx.measureText(text); cv.width = Math.ceil(m.width + px * 0.6); cv.height = Math.ceil(px * 1.6); const c2 = cv.getContext("2d"); c2.font = `${px}px ${font}`; c2.fillStyle = pen.color; c2.textBaseline = "middle"; c2.fillText(text, px * 0.3, cv.height / 2); return trimCanvas(cv); };
        } else {
          const file = el("input", { type: "file", accept: "image/png,image/jpeg", "aria-label": "Image file" });
          const prev = el("img", { class: "esigimg", alt: "" });
          let loaded = null;
          file.onchange = async () => { const f = file.files[0]; if (!f) return; loaded = await loadImageFile(f); prev.src = loaded.src; };
          body.append(file, el("p", { class: "hint" }, "A photo of your signature on white paper works well. PNG transparency is kept."), prev);
          body.getResult = () => loaded;
        }
      }
      renderTab();
      const close = (v) => { result = v; dlg.close(); };
      dlg.append(el("h3", {}, initials ? "Your initials" : "Your signature"), tabs, body,
        el("div", { class: "row", style: "justify-content:space-between;align-items:center;margin-top:12px" },
          el("label", { class: "check" }, remember, el("span", {}, "Remember on this device")),
          el("div", { class: "row" }, el("button", { type: "button", class: "btn", onclick: () => close(null) }, "Cancel"), el("button", { type: "button", class: "btn primary", style: "font-size:15px;padding:10px 18px", onclick: () => { const r = body.getResult?.(); if (!r) { toast(tab === "draw" ? "Draw your signature first." : tab === "type" ? "Type your name first." : "Choose an image first."); return; } close(r); } }, "Use it"))));
      dlg.onclose = () => {
        dlg.remove();
        if (result) {
          if (initials) state.initials = result; else state.signature = result;
          try { const key = initials ? "foliopdf.initials" : "foliopdf.signature"; if (remember.checked) localStorage.setItem(key, result.src); else localStorage.removeItem(key); } catch {}
        }
        resolve(result);
      };
      document.body.append(dlg); dlg.showModal();
    });
  }
  // Restore a remembered signature (opt-in).
  try { for (const [key, prop] of [["foliopdf.signature", "signature"], ["foliopdf.initials", "initials"]]) { const src = localStorage.getItem(key); if (src && !state[prop]) dataUrlToSig(src).then((s) => { if (s) state[prop] = s; }); } } catch {}

  async function pickImage() {
    return new Promise((resolve) => { const inp = $("#imgpicker"); inp.value = ""; inp.onchange = async () => { const f = inp.files[0]; resolve(f ? await loadImageFile(f) : null); }; inp.click(); });
  }
  function destroy() { io.disconnect(); for (const p of pages) { try { p.renderTask?.cancel(); } catch {} } try { pdf.loadingTask?.destroy?.(); } catch {} }

  // ------------------------------------------------------------ writing into the PDF
  /**
   * Writes the items and form values into `doc` (a PdfDocument for the same
   * file). `flatten`: burn everything into the page content. Returns the
   * number of things written.
   */
  function apply(doc, { flatten = true, author = "", fill = "#000000", strip = false, cropAll = true } = {}) {
    const meta = { modified: pdfDate(), ...(author ? { author } : {}) };
    const created = [];
    let n = 0;
    if (mode === "crop") {
      const c = state.items.find((i) => i.kind === "crop"); if (!c) return 0;
      const rect = { x0: c.x, y0: c.y, x1: c.x + c.w, y1: c.y + c.h };
      if (cropAll) {
        // Same area on every page, clipped to each page's own size.
        for (const p of pages) { const r = { x0: Math.min(rect.x0, p.w - 1), y0: Math.min(rect.y0, p.h - 1), x1: Math.min(rect.x1, p.w), y1: Math.min(rect.y1, p.h) }; if (r.x1 - r.x0 > 1 && r.y1 - r.y0 > 1) doc.cropPages(String(p.index + 1), r); }
        return pages.length;
      }
      doc.cropPages(String(c.page + 1), rect); return 1;
    }
    if (mode === "redact") {
      const byPage = new Map();
      for (const it of state.items) if (it.kind === "redact") { if (!byPage.has(it.page)) byPage.set(it.page, []); byPage.get(it.page).push({ x0: it.x, y0: it.y, x1: it.x + it.w, y1: it.y + it.h }); }
      const total = { glyphsRemoved: 0, imagesRemoved: 0, imagesEdited: 0, pathsRemoved: 0, annotationsRemoved: 0, formsEdited: 0, warnings: [] };
      for (const [page, rects] of byPage) {
        const r = doc.redact(page, rects, { fill: fill === "none" ? null : hexToRgb(fill) });
        for (const k of Object.keys(total)) if (k !== "warnings") total[k] += r[k] || 0;
        total.warnings.push(...(r.warnings || []));
        n += rects.length;
      }
      if (strip) doc.stripMetadata();
      api.lastReport = total;
      return n;
    }
    if (mode === "forms") {
      const vals = {};
      for (const f of fields) {
        const v = state.values[f.name];
        if (v == null) continue;
        if (f.kind === "checkbox" || f.kind === "radio") vals[f.name] = v;
        else if (f.kind === "list") vals[f.name] = Array.isArray(v) ? v : [v];
        else vals[f.name] = String(v);
      }
      if (Object.keys(vals).length) { doc.setFields(vals); n += Object.keys(vals).length; }
    }
    for (const it of state.items) {
      const rgb = hexToRgb(it.color || "#111111");
      let id = null;
      switch (it.kind) {
        case "text": case "date": if (!it.text?.trim()) break; id = doc.addAnnotation(it.page, { kind: "free-text", rect: { x0: it.x, y0: it.y, x1: it.x + it.w, y1: it.y + it.h }, text: it.text, font: it.font || "Helvetica", size: it.size, color: rgb, border: null, background: null }, meta); break;
        case "check": { const p = [[it.x + it.w * 0.15, it.y + it.h * 0.55], [it.x + it.w * 0.4, it.y + it.h * 0.85], [it.x + it.w * 0.88, it.y + it.h * 0.15]].map(([x, y]) => ({ x, y })); id = doc.addAnnotation(it.page, { kind: "ink", paths: [p], color: rgb, width: Math.max(1, it.h * 0.11) }, meta); break; }
        case "cross": { const a = [{ x: it.x + it.w * 0.15, y: it.y + it.h * 0.15 }, { x: it.x + it.w * 0.85, y: it.y + it.h * 0.85 }], b = [{ x: it.x + it.w * 0.85, y: it.y + it.h * 0.15 }, { x: it.x + it.w * 0.15, y: it.y + it.h * 0.85 }]; id = doc.addAnnotation(it.page, { kind: "ink", paths: [a, b], color: rgb, width: Math.max(1, it.h * 0.11) }, meta); break; }
        case "dot": id = doc.addAnnotation(it.page, { kind: "circle", rect: { x0: it.x + it.w * 0.2, y0: it.y + it.h * 0.2, x1: it.x + it.w * 0.8, y1: it.y + it.h * 0.8 }, stroke: null, fill: rgb, width: 0 }, meta); break;
        case "sign": case "initials": case "image": id = doc.addImageAnnotation(it.page, { x0: it.x, y0: it.y, x1: it.x + it.w, y1: it.y + it.h }, it.png, 1, meta); break;
        case "rect": id = doc.addAnnotation(it.page, { kind: "square", rect: { x0: it.x, y0: it.y, x1: it.x + it.w, y1: it.y + it.h }, stroke: rgb, fill: it.fill ? rgb : null, width: it.width, opacity: it.fill ? 0.5 : 1 }, meta); break;
        case "ellipse": id = doc.addAnnotation(it.page, { kind: "circle", rect: { x0: it.x, y0: it.y, x1: it.x + it.w, y1: it.y + it.h }, stroke: rgb, fill: it.fill ? rgb : null, width: it.width, opacity: it.fill ? 0.5 : 1 }, meta); break;
        case "line": id = doc.addAnnotation(it.page, { kind: "line", from: { x: it.x1, y: it.y1 }, to: { x: it.x2, y: it.y2 }, color: rgb, width: it.width }, meta); break;
        case "pen": id = doc.addAnnotation(it.page, { kind: "ink", paths: [it.points.map(([x, y]) => ({ x, y }))], color: rgb, width: it.width }, meta); break;
        case "highlight": id = doc.addAnnotation(it.page, { kind: "highlight", quads: it.quads, color: rgb }, meta); break;
        case "underline": id = doc.addAnnotation(it.page, { kind: "underline", quads: it.quads, color: rgb }, meta); break;
        case "strike": id = doc.addAnnotation(it.page, { kind: "strike-out", quads: it.quads, color: rgb }, meta); break;
        case "note": id = doc.addAnnotation(it.page, { kind: "note", at: { x: it.x, y: it.y }, color: rgb }, { ...meta, contents: it.contents || "" }); break;
      }
      if (id != null) { created.push(id); n++; }
    }
    if (flatten) {
      if (created.length) doc.flattenAnnotations(null, { objects: created });
      if (mode === "forms") doc.flattenFields();
    }
    return n;
  }

  // Keyboard: delete the selection, undo/redo, escape back to the select tool.
  column.addEventListener("keydown", (e) => {
    const editing = e.target instanceof Element && e.target.closest("input,textarea,select,[contenteditable]");
    if (editing) return;
    if ((e.key === "Delete" || e.key === "Backspace") && state.selected != null) { e.preventDefault(); deleteSelected(); }
    else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "z") { e.preventDefault(); if (e.shiftKey) redo(); else undo(); }
    else if (e.key === "Escape") { if (state.selected != null) select(null); else if (state.tool !== "select") setTool("select"); e.stopPropagation(); }
  });
  renderToolbar(); renderProps(); updateCursor();
  // Lay out once mounted. A MessageChannel hop (not requestAnimationFrame,
  // which never fires in a hidden tab) lets the caller attach `root` first.
  await new Promise((r) => { const c = new MessageChannel(); c.port1.onmessage = () => r(); c.port2.postMessage(0); });
  api.mounted = () => { fitWidth(); renderAll(); };
  return api;
}

// ------------------------------------------------------------ image helpers
function trimCanvas(cv) {
  const ctx = cv.getContext("2d"); const { width: w, height: h } = cv;
  const d = ctx.getImageData(0, 0, w, h).data;
  let x0 = w, y0 = h, x1 = -1, y1 = -1;
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) { if (d[(y * w + x) * 4 + 3] > 8) { if (x < x0) x0 = x; if (x > x1) x1 = x; if (y < y0) y0 = y; if (y > y1) y1 = y; } }
  if (x1 < 0) return null;
  const pad = 6; x0 = Math.max(0, x0 - pad); y0 = Math.max(0, y0 - pad); x1 = Math.min(w - 1, x1 + pad); y1 = Math.min(h - 1, y1 + pad);
  const out = el("canvas", { width: x1 - x0 + 1, height: y1 - y0 + 1 });
  out.getContext("2d").drawImage(cv, x0, y0, out.width, out.height, 0, 0, out.width, out.height);
  const src = out.toDataURL("image/png");
  return { src, png: dataUrlBytes(src), w: out.width, h: out.height, ratio: out.width / out.height };
}
function dataUrlBytes(src) { const b = atob(src.split(",")[1]); const u = new Uint8Array(b.length); for (let i = 0; i < b.length; i++) u[i] = b.charCodeAt(i); return u; }
async function dataUrlToSig(src) { try { const img = await loadImage(src); return { src, png: dataUrlBytes(src), w: img.naturalWidth, h: img.naturalHeight, ratio: img.naturalWidth / Math.max(1, img.naturalHeight) }; } catch { return null; } }
function loadImage(src) { return new Promise((res, rej) => { const i = new Image(); i.onload = () => res(i); i.onerror = rej; i.src = src; }); }
async function loadImageFile(file) {
  const bytes = new Uint8Array(await file.arrayBuffer());
  const url = URL.createObjectURL(file);
  try {
    const img = await loadImage(url);
    // Re-encode as PNG so any input (including progressive or CMYK JPEGs) is accepted by the engine.
    const cv = el("canvas", { width: img.naturalWidth, height: img.naturalHeight }); cv.getContext("2d").drawImage(img, 0, 0);
    const isJpeg = file.type === "image/jpeg";
    const src = isJpeg ? cv.toDataURL("image/jpeg", 0.92) : cv.toDataURL("image/png");
    return { src, png: isJpeg ? dataUrlBytes(src) : dataUrlBytes(src), w: img.naturalWidth, h: img.naturalHeight, ratio: img.naturalWidth / Math.max(1, img.naturalHeight), original: bytes };
  } finally { URL.revokeObjectURL(url); }
}
