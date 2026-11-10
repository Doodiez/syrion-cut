const { invoke, convertFileSrc } = window.__TAURI__.core;
const { open } = window.__TAURI__.dialog;
const { openPath, revealItemInDir } = window.__TAURI__.opener;
const { getCurrentWebview } = window.__TAURI__.webview;

const $ = (id) => document.getElementById(id);

// ---- settings (remembered between launches) ----
const store = {
  get(k, d) { try { const v = localStorage.getItem(k); return v === null ? d : JSON.parse(v); } catch { return d; } },
  set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch {} },
};
const settings = {
  cleanup: store.get("cleanup", 20),
  fixEdges: store.get("fixEdges", true),
  shrink: store.get("shrink", 0),
  soften: store.get("soften", 0),
  upscale: store.get("upscale", 1),
  outline: store.get("outline", false),
  outlineColor: store.get("outlineColor", "#ffffff"),
  outlineSize: store.get("outlineSize", 6),
  autoCrop: store.get("autoCrop", false),
  padding: store.get("padding", 20),
  outputDir: store.get("outputDir", null),
};

function renderSettings() {
  $("cleanup").value = settings.cleanup;
  $("cleanupVal").textContent = settings.cleanup === 0 ? "Off" : settings.cleanup;
  $("fixEdges").checked = settings.fixEdges;
  $("shrink").value = settings.shrink;
  $("shrinkVal").textContent = settings.shrink ? `${settings.shrink}px` : "Off";
  $("soften").value = settings.soften;
  $("softenVal").textContent = settings.soften ? `${settings.soften}px` : "Off";
  $("upscale").value = String(settings.upscale);
  $("outline").checked = settings.outline;
  $("outlineColor").value = settings.outlineColor;
  $("outlineColor").parentElement.style.background = settings.outlineColor;
  $("outlineSize").value = settings.outlineSize;
  $("outlineFields").style.opacity = settings.outline ? 1 : 0.45;
  $("autoCrop").checked = settings.autoCrop;
  $("padding").value = settings.padding;
  $("paddingField").style.opacity = settings.autoCrop ? 1 : 0.45;
  $("outputBtn").textContent = settings.outputDir || "“no-bg” folder next to originals";
  $("outputBtn").title = settings.outputDir || "Choose output folder";
  $("outputReset").classList.toggle("hidden", !settings.outputDir);
  renderPresets();
}
$("cleanup").oninput = (e) => { settings.cleanup = parseInt(e.target.value, 10) || 0; store.set("cleanup", settings.cleanup); renderSettings(); };
$("fixEdges").onchange = (e) => { settings.fixEdges = e.target.checked; store.set("fixEdges", settings.fixEdges); };
$("shrink").oninput = (e) => { settings.shrink = parseInt(e.target.value, 10) || 0; store.set("shrink", settings.shrink); renderSettings(); };
$("soften").oninput = (e) => { settings.soften = parseInt(e.target.value, 10) || 0; store.set("soften", settings.soften); renderSettings(); };
$("upscale").onchange = (e) => { settings.upscale = parseInt(e.target.value, 10) || 1; store.set("upscale", settings.upscale); };
$("outline").onchange = (e) => { settings.outline = e.target.checked; store.set("outline", settings.outline); renderSettings(); };
$("outlineColor").oninput = (e) => { settings.outlineColor = e.target.value; store.set("outlineColor", settings.outlineColor); renderSettings(); };
$("outlineSize").oninput = (e) => { settings.outlineSize = parseInt(e.target.value, 10) || 6; store.set("outlineSize", settings.outlineSize); };
$("autoCrop").onchange = (e) => { settings.autoCrop = e.target.checked; store.set("autoCrop", settings.autoCrop); renderSettings(); };
$("padding").onchange = (e) => {
  settings.padding = Math.max(0, Math.min(500, parseInt(e.target.value, 10) || 0));
  store.set("padding", settings.padding); renderSettings();
};
$("outputBtn").onclick = async () => {
  const dir = await open({ directory: true, title: "Save cut-outs to…" });
  if (dir) { settings.outputDir = dir; store.set("outputDir", dir); renderSettings(); }
};
$("outputReset").onclick = () => { settings.outputDir = null; store.set("outputDir", null); renderSettings(); };

