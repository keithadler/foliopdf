// Batch & presets: a visual builder for reusable recipes. A preset is a list of
// steps plus output settings; it is saved as JSON in this browser and runs
// unchanged with the `folio` command-line tool.
import { $, el, plural, toast, friendly, field, check, segmented, anchorPicker, swatches, passwordInput, cta, notice, rangeField, runJob, setProgress, ready, summaryLine, dropzone, fileList, put, renderStage, getFiles, runBatch, PresetStore } from "./app.js?v=dev";

const KEY = "foliopdf.presets";
const STEP_TYPES = [
  ["select-pages", "Keep only some pages", "Keep the listed pages, in that order."],
  ["delete-pages", "Delete pages", "Remove the listed pages."],
  ["rotate", "Rotate", "Turn pages by 90, 180 or 270 degrees."],
  ["resize", "Page size", "Change pages to A4, Letter or another size."],
  ["scale", "Scale", "Shrink or enlarge pages by a factor."],
  ["reverse", "Reverse order", "Last page first."],
  ["blank-pages", "Blank pages", "Insert empty pages."],
  ["stamp-text", "Text watermark", "Stamp text like DRAFT on pages."],
  ["stamp-image", "Image or logo", "Stamp a PNG or JPEG on pages."],
  ["page-numbers", "Page numbers", "Add page numbers."],
  ["metadata", "Document info", "Set title, author, subject, keywords."],
  ["strip-metadata", "Wipe hidden metadata", "Remove XMP, info and thumbnails."],
  ["compress-images", "Shrink images", "Downsample and re-encode pictures and scans (lossy)."],
  ["flatten", "Flatten", "Burn form fields and comments into the pages."],
  ["nup", "N-up", "2 or 4 pages per sheet."],
  ["booklet", "Booklet", "Fold-in-half order, 2-up."],
  ["split", "Split into files", "Must be the last step."],
];
const DEFAULTS = {
  "select-pages": () => ({ op: "select-pages", pages: "1-" }),
  "delete-pages": () => ({ op: "delete-pages", pages: "" }),
  rotate: () => ({ op: "rotate", degrees: 90 }),
  resize: () => ({ op: "resize", size: "a4", mode: "fit" }),
  scale: () => ({ op: "scale", factor: 0.5 }),
  reverse: () => ({ op: "reverse" }),
  "blank-pages": () => ({ op: "blank-pages", count: 1 }),
  "stamp-text": () => ({ op: "stamp-text", text: "DRAFT", size: 72, opacity: 0.3, rotation: 45, color: [0.48, 0.48, 0.48], position: "center" }),
  "stamp-image": () => ({ op: "stamp-image", asset: "image", width: 120, opacity: 1, position: "bottom-right" }),
  "page-numbers": () => ({ op: "page-numbers", format: "{page} / {pages}", position: "bottom-center", size: 10, startAt: 1 }),
  metadata: () => ({ op: "metadata" }),
  "strip-metadata": () => ({ op: "strip-metadata" }),
  "compress-images": () => ({ op: "compress-images", maxDpi: 150, quality: 75 }),
  flatten: () => ({ op: "flatten" }),
  nup: () => ({ op: "nup", perSheet: 2, sheet: "letter" }),
  booklet: () => ({ op: "booklet", sheet: "letter" }),
  split: () => ({ op: "split", every: 1 }),
};
const state = { store: null, name: null, preset: null, showJson: false, image: null, imageName: "" };

function loadStore() { try { const j = localStorage.getItem(KEY); return j ? PresetStore.fromJson(j) : PresetStore.withBuiltins(); } catch { return PresetStore.withBuiltins(); } }
function persist() { try { localStorage.setItem(KEY, state.store.toJson()); } catch {} }
function select(name) { state.name = name; state.preset = JSON.parse(JSON.stringify(state.store.get(name) ?? { name })); state.preset.steps ??= []; state.preset.output ??= {}; }
function fresh() { state.name = null; state.preset = { name: "", description: "", mode: "each", steps: [], output: { filename: "{stem}.pdf", compress: true } }; }
function download(name, text) { const a = el("a", { href: URL.createObjectURL(new Blob([text], { type: "application/json" })), download: name }); document.body.append(a); a.click(); a.remove(); }
const hex = (rgb) => "#" + rgb.map((v) => Math.round(v * 255).toString(16).padStart(2, "0")).join("");

