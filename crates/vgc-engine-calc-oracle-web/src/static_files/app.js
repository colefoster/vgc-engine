// calc-oracle webui — vanilla JS SPA.

const state = {
  scenarios: [],          // ScenarioSummary[]
  results: new Map(),     // stem -> RunResult
  selected: null,         // stem
  mode: "detail",         // "detail" | "editor"
};

const $ = (id) => document.getElementById(id);

async function api(path, opts) {
  const r = await fetch(path, opts);
  if (!r.ok) throw new Error(`${path}: ${r.status} ${await r.text()}`);
  const ct = r.headers.get("content-type") || "";
  return ct.includes("json") ? r.json() : r.text();
}

// ---------- init ----------
async function boot() {
  wireEvents();
  await refreshList();
  render();
}
document.addEventListener("DOMContentLoaded", boot);

async function refreshList() {
  state.scenarios = await api("/api/scenarios");
  // Populate filter dropdowns.
  const uniq = (key) => [...new Set(state.scenarios.map((s) => s[key]).filter(Boolean))].sort();
  fillSelect("f-attacker", uniq("attacker_species"));
  // "item" is under scenario JSON; summary doesn't carry it — collect on demand.
  // For v1 skip item dropdown values (kept for structural symmetry with the design).
  fillSelect("f-weather", uniq("weather"));
  fillSelect("f-terrain", uniq("terrain"));
}

function fillSelect(id, values) {
  const el = $(id);
  const cur = el.value;
  el.innerHTML = '<option value="">any</option>' + values.map((v) => `<option>${esc(v)}</option>`).join("");
  el.value = cur;
}

// ---------- filter + list render ----------
function filtered() {
  const q = $("f-text").value.trim().toLowerCase();
  const atk = $("f-attacker").value;
  const wea = $("f-weather").value;
  const ter = $("f-terrain").value;
  const status = document.querySelector('input[name="status"]:checked').value;
  const hand = $("f-hand").checked;
  const gen = $("f-gen").checked;
  return state.scenarios.filter((s) => {
    if (q && !(s.stem.toLowerCase().includes(q) || s.name.toLowerCase().includes(q))) return false;
    if (atk && s.attacker_species !== atk) return false;
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
  // Ensure we have a run result to render the SVG. If not, kick a run.
  if (!state.results.has(stem)) {
    await runOne(stem);
  }
  render();
}

// ---------- detail pane ----------
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
    `attacker: ${s.attacker_species}   defender: ${s.defender_species}`,
    `move: ${s.move_name}`,
    `field: weather=${s.weather || "none"}, terrain=${s.terrain || "none"}`,
    `source: ${s.source}${s.known_failure ? "  [KNOWN FAILURE]" : ""}`,
  ];
  if (r && r.desc) parts.push(`desc: ${r.desc}`);
  $("d-summary").textContent = parts.join("\n");
  $("d-svg").innerHTML = r ? renderSvg(r) : "<em>running…</em>";
  $("d-values").textContent = r ? renderValues(r) : "";
}

function renderSvg(r) {
  const max = Math.max(1, r.target_max_hp, ...r.expected_union, ...r.observed);
  const W = 700, H = 220, pad = 30;
  const barW = (W - pad * 2) / 16;
  const scaleY = (v) => H - pad - (v / max) * (H - pad * 2 - 20);
  const outOfSpec = new Set(r.out_of_spec);
  const isKnown = r.known_failure;

  // Engine bars (top row): observed[0..16].
  // Calc-union ghost: draw at ranks 0..N as thick grey bars behind.
  let bars = "";
  for (let i = 0; i < 16; i++) {
    const v = r.observed[i] || 0;
    const oos = outOfSpec.has(v);
    const color = oos ? (isKnown ? "#c88300" : "#c22") : "#4a8";
    const y = scaleY(v);
    bars += `<rect x="${pad + i * barW + 1}" y="${y}" width="${barW - 2}" height="${H - pad - y}" fill="${color}"/>`;
    bars += `<text x="${pad + i * barW + barW / 2}" y="${H - pad + 12}" font-size="9" text-anchor="middle" fill="#888">${v}</text>`;
  }

  // Calc-union ghost: 16 rolls; if calc has fewer entries just use noncrit array.
  const calc16 = r.expected_noncrit && r.expected_noncrit.length === 16 ? r.expected_noncrit : r.expected_union.slice(0, 16);
  let ghosts = "";
  for (let i = 0; i < calc16.length; i++) {
    const v = calc16[i];
    const y = scaleY(v);
    ghosts += `<rect x="${pad + i * barW + 1}" y="${y - 4}" width="${barW - 2}" height="2" fill="#888" opacity="0.7"/>`;
  }

  // Crit ghost: overlay thin bars.
  const critArr = r.expected_crit || [];
  let crits = "";
  for (let i = 0; i < critArr.length; i++) {
    const v = critArr[i];
    const y = scaleY(v);
    crits += `<rect x="${pad + i * barW + barW / 2 - 1}" y="${y}" width="2" height="${H - pad - y}" fill="#aaa" opacity="0.4"/>`;
  }

  // HP threshold line.
  const hpY = scaleY(r.target_max_hp);
  const hpLine = `<line x1="${pad}" y1="${hpY}" x2="${W - pad}" y2="${hpY}" stroke="#a44" stroke-dasharray="4 3" stroke-width="1"/><text x="${W - pad}" y="${hpY - 3}" font-size="9" text-anchor="end" fill="#a44">max HP ${r.target_max_hp}</text>`;

  return `<svg viewBox="0 0 ${W} ${H}" width="${W}" height="${H}">
    ${crits}${ghosts}${bars}${hpLine}
  </svg>`;
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

// ---------- editor ----------
async function openEditor(newScenario) {
  state.mode = "editor";
  if (newScenario) {
    state.selected = "__new__";
    $("e-text").value = JSON.stringify(newTemplate(), null, 2);
  } else if (state.selected && state.selected !== "__new__") {
    const d = await api(`/api/scenarios/${state.selected}`);
    $("e-text").value = JSON.stringify(d.scenario, null, 2);
  }
  $("e-err").textContent = "";
  render();
}

function renderEditor() {
  $("detail-empty").hidden = true;
  $("detail").hidden = true;
  $("editor").hidden = false;
  $("e-title").textContent = state.selected === "__new__" ? "New scenario" : `Edit ${state.selected}`;
}

async function saveEditor() {
  let body;
  try {
    body = JSON.parse($("e-text").value);
  } catch (e) {
    $("e-err").textContent = `JSON parse: ${e.message}`;
    return;
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
    trials: 200,
  };
}

// ---------- run ----------
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

// ---------- events ----------
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
  $("btn-cancel").addEventListener("click", () => { state.mode = "detail"; render(); });
  $("btn-runall").addEventListener("click", runAll);
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}