// ---- presets ----
const PRESET_KEYS = ["cleanup", "fixEdges", "shrink", "soften", "upscale", "outline", "outlineColor", "outlineSize", "autoCrop", "padding"];
const STANDARD = {
  cleanup: 20, fixEdges: true, shrink: 0, soften: 0, upscale: 1,
  outline: false, outlineColor: "#ffffff", outlineSize: 6, autoCrop: false, padding: 20,
};
const BUILT_IN = [
  { name: "Standard", values: STANDARD },
  { name: "Clean product edges", values: { ...STANDARD, shrink: 1, soften: 1, autoCrop: true } },
  { name: "Glass & see-through", values: { ...STANDARD, cleanup: 0 } },
  { name: "Small photo → poster", values: { ...STANDARD, upscale: 4, shrink: 1, autoCrop: true } },
  { name: "Sticker", values: { ...STANDARD, outline: true, autoCrop: true, padding: 10 } },
];
let userPresets = store.get("presets", []); // [{name, values}]

const allPresets = () => [...BUILT_IN, ...userPresets];
const matches = (p) => PRESET_KEYS.every((k) => p.values[k] === settings[k]);

function renderPresets() {
  const sel = $("preset");
  const current = allPresets().find(matches);
  sel.innerHTML = "";
  const add = (label, value, group) => {
    const o = document.createElement("option");
    o.textContent = label; o.value = value;
    (group || sel).append(o);
  };
  if (!current) add("Custom", "");
  for (const p of BUILT_IN) add(p.name, "b:" + p.name);
  if (userPresets.length) {
    const g = document.createElement("optgroup");
    g.label = "My presets";
    for (const p of userPresets) add(p.name, "u:" + p.name, g);
    sel.append(g);
  }
  sel.value = current ? (BUILT_IN.includes(current) ? "b:" : "u:") + current.name : "";
  $("presetDelete").classList.toggle("hidden", !(current && userPresets.includes(current)));
}

function applyPreset(p) {
  for (const k of PRESET_KEYS) { settings[k] = p.values[k]; store.set(k, settings[k]); }
  renderSettings();
}

$("preset").onchange = (e) => {
  const v = e.target.value;
  const p = v.startsWith("b:") ? BUILT_IN.find((x) => "b:" + x.name === v) : userPresets.find((x) => "u:" + x.name === v);
  if (p) applyPreset(p);
};
$("presetSave").onclick = () => {
  $("presetForm").classList.remove("hidden");
  $("presetSave").classList.add("hidden");
  $("presetName").value = "";
  $("presetName").focus();
};
const closePresetForm = () => { $("presetForm").classList.add("hidden"); $("presetSave").classList.remove("hidden"); };
$("presetCancel").onclick = closePresetForm;
$("presetName").onkeydown = (e) => { if (e.key === "Escape") closePresetForm(); };
$("presetForm").onsubmit = (e) => {
  e.preventDefault();
  let name = $("presetName").value.trim();
  if (!name) return;
  if (BUILT_IN.some((p) => p.name === name)) name += " (mine)";
  const values = Object.fromEntries(PRESET_KEYS.map((k) => [k, settings[k]]));
  const existing = userPresets.find((p) => p.name === name);
  if (existing) existing.values = values; else userPresets.push({ name, values });
  store.set("presets", userPresets);
  closePresetForm();
  renderPresets();
};
$("presetDelete").onclick = () => {
  const p = userPresets.find(matches);
  if (!p) return;
  userPresets = userPresets.filter((x) => x !== p);
  store.set("presets", userPresets);
  renderPresets();
};
// Any settings tweak may turn the preset into "Custom" (or match another one).
document.querySelectorAll(".settings").forEach((s) => {
  s.addEventListener("input", renderPresets);
  s.addEventListener("change", renderPresets);
});

// ---- queue ----
const jobs = []; // {id, path, name, status, thumb, result, error}
let selected = null;
let running = false;
let paused = false;
let nextId = 1;