export function batchStage() {
  if (!state.store) state.store = loadStore();
  if (!state.preset) { const names = state.store.names(); if (names.length) select(names[0]); else fresh(); }
  const p = state.preset;
  put(dropzone(), fileList(), summaryLine());

  // Preset chooser
  const sel = el("select", { "aria-label": "Saved presets", onchange: (e) => { select(e.target.value); renderStage(); } });
  for (const n of state.store.names()) sel.append(el("option", { value: n, selected: n === state.name }, n));
  if (!state.name) sel.append(el("option", { value: "", selected: true }, "New preset (unsaved)"));
  const importer = el("input", { type: "file", accept: "application/json,.json", class: "sr", onchange: async (e) => { const f = e.target.files[0]; if (!f) return; try { const txt = await f.text(); const j = JSON.parse(txt); if (j.presets) { const s = PresetStore.fromJson(txt); for (const n of s.names()) state.store.add(s.get(n)); toast(`Imported ${plural(s.names().length, "preset")}.`); } else { state.store.add(j); select(j.name); toast(`Imported “${j.name}”.`); } persist(); renderStage(); } catch (err) { toast("Couldn't import: " + friendly(err)); } } });
  put(el("div", { class: "panel" }, el("h3", {}, "Presets are recipes you can reuse: a list of steps plus how the output is saved. They stay in this browser unless you export them."),
    el("div", { class: "row", style: "align-items:flex-end" }, field("Saved presets", sel),
      el("button", { class: "btn small", onclick: () => { fresh(); renderStage(); } }, "New"),
      el("button", { class: "btn small", onclick: () => { state.preset = { ...p, name: p.name + " copy" }; state.name = null; renderStage(); } }, "Duplicate"),
      el("button", { class: "btn small", disabled: !state.name, onclick: () => { if (!confirm(`Delete “${state.name}”? This can't be undone.`)) return; state.store.remove(state.name); persist(); state.preset = null; state.name = null; renderStage(); } }, "Delete"),
      el("button", { class: "btn small", onclick: () => download((p.name || "preset") + ".json", JSON.stringify(p, null, 2)) }, "Export"),
      el("button", { class: "btn small", onclick: () => importer.click() }, "Import"), importer)));

  // Basics
  const basics = el("div", { class: "panel" }, el("h3", {}, "About this preset"),
    el("div", { class: "row" }, field("Name", el("input", { type: "text", value: p.name || "", placeholder: "e.g. client-pack", oninput: (e) => (p.name = e.target.value) })), field("Description (optional)", el("input", { type: "text", value: p.description || "", oninput: (e) => (p.description = e.target.value) }))),
    el("div", { class: "row" }, field("When several files are added", segmented([["each", "Process each file separately"], ["merge", "Merge them into one first"]], p.mode || "each", (v) => (p.mode = v), "Mode"))));
  put(basics);

  // Steps
  const steps = el("div", { class: "steps" });
  p.steps.forEach((st, i) => steps.append(stepCard(st, i, p)));
  const addMenu = el("div", { class: "row", style: "gap:8px" });
  for (const [op, label] of STEP_TYPES) addMenu.append(el("button", { class: "btn small", disabled: p.steps.some((s) => s.op === "split"), onclick: () => { p.steps.push(DEFAULTS[op]()); renderStage(); } }, "+ " + label));
  put(el("div", { class: "panel" }, el("h3", {}, p.steps.length ? `Steps (run in order)` : "Steps"), p.steps.length ? steps : el("p", { class: "summary", style: "margin:0 0 10px" }, "No steps yet. With no steps the preset just re-saves each file with the output settings below (useful for compress-only or encrypt-only recipes)."), el("p", { class: "summary", style: "margin:12px 0 6px" }, "Add a step"), addMenu, p.steps.some((s) => s.op === "split") ? el("p", { class: "summary", style: "margin-top:8px" }, "Split must be the last step, so no more steps can follow it.") : null));

  // Output
  const out = p.output; out.compress ??= true; out.filename ??= "{stem}.pdf";
  const enc = out.encryption;
  const encBox = el("div", {});
  if (enc) {
    enc.permissions ??= {};
    encBox.append(el("div", { class: "row" }, field("Password to open", passwordInput(enc.userPassword || "", (v) => (enc.userPassword = v), "Password to open"), "Empty = anyone can open."), field("Owner password", passwordInput(enc.ownerPassword || "", (v) => (enc.ownerPassword = v), "Owner password"), "Unlocks the restrictions.")),
      el("div", { class: "row" }, check("Allow copying", enc.permissions.copy !== false, (v) => { enc.permissions.copy = v; enc.permissions.accessibility = v; }), check("Allow printing", enc.permissions.print !== false, (v) => { enc.permissions.print = v; enc.permissions.printHighQuality = v; }), check("Allow editing", enc.permissions.modify !== false, (v) => { enc.permissions.modify = v; enc.permissions.annotate = v; enc.permissions.assemble = v; enc.permissions.fillForms = v; })),
      notice("warn", "Passwords are saved inside the preset in this browser. Don't export a preset with passwords to a place you don't trust."));
  }
  put(el("div", { class: "panel" }, el("h3", {}, "Output"),
    el("div", { class: "row" }, field("File name", el("input", { type: "text", value: out.filename, spellcheck: "false", oninput: (e) => (out.filename = e.target.value) }), "{stem} = original name · {index}/{total} = part numbers when splitting · {n} = running number"),
      field("Compression", segmented([[true, "Compress (recommended)"], [false, "Leave as is"]], out.compress !== false, (v) => (out.compress = v), "Compression"))),
    el("div", { class: "row" }, check("Encrypt the output", !!enc, (v) => { if (v) out.encryption = { userPassword: "", ownerPassword: "", method: "aes-256", permissions: {} }; else delete out.encryption; renderStage(); })), encBox));

  // JSON view
  const ta = el("textarea", { rows: 14, spellcheck: "false", style: "font: 13px/1.4 ui-monospace, SFMono-Regular, Menlo, monospace; width:100%", "aria-label": "Preset JSON" }, JSON.stringify(p, null, 2));
  put(el("div", { class: "panel" }, el("div", { class: "row", style: "justify-content:space-between" }, el("h3", { style: "margin:0" }, "Advanced: the preset as JSON"), el("button", { class: "btn small", onclick: () => { state.showJson = !state.showJson; renderStage(); } }, state.showJson ? "Hide JSON" : "Show JSON")),
    state.showJson ? el("div", { style: "margin-top:10px" }, ta, el("div", { class: "row", style: "margin-top:8px" }, el("button", { class: "btn small", onclick: () => { try { const j = JSON.parse(ta.value); state.preset = j; j.steps ??= []; j.output ??= {}; renderStage(); toast("Applied."); } catch (e) { toast("That isn't valid JSON: " + e.message); } } }, "Apply JSON"), el("span", { class: "summary" }, "The same JSON runs with: folio batch preset.json *.pdf"))) : null));

  // Save + run
  const saveBtn = el("button", { class: "btn", onclick: () => { try { if (!p.name?.trim()) throw new Error("Give the preset a name first."); const copy = JSON.parse(JSON.stringify(p)); state.store.add(copy); persist(); state.name = p.name; toast(`Saved “${p.name}”.`); renderStage(); } catch (e) { toast(friendly(e)); } } }, state.name === p.name ? "Save changes" : "Save preset");
  const n = ready().length;
  const needsImage = p.steps.some((s) => s.op === "stamp-image") && !state.image;
  put(el("div", { class: "cta", style: "flex-direction:row;flex-wrap:wrap" }, saveBtn, el("button", { class: "btn primary", disabled: n === 0 || needsImage, onclick: () => runJob("Running preset", async () => {
    const preset = JSON.parse(JSON.stringify(p)); preset.name ||= "untitled";
    const assets = state.image ? [{ name: "image", data: state.image }] : [];
    const r = runBatch(preset, ready().map((f) => ({ name: f.file.name, data: f.bytes, password: f.pw || undefined })), assets);
    if (r.warnings.length) toast(r.warnings[0]);
    return r.outputs.map((x) => ({ name: x.name, data: x.data, pages: x.pages }));
  }) }, `Run on ${plural(n, "file")}`)),
    n === 0 ? el("p", { class: "why", style: "text-align:center;color:var(--muted);font-size:14px" }, "Add files at the top to run this preset.") : needsImage ? el("p", { class: "why", style: "text-align:center;color:var(--muted);font-size:14px" }, "Choose an image in the image step first.") : null);
}

