// calc-oracle webui — vanilla JS SPA.

const state = {
  scenarios: [],
  results: new Map(),
  selected: null,
  mode: "detail",
  editorTab: "form",
};

const NATURES = ["Adamant","Bashful","Bold","Brave","Calm","Careful","Docile","Gentle","Hardy","Hasty","Impish","Jolly","Lax","Lonely","Mild","Modest","Naive","Naughty","Quiet","Quirky","Rash","Relaxed","Sassy","Serious","Timid"];
const TYPES = ["Normal","Fire","Water","Electric","Grass","Ice","Fighting","Poison","Ground","Flying","Psychic","Bug","Rock","Ghost","Dragon","Dark","Steel","Fairy"];
const STATUSES = ["","brn","par","psn","tox","frz","slp"];
const WEATHERS = ["","Sun","Rain","Sand","Snow"];
const TERRAINS = ["","Electric","Grassy","Psychic","Misty"];

const $ = (id) => document.getElementById(id);

async function api(path, opts) {
  const r = await fetch(path, opts);
  if (!r.ok) throw new Error(`${path}: ${r.status} ${await r.text()}`);
  const ct = r.headers.get("content-type") || "";
  return ct.includes("json") ? r.json() : r.text();
}

async function boot() {
  wireEvents();
  populateFormStatics();
  await refreshList();
  render();
}
document.addEventListener("DOMContentLoaded", boot);

async function refreshList() {
  state.scenarios = await api("/api/scenarios");
  const uniq = (key) => [...new Set(state.scenarios.map((s) => s[key]).filter(Boolean))].sort();
  fillSelect("f-attacker", uniq("attacker_species"));
  const items = [...new Set(state.scenarios.map((s) => s.attacker_item || "none"))].sort();
  fillSelect("f-item", items);
  fillSelect("f-weather", uniq("weather"));
  fillSelect("f-terrain", uniq("terrain"));
  const species = uniq("attacker_species").concat(uniq("defender_species"));
  $("dl-species").innerHTML = [...new Set(species)].sort().map((s) => `<option value="${esc(s)}">`).join("");
  const formItems = [...new Set(state.scenarios.map((s) => s.attacker_item).filter(Boolean))].sort();
  $("dl-items").innerHTML = formItems.map((i) => `<option value="${esc(i)}">`).join("");
}

function fillSelect(id, values) {
  const el = $(id);
  const cur = el.value;
  el.innerHTML = '<option value="">any</option>' + values.map((v) => `<option>${esc(v)}</option>`).join("");
  el.value = cur;
}

function populateFormStatics() {
  const natOpts = NATURES.map((n) => `<option>${n}</option>`).join("");
  $("ef-a-nature").innerHTML = natOpts;
  $("ef-d-nature").innerHTML = natOpts;
  const teraOpts = TYPES.map((t) => `<option>${t}</option>`).join("");
  $("ef-a-tera").innerHTML = teraOpts;
  $("ef-d-tera").innerHTML = teraOpts;
  const statusOpts = STATUSES.map((s) => `<option value="${s}">${s || "none"}</option>`).join("");
  $("ef-a-status").innerHTML = statusOpts;
  $("ef-d-status").innerHTML = statusOpts;
  $("ef-weather").innerHTML = WEATHERS.map((w) => `<option value="${w}">${w || "none"}</option>`).join("");
  $("ef-terrain").innerHTML = TERRAINS.map((t) => `<option value="${t}">${t || "none"}</option>`).join("");
}