const baseName = (p) => p.split(/[\\/]/).pop();

async function addPaths(paths) {
  const files = await invoke("expand_paths", { paths });
  const known = new Set(jobs.filter((j) => j.status !== "done" && j.status !== "error").map((j) => j.path));
  const fresh = files.filter((f) => !known.has(f));
  for (const path of fresh) {
    const job = { id: nextId++, path, name: baseName(path), status: "queued" };
    jobs.push(job);
    invoke("thumbnail", { path }).then((t) => { job.thumb = t; renderList(); }).catch(() => {});
  }
  if (fresh.length && selected === null) selected = jobs[jobs.length - fresh.length].id;
  if (fresh.length) paused = false;
  render();
  run();
}

async function run() {
  if (running) return;
  running = true;
  let job;
  while (!paused && (job = jobs.find((j) => j.status === "queued"))) {
    job.status = "working";
    render();
    try {
      job.result = await invoke("remove_background", {
        path: job.path,
        options: {
          cleanup: settings.cleanup,
          fixEdges: settings.fixEdges,
          shrink: settings.shrink,
          soften: settings.soften,
          upscale: settings.upscale,
          outline: settings.outline,
          outlineSize: settings.outlineSize,
          outlineColor: settings.outlineColor,
          autoCrop: settings.autoCrop,
          padding: settings.padding,
          outputDir: settings.outputDir,
        },
      });
      job.status = "done";
      job.version = (job.version || 0) + 1;
    } catch (e) {
      job.status = "error";
      job.error = String(e);
    }
    render();
  }
  running = false;
  render();
}

function statusText(j) {
  switch (j.status) {
    case "queued": return paused ? "Paused" : "Waiting…";
    case "working": return settings.upscale > 1 ? "Cutting out & upscaling…" : "Removing background…";
    case "done": return `Done · ${(j.result.millis / 1000).toFixed(1)}s`;
    case "error": return "Failed";
  }
}

function renderList() {
  const list = $("list");
  list.innerHTML = "";
  for (const j of jobs) {
    const li = document.createElement("li");
    if (j.id === selected) li.className = "selected";
    const img = document.createElement("img");
    img.className = "thumb";
    if (j.result) img.src = j.result.preview; else if (j.thumb) img.src = j.thumb;
    const meta = document.createElement("div");
    meta.className = "meta";
    meta.innerHTML = `<div class="name"></div><div class="status ${j.status}"></div>`;
    meta.querySelector(".name").textContent = j.name;
    meta.querySelector(".status").textContent = statusText(j);
    li.append(img, meta);
    li.onclick = () => { selected = j.id; render(); };
    list.append(li);
  }
  const done = jobs.filter((j) => j.status === "done").length;
  $("queueCount").textContent = jobs.length ? `${done} of ${jobs.length} done` : "No images yet";
  $("clearDone").classList.toggle("hidden", !jobs.some((j) => j.status === "done" || j.status === "error"));
  renderBatch();
}

function renderBatch() {
  const total = jobs.length;
  const finished = jobs.filter((j) => j.status === "done" || j.status === "error").length;
  const waiting = jobs.filter((j) => j.status === "queued").length;
  const working = jobs.some((j) => j.status === "working");
  const failed = jobs.filter((j) => j.status === "error").length;
  $("batch").classList.toggle("hidden", total === 0);
  $("batch").classList.toggle("paused", paused && waiting > 0);
  $("progressFill").style.width = total ? `${(finished / total) * 100}%` : "0";

  let text;
  if (working || (waiting && !paused)) {
    const times = jobs.filter((j) => j.status === "done").map((j) => j.result.millis);
    const avg = times.length ? times.reduce((a, b) => a + b, 0) / times.length : 0;
    const left = avg ? Math.round((avg * (waiting + (working ? 0.5 : 0))) / 1000) : 0;
    text = `Processing ${Math.min(finished + 1, total)} of ${total}`;
    if (left > 0) text += left < 60 ? ` · ~${left}s left` : ` · ~${Math.round(left / 60)} min left`;
    if (paused) text = "Stopping after this image…";
  } else if (waiting) {
    text = `Stopped · ${waiting} waiting`;
  } else {
    text = failed ? `All finished · ${failed} failed` : `All ${total} done`;
  }
  $("batchText").textContent = text;

  $("pauseBtn").classList.toggle("hidden", waiting === 0);
  $("pauseBtn").textContent = paused ? "Resume" : "Stop";
  $("redoAll").classList.toggle("hidden", working || (waiting > 0 && !paused) || finished === 0);
}