function stepCard(st, i, p) {
  const title = STEP_TYPES.find((t) => t[0] === st.op)?.[1] || st.op;
  const body = el("div", { class: "stepbody" });
  const pagesField = (label = "Which pages") => field(label, el("input", { type: "text", value: st.pages || "", placeholder: "all pages (or e.g. 1-3, odd, last)", spellcheck: "false", oninput: (e) => { st.pages = e.target.value.trim() || undefined; if (!st.pages) delete st.pages; } }));
  switch (st.op) {
    case "select-pages": body.append(el("div", { class: "row" }, field("Pages to keep, in order", el("input", { type: "text", value: st.pages || "", placeholder: "e.g. 1-3, last", spellcheck: "false", oninput: (e) => (st.pages = e.target.value) })))); break;
    case "delete-pages": body.append(el("div", { class: "row" }, field("Pages to delete", el("input", { type: "text", value: st.pages || "", placeholder: "e.g. 2, 5-7", spellcheck: "false", oninput: (e) => (st.pages = e.target.value) })))); break;
    case "rotate": body.append(el("div", { class: "row" }, field("Direction", segmented([[90, "↻ 90°"], [180, "180°"], [270, "↺ 90°"]], st.degrees ?? 90, (v) => (st.degrees = v), "Rotation")), pagesField())); break;
    case "resize": { const land = (st.size || "").endsWith("-landscape"); const base = (st.size || "a4").replace("-landscape", ""); body.append(el("div", { class: "row" }, field("Size", segmented([["a4", "A4"], ["letter", "Letter"], ["legal", "Legal"], ["a3", "A3"], ["a5", "A5"], ["tabloid", "Tabloid"]], base, (v) => (st.size = v + (land ? "-landscape" : "")), "Page size")), field("Orientation", segmented([[false, "Portrait"], [true, "Landscape"]], land, (v) => (st.size = base + (v ? "-landscape" : "")), "Orientation")), field("If the shape differs", segmented([["fit", "Fit"], ["fill", "Fill"], ["stretch", "Stretch"]], st.mode || "fit", (v) => (st.mode = v), "Fit mode")), pagesField())); break; }
    case "scale": body.append(el("div", { class: "row" }, field("Factor", el("input", { type: "number", min: 0.1, max: 4, step: 0.05, value: st.factor ?? 0.5, oninput: (e) => (st.factor = +e.target.value || 1) }), "0.5 halves the pages, 2 doubles them."), pagesField())); break;
    case "reverse": body.append(el("p", { class: "summary", style: "margin:0" }, "Puts the pages in reverse order.")); break;
    case "blank-pages": body.append(el("div", { class: "row" }, field("Insert before page", el("input", { type: "number", min: 0, value: st.at ?? 0, oninput: (e) => { const v = +e.target.value || 0; if (v) st.at = v; else delete st.at; } }), "0 = add at the end."), field("How many", el("input", { type: "number", min: 1, value: st.count ?? 1, oninput: (e) => (st.count = Math.max(1, +e.target.value || 1)) })), field("Size", segmented([["", "Same as neighbour"], ["a4", "A4"], ["letter", "Letter"]], st.size || "", (v) => { if (v) st.size = v; else delete st.size; }, "Blank page size")))); break;
    case "stamp-text": body.append(
      el("div", { class: "row" }, field("Text", el("input", { type: "text", value: st.text || "", oninput: (e) => (st.text = e.target.value) }), "{page} and {pages} are replaced"), field("Size", el("input", { type: "number", min: 6, max: 300, value: st.size ?? 36, oninput: (e) => (st.size = +e.target.value || 36) })), field("Opacity", el("input", { type: "range", min: 0.05, max: 1, step: 0.05, value: st.opacity ?? 0.5, oninput: (e) => (st.opacity = +e.target.value) }))),
      el("div", { class: "row" }, field("Angle", segmented([[0, "Straight"], [45, "Diagonal"], [90, "Vertical"]], st.rotation ?? 0, (v) => (st.rotation = v), "Angle")), field("Colour", swatches(hex(st.color || [0.48, 0.48, 0.48]), (rgb) => (st.color = rgb))), field("Position", anchorPicker(st.position || "center", (v) => (st.position = v))), pagesField()),
      el("div", { class: "row" }, check("Behind the content", !!st.under, (v) => (st.under = v || undefined)))); break;
    case "stamp-image": {
      const picker = el("input", { type: "file", accept: "image/png,image/jpeg", class: "sr", onchange: async (e) => { const f = e.target.files[0]; if (!f) return; state.image = new Uint8Array(await f.arrayBuffer()); state.imageName = f.name; renderStage(); } });
      body.append(el("div", { class: "row" }, field("Image", el("button", { class: "btn small", onclick: () => picker.click() }, state.image ? `Change image (${state.imageName})` : "Choose PNG or JPEG"), "Chosen at run time; images are not stored in the preset."), picker, field("Width on page", el("input", { type: "number", min: 10, max: 2000, value: st.width ?? 120, oninput: (e) => (st.width = +e.target.value || 120) }), "72 = one inch"), field("Opacity", el("input", { type: "range", min: 0.05, max: 1, step: 0.05, value: st.opacity ?? 1, oninput: (e) => (st.opacity = +e.target.value) })), field("Position", anchorPicker(st.position || "bottom-right", (v) => (st.position = v))), pagesField()), el("div", { class: "row" }, check("Behind the content", !!st.under, (v) => (st.under = v || undefined)))); st.asset = "image"; break; }
    case "page-numbers": body.append(el("div", { class: "row" }, field("Style", segmented([["{page}", "1"], ["{page} / {pages}", "1 / 12"], ["Page {page} of {pages}", "Page 1 of 12"], ["- {page} -", "- 1 -"]], st.format || "{page} / {pages}", (v) => (st.format = v), "Style")), field("Position", anchorPicker(st.position || "bottom-center", (v) => (st.position = v))), field("Size", el("input", { type: "number", min: 6, max: 36, value: st.size ?? 10, oninput: (e) => (st.size = +e.target.value || 10) })), field("First number", el("input", { type: "number", min: 0, value: st.startAt ?? 1, oninput: (e) => (st.startAt = +e.target.value || 0) })), pagesField())); break;
    case "metadata": { const f = (k, l) => field(l, el("input", { type: "text", value: st[k] || "", placeholder: "leave blank to keep", oninput: (e) => { if (e.target.value === "") delete st[k]; else st[k] = e.target.value; } })); body.append(el("div", { class: "row" }, f("title", "Title"), f("author", "Author")), el("div", { class: "row" }, f("subject", "Subject"), f("keywords", "Keywords")), el("p", { class: "summary", style: "margin:8px 0 0" }, "Blank fields are left unchanged. To erase a field, use “Wipe hidden metadata” first.")); break; }
    case "compress-images": body.append(el("div", { class: "row" }, field("Resolution", segmented([[300, "300 dpi"], [150, "150 dpi"], [96, "96 dpi"]], st.maxDpi ?? 150, (v) => (st.maxDpi = v), "Resolution"), "Images shown at more than this are scaled down."), field("JPEG quality", el("input", { type: "number", min: 1, max: 100, value: st.quality ?? 75, oninput: (e) => (st.quality = Math.min(100, Math.max(1, +e.target.value || 75)) ) })))); break;
    case "flatten": body.append(el("div", { class: "row" }, check("Form fields", st.forms !== false, (v) => (st.forms = v)), check("Comments and annotations", st.annotations !== false, (v) => (st.annotations = v)))); break;
    case "nup": body.append(el("div", { class: "row" }, field("Per sheet", segmented([[2, "2"], [4, "4"]], st.perSheet ?? 2, (v) => (st.perSheet = v), "Per sheet")), field("Sheet", segmented([["letter", "Letter"], ["a4", "A4"]], st.sheet || "letter", (v) => (st.sheet = v), "Sheet")), check("Landscape", st.landscape !== false, (v) => (st.landscape = v)))); break;
    case "booklet": body.append(el("div", { class: "row" }, field("Sheet", segmented([["letter", "Letter"], ["a4", "A4"]], st.sheet || "letter", (v) => (st.sheet = v), "Sheet")))); break;
    case "strip-metadata": body.append(el("p", { class: "summary", style: "margin:0" }, "Removes XMP metadata, the info dictionary and page thumbnails.")); break;
    case "split": { const mode = st.ranges ? "ranges" : "every"; body.append(el("div", { class: "row" }, field("How", segmented([["every", "Every N pages"], ["ranges", "Custom ranges"]], mode, (v) => { if (v === "every") { delete st.ranges; st.every = 1; } else { delete st.every; st.ranges = ["1-"]; } renderStage(); }, "Split mode")), mode === "every" ? field("Pages per file", el("input", { type: "number", min: 1, value: st.every ?? 1, oninput: (e) => (st.every = Math.max(1, +e.target.value || 1)) })) : field("Ranges, one per file, separated by commas", el("input", { type: "text", value: (st.ranges || []).join(", "), spellcheck: "false", oninput: (e) => (st.ranges = e.target.value.split(",").map((x) => x.trim()).filter(Boolean)) })))); break; }
  }
  return el("div", { class: "step" }, el("div", { class: "stephead" }, el("b", {}, `${i + 1}. ${title}`), el("div", { class: "actions" },
    el("button", { class: "iconbtn", title: "Move up", "aria-label": "Move step up", disabled: i === 0, onclick: () => { [p.steps[i - 1], p.steps[i]] = [p.steps[i], p.steps[i - 1]]; renderStage(); } }, "↑"),
    el("button", { class: "iconbtn", title: "Move down", "aria-label": "Move step down", disabled: i === p.steps.length - 1 || p.steps[i + 1]?.op === "split", onclick: () => { [p.steps[i + 1], p.steps[i]] = [p.steps[i], p.steps[i + 1]]; renderStage(); } }, "↓"),
    el("button", { class: "iconbtn", title: "Remove step", "aria-label": "Remove step", onclick: () => { p.steps.splice(i, 1); renderStage(); } }, "✕"))), body);
}