function filtered() {
  const q = $("f-text").value.trim().toLowerCase();
  const atk = $("f-attacker").value;
  const itm = $("f-item").value;
  const wea = $("f-weather").value;
  const ter = $("f-terrain").value;
  const status = document.querySelector('input[name="status"]:checked').value;
  const hand = $("f-hand").checked;
  const gen = $("f-gen").checked;
  return state.scenarios.filter((s) => {
    if (q && !(s.stem.toLowerCase().includes(q) || s.name.toLowerCase().includes(q))) return false;
    if (atk && s.attacker_species !== atk) return false;
    if (itm) { if ((s.attacker_item || "none") !== itm) return false; }
    if (wea && s.weather !== wea) return false;
    if (ter && s.terrain !== ter) return false;
    if (!hand && s.source === "hand") return false;
    if (!gen && s.source === "gen") return false;
    const r = state.results.get(s.stem);
    if (status === "failing") {
      if (!r) return false;
      if (r.pass) return false;
      if (r.known_failure) return false;
    } else if (status === "passing") {
      if (!r || !r.pass) return false;
    } else if (status === "known") {
      if (!s.known_failure) return false;
    }
    return true;
  });
}

function render() {
  renderCounters();
  renderList();
  if (state.mode === "editor") renderEditor();
  else renderDetail();
}

function renderCounters() {
  const total = state.scenarios.length;
  let pass = 0, fail = 0, known = 0, unrun = 0;
  for (const s of state.scenarios) {
    const r = state.results.get(s.stem);
    if (!r) { unrun++; continue; }
    if (r.pass) pass++;
    else if (r.known_failure) known++;
    else fail++;
  }
  $("counters").textContent = `${total} scenarios · ${pass} pass · ${fail} fail · ${known} known-fail · ${unrun} un-run`;
}

function renderList() {
  const items = filtered();
  const ul = $("scenario-list");
  ul.innerHTML = items.map((s) => {
    const r = state.results.get(s.stem);
    let cls = "unrun", label = "?";
    if (r) {
      if (r.pass) { cls = "pass"; label = "OK"; }
      else if (r.known_failure) { cls = "known"; label = "known"; }
      else { cls = "fail"; label = "FAIL"; }
    } else if (s.known_failure) {
      cls = "known"; label = "known";
    }
    const sel = s.stem === state.selected ? " sel" : "";
    return `<li class="${sel.trim()}" data-stem="${esc(s.stem)}"><span class="pill ${cls}">${label}</span><span class="stem">${esc(s.stem)}</span></li>`;
  }).join("");
  for (const li of ul.querySelectorAll("li")) {
    li.addEventListener("click", () => selectStem(li.dataset.stem));
  }
}

async function selectStem(stem) {
  state.selected = stem;
  state.mode = "detail";
  if (!state.results.has(stem)) await runOne(stem);
  render();
  loadHistory(stem);
}

function renderDetail() {
  $("detail-empty").hidden = !!state.selected;
  $("editor").hidden = true;
  $("detail").hidden = !state.selected;
  if (!state.selected) return;
  const s = state.scenarios.find((x) => x.stem === state.selected);
  const r = state.results.get(state.selected);
  $("d-title").textContent = state.selected;
  const parts = [
    `name: ${s.name}`,
    `attacker: ${s.attacker_species}${s.attacker_item ? ` @ ${s.attacker_item}` : ""}   defender: ${s.defender_species}`,
    `move: ${s.move_name}`,
    `field: weather=${s.weather || "none"}, terrain=${s.terrain || "none"}`,
    `source: ${s.source}${s.known_failure ? "  [KNOWN FAILURE]" : ""}`,
  ];
  if (r && r.desc) parts.push(`desc: ${r.desc}`);
  $("d-summary").textContent = parts.join("\n");
  $("d-svg").innerHTML = r ? renderSvg(r) : "<em>running…</em>";
  $("d-values").textContent = r ? renderValues(r) : "";
  const diag = $("d-diagnose");
  if (r && !r.pass && r.diagnosis) {
    diag.hidden = false;
    $("d-diag-label").textContent = r.diagnosis;
    $("d-diag-hist").innerHTML = renderDeltaHistogram(r.delta_histogram || []);
  } else {
    diag.hidden = true;
  }
}