$("pauseBtn").onclick = () => {
  paused = !paused;
  render();
  if (!paused) run();
};
$("redoAll").onclick = () => {
  for (const j of jobs) if (j.status === "done" || j.status === "error") { j.status = "queued"; j.error = undefined; }
  paused = false;
  render();
  run();
};

function renderViewer() {
  const j = jobs.find((x) => x.id === selected);
  $("empty").classList.toggle("hidden", !!j);
  $("viewer").classList.toggle("hidden", !j);
  if (!j) return;
  $("viewName").textContent = j.name;
  showInViewer(j);
  $("viewImg").style.opacity = j.result ? 1 : 0.35;
  $("viewBusy").classList.toggle("hidden", !(j.status === "working" || j.status === "queued"));
  $("viewBusy").lastChild.textContent = j.status === "queued" ? "Waiting in queue…" : statusText(j);
  $("viewError").classList.toggle("hidden", j.status !== "error");
  $("viewError").textContent = j.error || "";
  $("viewInfo").textContent = j.result ? `${j.result.width} × ${j.result.height}px · saved as ${baseName(j.result.output)}` : "";
  $("viewInfo").title = j.result ? j.result.output : "";
  $("openFile").disabled = $("showFolder").disabled = !j.result;
  $("redo").disabled = !(j.status === "done" || j.status === "error");
}

function render() { renderList(); renderViewer(); }

$("redo").onclick = () => {
  const j = jobs.find((x) => x.id === selected);
  if (!j || j.status === "queued" || j.status === "working") return;
  j.status = "queued";
  j.error = undefined;
  render();
  run();
};
$("openFile").onclick = () => { const j = jobs.find((x) => x.id === selected); if (j?.result) openPath(j.result.output); };
$("showFolder").onclick = () => { const j = jobs.find((x) => x.id === selected); if (j?.result) revealItemInDir(j.result.output); };
$("clearDone").onclick = () => {
  for (let i = jobs.length - 1; i >= 0; i--) if (jobs[i].status === "done" || jobs[i].status === "error") jobs.splice(i, 1);
  if (!jobs.some((j) => j.id === selected)) selected = jobs[0]?.id ?? null;
  render();
};

// ---- zoom & pan ----
// view.s = screen pixels per image pixel; (x, y) = where the image's top-left sits in the canvas.
const view = { s: 1, x: 0, y: 0, fit: true, w: 0, h: 0, key: null };
const viewImg = $("viewImg");
const canvasEl = $("canvas");

function fitScale() {
  const r = canvasEl.getBoundingClientRect();
  if (!view.w || !view.h || r.width < 60 || r.height < 60) return 1;
  return Math.min((r.width - 40) / view.w, (r.height - 40) / view.h, 1);
}

function applyView() {
  if (!view.w || !view.h) return;
  if (view.fit) {
    const r = canvasEl.getBoundingClientRect();
    view.s = fitScale();
    view.x = (r.width - view.w * view.s) / 2;
    view.y = (r.height - view.h * view.s) / 2;
  }
  viewImg.style.width = stack.style.width = `${view.w}px`;
  viewImg.style.height = stack.style.height = `${view.h}px`;
  stack.style.transform = `translate(${view.x}px, ${view.y}px) scale(${view.s})`;
  // Show crisp pixels when zoomed in, so edges can be judged exactly.
  stack.style.imageRendering = view.s >= 2 ? "pixelated" : "auto";
  $("zoomPct").textContent = view.fit ? "Fit" : `${Math.round(view.s * 100)}%`;
  applyCompare();
}

