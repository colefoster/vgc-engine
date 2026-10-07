(() => {
  'use strict';

  const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  /* ---------- mobile nav ---------- */
  const toggle = document.querySelector('.nav-toggle');
  const nav = document.getElementById('site-nav');
  if (toggle && nav) {
    const setOpen = (open, returnFocus) => {
      toggle.setAttribute('aria-expanded', String(open));
      nav.classList.toggle('open', open);
      if (!open && returnFocus) toggle.focus();
    };
    toggle.addEventListener('click', () => setOpen(toggle.getAttribute('aria-expanded') !== 'true'));
    nav.addEventListener('click', (e) => { if (e.target.closest('a')) setOpen(false); });
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && toggle.getAttribute('aria-expanded') === 'true') setOpen(false, true);
    });
    document.addEventListener('click', (e) => {
      if (toggle.getAttribute('aria-expanded') === 'true' && !e.target.closest('.site-header')) setOpen(false);
    });
    window.matchMedia('(min-width: 861px)').addEventListener('change', (m) => { if (m.matches) setOpen(false); });
  }

  /* ---------- generic ARIA tabs (roving tabindex, arrow keys) ---------- */
  function wireTabs(tabs, onSelect) {
    const select = (i, focus) => {
      tabs.forEach((t, j) => {
        const on = i === j;
        t.setAttribute('aria-selected', String(on));
        t.tabIndex = on ? 0 : -1;
      });
      if (focus) tabs[i].focus();
      onSelect(i);
    };
    tabs.forEach((t, i) => {
      t.addEventListener('click', () => select(i, false));
      t.addEventListener('keydown', (e) => {
        let n = null;
        if (e.key === 'ArrowRight' || e.key === 'ArrowDown') n = (i + 1) % tabs.length;
        else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') n = (i - 1 + tabs.length) % tabs.length;
        else if (e.key === 'Home') n = 0;
        else if (e.key === 'End') n = tabs.length - 1;
        if (n !== null) { e.preventDefault(); select(n, true); }
      });
    });
    return select;
  }

  /* ---------- code tabs ---------- */
  const codeTabs = Array.from(document.querySelectorAll('#code-tabs [role="tab"]'));
  if (codeTabs.length) {
    wireTabs(codeTabs, (i) => {
      codeTabs.forEach((t, j) => { document.getElementById(t.getAttribute('aria-controls')).hidden = i !== j; });
    });
  }

  /* ---------- copy buttons ---------- */
  const copyLive = document.getElementById('copy-live');
  document.querySelectorAll('.copy').forEach((btn) => {
    btn.addEventListener('click', async () => {
      const text = document.getElementById(btn.dataset.copy).textContent;
      let ok = false;
      try { await navigator.clipboard.writeText(text); ok = true; } catch (_) {
        // Clipboard API is unavailable on some non-secure origins; fall back.
        const ta = document.createElement('textarea');
        ta.value = text; ta.setAttribute('readonly', ''); ta.style.position = 'fixed'; ta.style.opacity = '0';
        document.body.appendChild(ta); ta.select();
        try { ok = document.execCommand('copy'); } catch (_) { ok = false; }
        ta.remove();
      }
      btn.textContent = ok ? 'Copied' : 'Select & copy';
      if (copyLive) copyLive.textContent = ok ? 'Code copied to clipboard.' : 'Copy failed. Select the code manually.';
      setTimeout(() => { btn.textContent = 'Copy'; }, 1800);
    });
  });

  /* ---------- frontier toy model ----------
     Synthetic numbers only. Mirrors the shape of the documented
     record → cross product → keyed replay → dedup algorithm. */
  const demo = document.getElementById('demo');
  if (!demo) return;

  const ROLLS = 16;
  const HIT = 0.9;
  const CRIT = 1 / 24;
  const SCENARIOS = {
    single: { label: 'Strike B1', base: 44, targets: ['B1'] },
    spread: { label: 'Spread move', base: 33, targets: ['B1', 'B2'] },
    protect: { label: 'Spread into Protect', base: 33, targets: ['B2'], blocked: ['B1'] },
  };
  const START = { healthy: { B1: 100, B2: 100 }, low: { B1: 38, B2: 100 } };

  // Every (acc, crit, roll) cell for one target, with its prior and damage.
  function siteCells(base) {
    const out = [];
    for (const hit of [true, false]) {
      for (const crit of [false, true]) {
        for (let r = 0; r < ROLLS; r++) {
          const p = (hit ? HIT : 1 - HIT) * (crit ? CRIT : 1 - CRIT) / ROLLS;
          const dmg = hit ? Math.floor(Math.floor(base * (crit ? 1.5 : 1)) * (85 + r) / 100) : 0;
          out.push({ p, dmg });
        }
      }
    }
    return out;
  }

  function enumerate(actionKey, hpKey) {
    const sc = SCENARIOS[actionKey];
    const hp0 = START[hpKey];
    const per = siteCells(sc.base);
    let grid = [{ p: 1, hp: { ...hp0 } }];
    for (const t of sc.targets) {
      const next = [];
      for (const g of grid) {
        for (const c of per) {
          const hp = { ...g.hp };
          hp[t] = Math.max(0, hp[t] - c.dmg);
          next.push({ p: g.p * c.p, hp });
        }
      }
      grid = next;
    }
    const states = new Map();
    for (const g of grid) {
      const k = g.hp.B1 + '|' + g.hp.B2;
      const s = states.get(k) || { p: 0, hp: g.hp, cells: 0 };
      s.p += g.p; s.cells += 1;
      states.set(k, s);
    }
    const sorted = Array.from(states.values()).sort((a, b) => b.p - a.p);
    const koB1 = sorted.filter((s) => s.hp.B1 === 0).reduce((a, s) => a + s.p, 0);
    const sum = sorted.reduce((a, s) => a + s.p, 0);
    return { sc, hp0, cells: grid.length, states: sorted, koB1, sum };
  }

  const el = (id) => document.getElementById(id);
  const titleEl = el('pass-title'), textEl = el('pass-text'), visEl = el('pass-visual'), statsEl = el('pass-stats');
  const panel = el('pass-panel'), live = el('demo-live');
  const prev = el('pass-prev'), next = el('pass-next');
  const passTabs = Array.from(demo.querySelectorAll('.passes [role="tab"]'));
  const fmt = (n) => n.toLocaleString('en-US');
  const pct = (p) => (p * 100).toFixed(p < 0.01 ? 2 : 1) + '%';
  let pass = 0;
  let model = null;

  const stats = (pairs) => pairs.map(([k, v]) => `<div><dt>${k}</dt><dd>${v}</dd></div>`).join('');
  const hpBar = (hp, max) => {
    const ko = hp === 0;
    return `<div class="hpbar${ko ? ' ko' : ''}"><span style="width:${(hp / max) * 100}%"></span><em>${ko ? 'KO' : hp + ' HP'}</em></div>`;
  };

  const PASSES = [
    () => {
      const { sc } = model;
      const chips = [];
      sc.targets.forEach((t, i) => {
        chips.push(`<span class="site-chip">${t} accuracy → <b>hit</b></span>`);
        chips.push(`<span class="site-chip">${t} crit → <b>no</b></span>`);
        chips.push(`<span class="site-chip">${t} damage roll → <b>${[11, 4][i]}/16</b></span>`);
      });
      (sc.blocked || []).forEach((t) => chips.push(`<span class="site-chip off">${t}: blocked by Protect, no draws</span>`));
      return {
        title: 'Record pass',
        text: `Clone the battle, swap in a recording RNG, call step() once. It walks one path and logs every draw site it touched, with that site's full outcome space. "${sc.label}" visits ${sc.targets.length * 3} sites.`,
        visual: `<div class="sites">${chips.join('')}</div>`,
        stats: [['steps', '1'], ['draw sites', String(sc.targets.length * 3)]],
        announce: `${sc.targets.length * 3} draw sites recorded.`,
      };
    },
    () => {
      const { sc, cells } = model;
      const shown = Math.min(cells, 4096);
      const dots = '<i class="on"></i>'.repeat(shown);
      return {
        title: 'Cross product',
        text: `Expand each recorded site into every outcome: 2 accuracy × 2 crit × 16 rolls = 64 per hit target. Multiply across targets to get the enumeration grid. Each cell's prior is the product of its site weights.`,
        visual: `<div class="cells">${dots}</div><p class="cells-note">${fmt(cells)} cells · one square each</p>`,
        stats: [['per target', '64'], ['targets hit', String(sc.targets.length)], ['grid cells', fmt(cells)]],
        announce: `${fmt(cells)} grid cells.`,
      };
    },
    () => {
      const { cells } = model;
      return {
        title: 'Keyed replay',
        text: 'For every cell, build a keyed RNG that answers each recorded site with that cell\'s outcome, attach it to a fresh clone of the original battle and step(). This is where a cheap clone and a heap-free step pay off.',
        visual: '<div class="replay-bar"><span id="replay-fill"></span></div>',
        stats: [['clones', fmt(cells)], ['steps', fmt(cells)]],
        announce: `${fmt(cells)} replays.`,
        after: () => {
          const f = el('replay-fill');
          if (!f) return;
          if (reduceMotion) { f.style.width = '100%'; return; }
          requestAnimationFrame(() => requestAnimationFrame(() => { f.style.width = '100%'; }));
        },
      };
    },
    () => {
      const { states, cells, sum, koB1, hp0, sc } = model;
      const top = states.slice(0, 6);
      const rest = states.slice(6);
      const restP = rest.reduce((a, s) => a + s.p, 0);
      const rows = top.map((s) => {
        const w = Math.max(4, Math.round(s.p / states[0].p * 100));
        return `<li class="outcome">${hpBar(s.hp.B1, hp0.B1)}${hpBar(s.hp.B2, hp0.B2)}<span class="prob">${pct(s.p)}<span class="prob-bar" style="width:${w}%"></span></span></li>`;
      }).join('');
      const head = '<li class="outcome outcome-head" aria-hidden="true"><span>B1 after</span><span>B2 after</span><span class="prob">prior</span></li>';
      const more = rest.length ? `<p class="cells-note">+ ${fmt(rest.length)} more states holding ${pct(restP)}</p>` : '';
      const koNote = hp0.B1 < 100 && sc.targets.includes('B1') ? ` Every roll that KOs B1 lands on the same HP, so those cells collapse: B1 is KO'd with ${pct(koB1)} total prior.` : '';
      return {
        title: 'Dedup & sum',
        text: `Group results by canonical state hash and add their priors. Sites that never fired on a path, like a damage roll after a miss, marginalize out on their own.${koNote}`,
        visual: `<ul class="outcomes">${head}${rows}</ul>${more}`,
        stats: [['grid cells', fmt(cells)], ['distinct states', fmt(states.length)], ['Σ prior', sum.toFixed(3)]],
        announce: `${fmt(cells)} cells merged into ${fmt(states.length)} distinct states. Probabilities sum to ${sum.toFixed(3)}.`,
      };
    },
  ];

  function render(announce) {
    const r = PASSES[pass]();
    titleEl.textContent = (pass + 1) + ' · ' + r.title;
    textEl.textContent = r.text;
    visEl.innerHTML = r.visual;
    statsEl.innerHTML = stats(r.stats);
    panel.setAttribute('aria-labelledby', passTabs[pass].id);
    prev.disabled = pass === 0;
    next.disabled = pass === PASSES.length - 1;
    if (announce) live.textContent = `Pass ${pass + 1}, ${r.title}. ${r.announce}`;
    if (r.after) r.after();
  }

  const selectPass = wireTabs(passTabs, (i) => { pass = i; render(true); });
  prev.addEventListener('click', () => { if (pass > 0) selectPass(pass - 1, false); });
  next.addEventListener('click', () => { if (pass < PASSES.length - 1) selectPass(pass + 1, false); });

  function recompute(announce) {
    const action = demo.querySelector('input[name="action"]:checked').value;
    const hp = demo.querySelector('input[name="hp"]:checked').value;
    model = enumerate(action, hp);
    render(announce);
  }
  demo.querySelectorAll('input[type="radio"]').forEach((r) => r.addEventListener('change', () => recompute(true)));
  recompute(false);

  // Exposed for the page's own self-check (see README); not part of any API.
  window.__frontierToy = { enumerate };
})();