function renderSvg(r) {
  const max = Math.max(1, r.target_max_hp, ...r.expected_union, ...r.observed);
  const W = 700, H = 220, pad = 30;
  const barW = (W - pad * 2) / 16;
  const scaleY = (v) => H - pad - (v / max) * (H - pad * 2 - 20);
  const outOfSpec = new Set(r.out_of_spec);
  const isKnown = r.known_failure;
  let bars = "";
  for (let i = 0; i < 16; i++) {
    const v = r.observed[i] || 0;
    const oos = outOfSpec.has(v);
    const color = oos ? (isKnown ? "#c88300" : "#c22") : "#4a8";
    const y = scaleY(v);
    bars += `<rect x="${pad + i * barW + 1}" y="${y}" width="${barW - 2}" height="${H - pad - y}" fill="${color}"/>`;
    bars += `<text x="${pad + i * barW + barW / 2}" y="${H - pad + 12}" font-size="9" text-anchor="middle" fill="#888">${v}</text>`;
  }
  const calc16 = r.expected_noncrit && r.expected_noncrit.length === 16 ? r.expected_noncrit : r.expected_union.slice(0, 16);
  let ghosts = "";
  for (let i = 0; i < calc16.length; i++) {
    const y = scaleY(calc16[i]);
    ghosts += `<rect x="${pad + i * barW + 1}" y="${y - 4}" width="${barW - 2}" height="2" fill="#888" opacity="0.7"/>`;
  }
  const critArr = r.expected_crit || [];
  let crits = "";
  for (let i = 0; i < critArr.length; i++) {
    const y = scaleY(critArr[i]);
    crits += `<rect x="${pad + i * barW + barW / 2 - 1}" y="${y}" width="2" height="${H - pad - y}" fill="#aaa" opacity="0.4"/>`;
  }
  const hpY = scaleY(r.target_max_hp);
  const hpLine = `<line x1="${pad}" y1="${hpY}" x2="${W - pad}" y2="${hpY}" stroke="#a44" stroke-dasharray="4 3" stroke-width="1"/><text x="${W - pad}" y="${hpY - 3}" font-size="9" text-anchor="end" fill="#a44">max HP ${r.target_max_hp}</text>`;
  return `<svg viewBox="0 0 ${W} ${H}" width="${W}" height="${H}">${crits}${ghosts}${bars}${hpLine}</svg>`;
}

function renderDeltaHistogram(deltas) {
  if (!deltas.length) return "";
  const max = Math.max(1, ...deltas.map((d) => Math.abs(d)));
  const W = 480, H = 60, pad = 12;
  const bw = (W - pad * 2) / deltas.length;
  const mid = H / 2;
  let bars = "";
  for (let i = 0; i < deltas.length; i++) {
    const d = deltas[i];
    const h = Math.round((Math.abs(d) / max) * (mid - 4));
    const y = d >= 0 ? mid - h : mid;
    const color = d === 0 ? "#4a8" : (d > 0 ? "#c22" : "#26a");
    bars += `<rect x="${pad + i * bw + 0.5}" y="${y}" width="${Math.max(1, bw - 1)}" height="${Math.max(1, h)}" fill="${color}"><title>${d}</title></rect>`;
  }
  const zero = `<line x1="${pad}" y1="${mid}" x2="${W - pad}" y2="${mid}" stroke="#999" stroke-width="0.5"/>`;
  return `<svg viewBox="0 0 ${W} ${H}" width="${W}" height="${H}">${zero}${bars}</svg><div class="hist-values">deltas: [${deltas.join(", ")}]</div>`;
}