// ---- before / after ----
// The original photo is placed where it sits inside the saved file (after crop / upscale / outline),
// and is shown left of the divider.
const stack = $("stack");
const origImg = $("origImg");
const divider = $("divider");
const cmp = { on: false, split: 0.5, rect: null, ok: false, key: null };

function applyCompare() {
  const show = cmp.on && cmp.ok && cmp.rect && view.w > 0;
  origImg.classList.toggle("hidden", !show);
  divider.classList.toggle("hidden", !show);
  $("compareBtn").classList.toggle("active", cmp.on);
  if (!show) return;
  const [ox, oy, ow, oh] = cmp.rect;
  origImg.style.left = `${ox}px`;
  origImg.style.top = `${oy}px`;
  origImg.style.width = `${ow}px`;
  origImg.style.height = `${oh}px`;
  const cw = canvasEl.getBoundingClientRect().width;
  const dx = cmp.split * cw;
  divider.style.left = `${dx}px`;
  const lx = (dx - view.x) / view.s; // divider position in image pixels
  const hide = Math.max(0, Math.min(ow, ox + ow - lx));
  origImg.style.clipPath = `inset(0 ${hide}px 0 0)`;
}

function loadOriginal(j) {
  const key = j.result ? `${j.id}:${j.version}` : null;
  if (cmp.key === key) return;
  cmp.key = key;
  cmp.ok = false;
  cmp.rect = j.result ? j.result.origRect : null;
  updateCompareBtn(j);
  if (!j.result) { origImg.removeAttribute("src"); return; }
  origImg.onload = () => { if (cmp.key === key) { cmp.ok = true; updateCompareBtn(j); applyView(); } };
  origImg.onerror = () => { if (cmp.key === key) { cmp.ok = false; updateCompareBtn(j, true); applyView(); } };
  origImg.src = convertFileSrc(j.path);
}

function updateCompareBtn(j, failed) {
  const btn = $("compareBtn");
  btn.disabled = !j.result || failed;
  btn.title = failed
    ? "The original photo can't be previewed (this file type isn't supported for compare)"
    : "Before / after: drag the divider to compare with the original photo (shortcut: C)";
}

function toggleCompare() {
  if ($("compareBtn").disabled) return;
  cmp.on = !cmp.on;
  applyView();
}
$("compareBtn").onclick = toggleCompare;
document.addEventListener("keydown", (e) => {
  if (e.target.closest("input, select, textarea") || e.ctrlKey || e.altKey || e.metaKey) return;
  if (e.key === "c" || e.key === "C") toggleCompare();
});

let splitDrag = false;
divider.addEventListener("pointerdown", (e) => {
  e.stopPropagation();
  splitDrag = true;
  divider.setPointerCapture(e.pointerId);
});
divider.addEventListener("pointermove", (e) => {
  if (!splitDrag) return;
  const r = canvasEl.getBoundingClientRect();
  cmp.split = Math.max(0, Math.min(1, (e.clientX - r.left) / r.width));
  applyCompare();
});
const endSplit = () => { splitDrag = false; };
divider.addEventListener("pointerup", endSplit);
divider.addEventListener("pointercancel", endSplit);
divider.addEventListener("dblclick", (e) => e.stopPropagation());

function zoomAt(newScale, mx, my) {
  const ns = Math.max(Math.min(fitScale(), 1) * 0.5, Math.min(32, newScale));
  view.x = mx - (mx - view.x) * (ns / view.s);
  view.y = my - (my - view.y) * (ns / view.s);
  view.s = ns;
  view.fit = false;
  applyView();
}
const canvasCenter = () => { const r = canvasEl.getBoundingClientRect(); return [r.width / 2, r.height / 2]; };

// Loads the full-resolution saved file for zooming (the quick preview is shown meanwhile).
function showInViewer(j) {
  loadOriginal(j);
  const key = j.result ? `${j.id}:${j.version}` : `${j.id}:thumb:${j.thumb ? 1 : 0}`;
  if (view.key === key) return;
  view.key = key;
  view.fit = true;
  if (j.result) {
    view.w = j.result.width;
    view.h = j.result.height;
    viewImg.src = j.result.preview;
    const full = `${convertFileSrc(j.result.output)}?v=${Date.now()}`;
    const loader = new Image();
    loader.onload = () => { if (view.key === key) viewImg.src = full; };
    loader.src = full;
  } else {
    view.w = view.h = 0;
    viewImg.onload = () => {
      if (view.key !== key) return;
      view.w = viewImg.naturalWidth;
      view.h = viewImg.naturalHeight;
      applyView();
    };
    viewImg.src = j.thumb || "";
  }
  applyView();
}