function renderValues(r) {
  return [
    `pass: ${r.pass}${r.known_failure ? " (known-failure)" : ""}`,
    `target_max_hp: ${r.target_max_hp}`,
    `observed: [${r.observed.join(", ")}]`,
    `expected_union: [${r.expected_union.join(", ")}]`,
    `expected_noncrit: [${(r.expected_noncrit || []).join(", ")}]`,
    `expected_crit: [${(r.expected_crit || []).join(", ")}]`,
    `out_of_spec: [${r.out_of_spec.join(", ")}]`,
    r.err ? `err: ${r.err}` : "",
  ].filter(Boolean).join("\n");
}

async function loadHistory(stem) {
  const el = $("d-history");
  el.innerHTML = "";
  try {
    const list = await api(`/api/scenarios/${stem}/history`);
    if (!list.length) { el.innerHTML = "<em>no git history</em>"; return; }
    el.innerHTML = list.map((h) => {
      const d = (h.date || "").slice(0, 10);
      const tip = `${h.sha.slice(0,7)}  ${d}  [${h.path}]  ${h.subject}`;
      const cls = h.path === "calc" ? "dot dot-calc" : "dot dot-sc";
      return `<span class="${cls}" title="${esc(tip)}"></span>`;
    }).join("");
  } catch (e) {
    el.innerHTML = `<em>history err: ${esc(e.message)}</em>`;
  }
}

async function openEditor(newScenario) {
  state.mode = "editor";
  state.editorTab = "form";
  $("e-cross-log").innerHTML = "";
  if (newScenario) {
    state.selected = "__new__";
    fillFormFrom(newTemplate());
    $("e-text").value = JSON.stringify(newTemplate(), null, 2);
  } else if (state.selected && state.selected !== "__new__") {
    const d = await api(`/api/scenarios/${state.selected}`);
    fillFormFrom(d.scenario);
    $("e-text").value = JSON.stringify(d.scenario, null, 2);
  }
  $("e-err").textContent = "";
  render();
  setEditorTab("form");
}

function renderEditor() {
  $("detail-empty").hidden = true;
  $("detail").hidden = true;
  $("editor").hidden = false;
  $("e-title").textContent = state.selected === "__new__" ? "New scenario" : `Edit ${state.selected}`;
}

function setEditorTab(tab) {
  state.editorTab = tab;
  $("e-form").hidden = tab !== "form";
  $("e-text").hidden = tab !== "json";
  for (const el of document.querySelectorAll(".tab")) {
    el.classList.toggle("active", el.dataset.tab === tab);
  }
  if (tab === "json") {
    try { $("e-text").value = JSON.stringify(readForm(), null, 2); } catch {}
  } else {
    try { fillFormFrom(JSON.parse($("e-text").value)); } catch {}
  }
}

function fillFormFrom(sc) {
  $("ef-name").value = sc.name || "";
  const fill = (side, spec) => {
    spec = spec || {};
    $(`ef-${side}-species`).value = spec.species || "";
    $(`ef-${side}-item`).value = spec.item || "";
    $(`ef-${side}-ability`).value = spec.ability || "";
    $(`ef-${side}-nature`).value = spec.nature || "Hardy";
    $(`ef-${side}-tera`).value = spec.tera_type || "Normal";
    $(`ef-${side}-terad`).checked = !!spec.terastallized;
    $(`ef-${side}-status`).value = spec.status || "";
    const evs = spec.evs || {};
    for (const k of ["hp","atk","def","spa","spd","spe"]) {
      $(`ef-${side}-ev-${k}`).value = evs[k] || 0;
    }
  };
  fill("a", sc.attacker);
  fill("d", sc.defender);
  $("ef-move").value = sc.move || "";
  $("ef-weather").value = (sc.field && sc.field.weather) || "";
  $("ef-terrain").value = (sc.field && sc.field.terrain) || "";
  $("ef-trials").value = sc.trials || 500;
}