canvasEl.addEventListener("wheel", (e) => {
  if (!view.w) return;
  e.preventDefault();
  const r = canvasEl.getBoundingClientRect();
  zoomAt(view.s * (e.deltaY < 0 ? 1.25 : 0.8), e.clientX - r.left, e.clientY - r.top);
}, { passive: false });

let drag = null;
canvasEl.addEventListener("pointerdown", (e) => {
  if (!view.w || e.button !== 0) return;
  drag = { x: e.clientX, y: e.clientY };
  canvasEl.setPointerCapture(e.pointerId);
  canvasEl.classList.add("dragging");
});
canvasEl.addEventListener("pointermove", (e) => {
  if (!drag) return;
  view.x += e.clientX - drag.x;
  view.y += e.clientY - drag.y;
  drag = { x: e.clientX, y: e.clientY };
  view.fit = false;
  applyView();
});
const endDrag = () => { drag = null; canvasEl.classList.remove("dragging"); };
canvasEl.addEventListener("pointerup", endDrag);
canvasEl.addEventListener("pointercancel", endDrag);
canvasEl.addEventListener("dblclick", (e) => {
  if (!view.w) return;
  if (view.fit) {
    const r = canvasEl.getBoundingClientRect();
    zoomAt(1, e.clientX - r.left, e.clientY - r.top);
  } else {
    view.fit = true;
    applyView();
  }
});
$("zoomIn").onclick = () => zoomAt(view.s * 1.5, ...canvasCenter());
$("zoomOut").onclick = () => zoomAt(view.s / 1.5, ...canvasCenter());
$("zoomPct").onclick = () => {
  if (view.fit) zoomAt(1, ...canvasCenter());
  else { view.fit = true; applyView(); }
};
window.addEventListener("resize", applyView);

// ---- preview background ----
document.querySelectorAll(".bg-swatch[data-bg]").forEach((b) => {
  b.onclick = () => setBg(b.dataset.bg, b);
});
$("bgColor").oninput = (e) => setBg(e.target.value, e.target.parentElement);
function setBg(bg, el) {
  document.querySelectorAll(".bg-swatch").forEach((s) => s.classList.remove("active"));
  el.classList.add("active");
  const c = $("canvas");
  c.classList.toggle("checker", bg === "checker");
  c.style.background = bg === "checker" ? "" : bg;
}

// ---- adding files ----
$("addFiles").onclick = async () => {
  const picked = await open({
    multiple: true,
    title: "Choose product images",
    filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff"] }],
  });
  if (picked) addPaths(Array.isArray(picked) ? picked : [picked]);
};
$("addFolder").onclick = async () => {
  const dir = await open({ directory: true, title: "Choose a folder of images" });
  if (dir) addPaths([dir]);
};

getCurrentWebview().onDragDropEvent((e) => {
  const t = e.payload.type;
  if (t === "enter" || t === "over") $("dropOverlay").classList.remove("hidden");
  else $("dropOverlay").classList.add("hidden");
  if (t === "drop" && e.payload.paths?.length) addPaths(e.payload.paths);
});

// ---- start ----
function warmUp() {
  const pill = $("engine");
  pill.textContent = "Loading model…";
  pill.classList.remove("gpu");
  invoke("warm_up")
    .then((device) => {
      pill.textContent = `Ready · ${device}`;
      pill.title = device === "GPU" ? "Running on your graphics card, offline" : "Running on your processor, offline";
      pill.classList.toggle("gpu", device === "GPU");
    })
    .catch((e) => { pill.textContent = "Model failed to load"; pill.title = String(e); });
}

renderSettings();
render();
warmUp();