function readForm() {
  const readSide = (side) => {
    const spec = {
      species: $(`ef-${side}-species`).value.trim(),
      level: 50,
      nature: $(`ef-${side}-nature`).value,
      tera_type: $(`ef-${side}-tera`).value,
      terastallized: $(`ef-${side}-terad`).checked,
      evs: {},
    };
    const ability = $(`ef-${side}-ability`).value.trim();
    if (ability) spec.ability = ability;
    const item = $(`ef-${side}-item`).value.trim();
    if (item) spec.item = item;
    const status = $(`ef-${side}-status`).value;
    if (status) spec.status = status;
    for (const k of ["hp","atk","def","spa","spd","spe"]) {
      const v = parseInt($(`ef-${side}-ev-${k}`).value, 10) || 0;
      if (v > 0) spec.evs[k] = v;
    }
    return spec;
  };
  const sc = {
    name: $("ef-name").value.trim() || "new_scenario",
    attacker: readSide("a"),
    defender: readSide("d"),
    move: $("ef-move").value.trim(),
    trials: parseInt($("ef-trials").value, 10) || 500,
  };
  const w = $("ef-weather").value, t = $("ef-terrain").value;
  if (w || t) {
    sc.field = {};
    if (w) sc.field.weather = w;
    if (t) sc.field.terrain = t;
  }
  return sc;
}

async function saveEditor() {
  let body;
  if (state.editorTab === "json") {
    try { body = JSON.parse($("e-text").value); }
    catch (e) { $("e-err").textContent = `JSON parse: ${e.message}`; return; }
  } else {
    body = readForm();
  }
  try {
    if (state.selected === "__new__") {
      const r = await api("/api/scenarios", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      state.selected = r.stem;
    } else {
      await api(`/api/scenarios/${state.selected}`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
    }
  } catch (e) {
    $("e-err").textContent = e.message;
    return;
  }
  state.mode = "detail";
  await refreshList();
  await runOne(state.selected);
  render();
}

async function crossGenerate() {
  const base = readForm();
  const log = $("e-cross-log");
  log.innerHTML = "<h4>Cross-generate progress</h4>";
  const status = document.createElement("div");
  status.className = "xgen-status";
  log.appendChild(status);
  const cat = $("ef-cat").value;
  const heldItems = cat === "physical"
    ? [null, "Life Orb", "Choice Band", "Muscle Band", "Expert Belt"]
    : [null, "Life Orb", "Choice Specs", "Wise Glasses", "Expert Belt"];
  const weathers = ["", "Sun", "Rain"];
  const terrains = ["", cat === "physical" ? "Grassy" : "Electric"];
  let done = 0, ok = 0, skip = 0, err = 0;
  const total = heldItems.length * weathers.length * terrains.length;
  for (const item of heldItems) {
    for (const w of weathers) {
      for (const t of terrains) {
        const sc = JSON.parse(JSON.stringify(base));
        if (item) sc.attacker.item = item;
        else delete sc.attacker.item;
        sc.field = {};
        if (w) sc.field.weather = w;
        if (t) sc.field.terrain = t;
        const tag = `${item || "none"}-${w || "clear"}-${t || "clear"}`;
        sc.name = `${base.name || "xgen"}-${slugish(tag)}`;
        try {
          const r = await fetch("/api/scenarios", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify(sc),
          });
          if (r.status === 201) ok++;
          else if (r.status === 409) skip++;
          else err++;
          const line = document.createElement("div");
          line.textContent = `[${r.status}] ${sc.name}`;
          log.appendChild(line);
        } catch (e) {
          err++;
          const line = document.createElement("div");
          line.textContent = `[ERR] ${sc.name}: ${e.message}`;
          log.appendChild(line);
        }
        done++;
        status.textContent = `${done}/${total} · ${ok} created · ${skip} skipped · ${err} err`;
      }
    }
  }
  await refreshList();
  render();
}

function slugish(s) {
  return String(s).toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/(^-|-$)/g, "");
}

function newTemplate() {
  return {
    name: "new_scenario",
    attacker: {
      species: "Chi-Yu",
      level: 50,
      ability: "Beads of Ruin",
      nature: "Timid",
      evs: { hp: 4, spa: 252, spe: 252 },
    },
    defender: {
      species: "Amoonguss",
      level: 50,
      ability: "Regenerator",
      nature: "Bold",
      evs: { hp: 252, def: 252, spd: 4 },
    },
    move: "Overheat",
    trials: 500,
  };
}

async function runOne(stem) {
  const r = await api(`/api/run/${stem}`, { method: "POST" });
  state.results.set(stem, r);
}

async function runAll() {
  $("runall").hidden = false;
  const log = $("runall-log");
  log.innerHTML = "";
  const total = state.scenarios.length;
  let done = 0, pass = 0, fail = 0, known = 0;
  const r = await fetch("/api/run-all", { method: "POST" });
  const reader = r.body.getReader();
  const dec = new TextDecoder();
  let buf = "";
  while (true) {
    const { value, done: streamDone } = await reader.read();
    if (streamDone) break;
    buf += dec.decode(value, { stream: true });
    let idx;
    while ((idx = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, idx).trim();
      buf = buf.slice(idx + 1);
      if (!line) continue;
      let obj;
      try { obj = JSON.parse(line); } catch { continue; }
      state.results.set(obj.stem, obj);
      done++;
      let cls, tag;
      if (obj.pass) { cls = "pass"; tag = "OK"; pass++; }
      else if (obj.known_failure) { cls = "known"; tag = "known"; known++; }
      else { cls = "fail"; tag = "FAIL"; fail++; }
      const li = document.createElement("li");
      li.className = cls;
      li.textContent = `[${tag}] ${obj.stem}`;
      log.appendChild(li);
      $("runall-status").textContent = `${done} / ${total} · ${pass} pass · ${fail} fail · ${known} known-fail`;
    }
  }
  renderCounters();
  renderList();
}

async function regenMatrix() {
  $("regen").hidden = false;
  const pre = $("regen-log");
  pre.textContent = "starting…\n";
  const r = await fetch("/api/regen", { method: "POST" });
  const reader = r.body.getReader();
  const dec = new TextDecoder();
  let buf = "";
  while (true) {
    const { value, done } = await reader.read();
    if (done) break;
    buf += dec.decode(value, { stream: true });
    let idx;
    while ((idx = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, idx).trim();
      buf = buf.slice(idx + 1);
      if (!line) continue;
      let obj;
      try { obj = JSON.parse(line); } catch { pre.textContent += line + "\n"; continue; }
      if (obj.line !== undefined) pre.textContent += obj.line + "\n";
      else if (obj.done) pre.textContent += `\n[done] ${obj.status || obj.err || ""}\n`;
      pre.scrollTop = pre.scrollHeight;
    }
  }
  await refreshList();
  render();
}

function wireEvents() {
  for (const id of ["f-text", "f-attacker", "f-item", "f-weather", "f-terrain", "f-hand", "f-gen"]) {
    const el = $(id);
    if (el) el.addEventListener("input", render);
  }
  for (const el of document.querySelectorAll('input[name="status"]')) {
    el.addEventListener("change", render);
  }
  $("btn-run").addEventListener("click", async () => {
    if (!state.selected || state.selected === "__new__") return;
    await runOne(state.selected);
    render();
  });
  $("btn-edit").addEventListener("click", () => openEditor(false));
  $("btn-new").addEventListener("click", () => openEditor(true));
  $("btn-save").addEventListener("click", saveEditor);
  $("btn-cross").addEventListener("click", crossGenerate);
  $("btn-cancel").addEventListener("click", () => { state.mode = "detail"; render(); });
  $("btn-runall").addEventListener("click", runAll);
  $("btn-regen").addEventListener("click", regenMatrix);
  for (const el of document.querySelectorAll(".tab")) {
    el.addEventListener("click", () => setEditorTab(el.dataset.tab));
  }
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}
