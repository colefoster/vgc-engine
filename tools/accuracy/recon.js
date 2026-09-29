#!/usr/bin/env node
// Real-log -> full-information battle job.
//
// A spectated ladder log hides EV/SP spreads, natures and any move, item or
// ability that never showed up. This builds a *complete*, plausible team for
// each side from what the log reveals, filling the gaps from usage counts over
// the whole replay sample, and extracts the per-turn choices the players made
// ("intents") so ps-battle.js can steer PS along the real game.
//
// PS and the engine then both receive the SAME reconstructed teams, so the
// hidden-information error cancels out of the engine-vs-PS comparison. How far
// PS itself strays from the real log (because the fills are guesses) is
// measured separately (see docs/accuracy/2026-09-accuracy-proof.md).
//
// Usage:
//   node recon.js <replay_dir> <out_jobs.jsonl> [--limit N] [--min-turns T]
//
// Each output line: {id, seed, format, p1team, p2team, intents, meta}.

'use strict';

const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

const PS_PATH = process.env.PS_DIST || '/tmp/pokemon-showdown-research/dist/sim';
const { Dex } = require(PS_PATH);
const FORMAT = 'gen9championsvgc2026regmc';
const dex = Dex.forFormat(FORMAT);

const toID = (s) => ('' + s).toLowerCase().replace(/[^a-z0-9]+/g, '');

function listReplays(dir) {
  const out = [];
  const walk = (d) => {
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p);
      else if (e.name.endsWith('.json')) out.push(p);
    }
  };
  walk(dir);
  return out.sort();
}

// "p1a: Nick" -> {side:'p1', slot:0, nick:'Nick'}
function parseIdent(s) {
  const m = /^(p[12])([ab])?: ?(.*)$/.exec(s || '');
  if (!m) return null;
  return { side: m[1], slot: m[2] ? (m[2] === 'a' ? 0 : 1) : null, nick: m[3] };
}

// "Arcanine-Hisui, L50, M, shiny" -> {species, gender}
function parseDetails(s) {
  const parts = (s || '').split(', ');
  let gender = '';
  for (const p of parts.slice(1)) if (p === 'M' || p === 'F') gender = p;
  return { species: parts[0], gender };
}

const baseSpecies = (sp) => {
  const s = dex.species.get(sp);
  if (s.exists && s.isMega) return s.baseSpecies;
  return s.exists ? s.name : sp;
};

// Walk one log. Returns per-side mon records keyed by base species, the brought
// order, per-turn intents, and flags for things the recon cannot represent.
function parseLog(log) {
  const lines = log.split('\n');
  const sides = { p1: { preview: [], mons: {}, nick: {}, order: [] }, p2: { preview: [], mons: {}, nick: {}, order: [] } };
  const active = { p1: [null, null], p2: [null, null] }; // base species at slot
  const intents = {}; // turn -> side -> {main:[slot0,slot1], mid:[], rep:[]}
  const flags = new Set();
  let turn = 0;
  let upkeep = false;
  let movedThisTurn = { p1: [false, false], p2: [false, false] };
  let megaThisTurn = { p1: [false, false], p2: [false, false] };
  let megaDone = { p1: {}, p2: {} };
  let turns = 0;

  const mon = (side, species) => {
    const S = sides[side].mons;
    if (!S[species]) S[species] = { species, gender: '', moves: [], item: null, ability: null, megaStone: null };
    return S[species];
  };
  const slotIntent = (side) => {
    intents[turn] = intents[turn] || {};
    const t = intents[turn];
    t[side] = t[side] || { main: [null, null], mid: [], rep: [] };
    return t[side];
  };
  const speciesOf = (id) => {
    if (!id) return null;
    const sp = sides[id.side].nick[id.nick];
    return sp || null;
  };
  const addAbility = (id, abilityName) => {
    const sp = speciesOf(id);
    if (!sp) return;
    if (megaDone[id.side][sp]) return; // mega forme ability, not the base one
    const m = mon(id.side, sp);
    if (!m.ability) m.ability = toID(abilityName);
  };
  const addItem = (id, itemName) => {
    const sp = speciesOf(id);
    if (!sp) return;
    const m = mon(id.side, sp);
    if (!m.item) m.item = toID(itemName);
  };

  for (const line of lines) {
    if (!line.startsWith('|')) continue;
    const parts = line.split('|').slice(1);
    const cmd = parts[0];
    const tag = (name) => {
      for (const p of parts) if (p.startsWith(`[${name}]`)) return p.slice(name.length + 2).trim();
      return null;
    };
    switch (cmd) {
      case 'poke': {
        const d = parseDetails(parts[2]);
        sides[parts[1]].preview.push(d.species);
        mon(parts[1], d.species).gender = d.gender;
        break;
      }
      case 'turn':
        turn = parseInt(parts[1], 10);
        turns = turn;
        upkeep = false;
        movedThisTurn = { p1: [false, false], p2: [false, false] };
        megaThisTurn = { p1: [false, false], p2: [false, false] };
        break;
      case 'upkeep':
        upkeep = true;
        break;
      case 'switch':
      case 'drag':
      case 'replace': {
        const id = parseIdent(parts[1]);
        const d = parseDetails(parts[2]);
        const sp = baseSpecies(d.species);
        if (cmd === 'replace') flags.add('illusion');
        sides[id.side].nick[id.nick] = sp;
        const m = mon(id.side, sp);
        if (d.gender) m.gender = d.gender;
        if (!sides[id.side].order.includes(sp)) sides[id.side].order.push(sp);
        active[id.side][id.slot] = sp;
        if (turn === 0 || cmd !== 'switch') break;
        const it = slotIntent(id.side);
        if (upkeep) it.rep.push({ slot: id.slot, species: sp });
        else if (movedThisTurn[id.side][id.slot]) it.mid.push({ slot: id.slot, species: sp });
        else if (!it.main[id.slot]) it.main[id.slot] = { kind: 'switch', species: sp };
        else it.mid.push({ slot: id.slot, species: sp });
        break;
      }
      case 'detailschange': {
        const id = parseIdent(parts[1]);
        const d = parseDetails(parts[2]);
        const s = dex.species.get(d.species);
        if (s.exists && s.isMega) {
          const sp = speciesOf(id);
          if (sp) megaDone[id.side][sp] = true;
        }
        break;
      }
      case '-mega': {
        const id = parseIdent(parts[1]);
        const sp = speciesOf(id);
        if (sp) {
          const m = mon(id.side, sp);
          m.megaStone = toID(parts[3] || '');
          m.item = m.megaStone || m.item;
          megaDone[id.side][sp] = true;
        }
        if (turn > 0 && id.slot !== null) megaThisTurn[id.side][id.slot] = true;
        break;
      }
      case 'move': {
        const id = parseIdent(parts[1]);
        const sp = speciesOf(id);
        const moveName = parts[2];
        const from = tag('from');
        const mv = dex.moves.get(moveName);
        if (!sp || !mv.exists) break;
        if (from && !/lockedmove/i.test(from)) break; // called / copied move
        const m = mon(id.side, sp);
        if (mv.id !== 'struggle' && !m.moves.includes(mv.id)) m.moves.push(mv.id);
        if (from || turn === 0 || id.slot === null) break;
        movedThisTurn[id.side][id.slot] = true;
        const it = slotIntent(id.side);
        if (!it.main[id.slot]) {
          const tgt = parseIdent(parts[3]);
          it.main[id.slot] = {
            kind: 'move', move: mv.id,
            target: tgt && tgt.slot !== null ? { side: tgt.side, slot: tgt.slot } : null,
            mega: megaThisTurn[id.side][id.slot],
          };
        }
        break;
      }
      case 'cant': {
        const id = parseIdent(parts[1]);
        if (turn > 0 && id && id.slot !== null) {
          movedThisTurn[id.side][id.slot] = true;
          const it = slotIntent(id.side);
          if (!it.main[id.slot]) {
            const mv = parts[3] ? dex.moves.get(parts[3]) : null;
            it.main[id.slot] = mv && mv.exists ? { kind: 'move', move: mv.id, target: null, mega: false } : { kind: 'any' };
          }
        }
        break;
      }
      default:
        break;
    }
    // item / ability reveals carried on any line
    const fromTag = tag('from');
    const ofTag = tag('of');
    if (fromTag) {
      const holder = parseIdent(ofTag) || parseIdent(parts[1]);
      const mi = /^item: ?(.*)$/.exec(fromTag);
      const ma = /^ability: ?(.*)$/.exec(fromTag);
      if (mi && holder) {
        // `[from] item: X|[of] Y` on a -damage means Y's item hurt the subject
        // (Rocky Helmet); otherwise the subject is the holder.
        const h = ofTag && /Rocky Helmet|Sticky Barb/.test(mi[1]) ? parseIdent(ofTag) : parseIdent(parts[1]);
        if (h && !/Frisk/.test(fromTag)) addItem(h, mi[1]);
      }
      if (ma && holder) addAbility(holder, ma[1]);
    }
    if (cmd === '-enditem' || cmd === '-item') {
      const id = parseIdent(parts[1]);
      if (id && !(tag('from') || '').startsWith('ability: Frisk')) {
        // -enditem for a consumed/knocked item names the holder's item.
        if (cmd === '-enditem') addItem(id, parts[2]);
        else if (!tag('from')) addItem(id, parts[2]);
      }
    }
    if (cmd === '-ability') {
      const id = parseIdent(parts[1]);
      if (id && !tag('from')) addAbility(id, parts[2]);
    }
    if (cmd === '-activate' && parts[2] && /^item: /.test(parts[2])) {
      const id = parseIdent(parts[1]);
      if (id) addItem(id, parts[2].slice(6));
    }
    if (cmd === '-activate' && parts[2] && /^ability: /.test(parts[2])) {
      const id = parseIdent(parts[1]);
      if (id) addAbility(id, parts[2].slice(9));
    }
  }
  return { sides, intents, flags, turns };
}

// --- usage fill ------------------------------------------------------------

function buildUsage(parsedList) {
  const U = {};
  const bump = (o, k) => { if (k) o[k] = (o[k] || 0) + 1; };
  for (const p of parsedList) {
    for (const side of ['p1', 'p2']) {
      for (const m of Object.values(p.sides[side].mons)) {
        const u = U[m.species] = U[m.species] || { moves: {}, items: {}, abilities: {} };
        for (const mv of m.moves) bump(u.moves, mv);
        bump(u.items, m.item);
        bump(u.abilities, m.ability);
      }
    }
  }
  return U;
}

const ranked = (o) => Object.entries(o || {}).sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1)).map((e) => e[0]);

const PHYS = 'Physical', SPEC = 'Special';

// Deterministic per-(replay, species) variety for the hidden spread, so two
// sides running the same species don't get identical stats (identical
// Speeds would make every such mirror a speed tie, which real ladder games
// don't show nearly as often).
function variety(key) {
  const h = crypto.createHash('sha1').update(key).digest();
  return h.readUInt32BE(0) / 2 ** 32;
}

function fillMon(m, U, usedItems, key) {
  const u = U[m.species] || { moves: {}, items: {}, abilities: {} };
  const s = dex.species.get(m.species);
  const moves = m.moves.slice(0, 4);
  for (const mv of ranked(u.moves)) {
    if (moves.length >= 4) break;
    if (!moves.includes(mv)) moves.push(mv);
  }
  if (moves.length === 0) moves.push('protect');
  let item = m.item;
  if (item && usedItems.has(item)) item = null;
  if (!item) {
    for (const it of ranked(u.items)) {
      const I = dex.items.get(it);
      if (!usedItems.has(it) && I.exists && !I.megaStone) { item = it; break; }
    }
  }
  if (item) usedItems.add(item);
  let ability = m.ability;
  const legal = Object.values(s.abilities || {}).map(toID);
  if (!ability || !legal.includes(ability)) {
    ability = ranked(u.abilities).find((a) => legal.includes(a)) || legal[0];
  }
  // Spread heuristic (Champions Stat Points: 66 total, 32 max per stat).
  let phys = 0, spec = 0;
  for (const mv of moves) {
    const M = dex.moves.get(mv);
    if (M.category === PHYS) phys++;
    else if (M.category === SPEC) spec++;
  }
  const trickRoom = moves.includes('trickroom');
  const bs = s.baseStats || {};
  let evs, nature;
  if (phys === 0 && spec === 0) {
    evs = { hp: 32, def: 2, spd: 32 }; nature = 'Calm';
  } else if (phys >= spec) {
    evs = { hp: 2, atk: 32, spe: 32 }; nature = trickRoom ? 'Brave' : (bs.spe >= 80 ? 'Jolly' : 'Adamant');
    if (trickRoom) evs = { hp: 32, atk: 32, def: 2 };
  } else {
    evs = { hp: 2, spa: 32, spe: 32 }; nature = trickRoom ? 'Quiet' : (bs.spe >= 80 ? 'Timid' : 'Modest');
    if (trickRoom) evs = { hp: 32, spa: 32, spd: 2 };
  }
  // Champions has no IVs (the mod's stat formula fixes them at 31), so a
  // Trick Room set just skips Speed SP and takes a -Spe nature.
  const ivs = null;
  if (!trickRoom && evs.spe !== undefined) {
    // Spend 0-32 Speed SP, the rest in bulk (66 SP total, 32 max per stat).
    const r = variety(key + ':spe');
    const spe = r < 0.5 ? 32 : r < 0.7 ? 24 : r < 0.85 ? 12 : 0;
    const bulk = 66 - (evs.atk || evs.spa || 32) - spe;
    evs.spe = spe;
    evs.hp = Math.min(32, bulk);
    const rest = bulk - evs.hp;
    if (rest > 0) evs[variety(key + ':def') < 0.5 ? 'def' : 'spd'] = rest;
    for (const k of Object.keys(evs)) if (!evs[k]) delete evs[k];
    if (spe < 32 && (nature === 'Jolly' || nature === 'Timid')) nature = nature === 'Jolly' ? 'Adamant' : 'Modest';
  }
  return { species: m.species, gender: m.gender, item, ability, moves, evs, nature, ivs };
}

const STAT_NAMES = { hp: 'HP', atk: 'Atk', def: 'Def', spa: 'SpA', spd: 'SpD', spe: 'Spe' };

function exportSet(set) {
  const sp = dex.species.get(set.species);
  const it = set.item ? dex.items.get(set.item).name : '';
  const lines = [];
  const g = set.gender && !sp.gender ? ` (${set.gender})` : '';
  lines.push(`${sp.name}${g}${it ? ` @ ${it}` : ''}`);
  lines.push(`Ability: ${dex.abilities.get(set.ability).name}`);
  lines.push('Level: 50');
  lines.push('EVs: ' + Object.entries(set.evs).map(([k, v]) => `${v} ${STAT_NAMES[k]}`).join(' / '));
  lines.push(`${set.nature} Nature`);
  if (set.ivs) lines.push('IVs: ' + Object.entries(set.ivs).map(([k, v]) => `${v} ${STAT_NAMES[k]}`).join(' / '));
  for (const mv of set.moves) lines.push(`- ${dex.moves.get(mv).name}`);
  return lines.join('\n');
}

function buildJob(file, parsed, U) {
  const job = { id: null, format: FORMAT, intents: parsed.intents, meta: { replay: path.basename(file), turns: parsed.turns, flags: [...parsed.flags] } };
  for (const side of ['p1', 'p2']) {
    const S = parsed.sides[side];
    // brought 4 in on-field order (leads first), padded from the preview
    const brought = S.order.slice(0, 4);
    for (const sp of S.preview) {
      if (brought.length >= 4) break;
      const b = baseSpecies(sp);
      if (!brought.includes(b)) brought.push(b);
    }
    const used = new Set();
    // mega stones first so the item clause keeps them
    const recs = brought.map((sp) => S.mons[sp] || { species: sp, gender: '', moves: [], item: null, ability: null, megaStone: null });
    for (const r of recs) if (r.megaStone) used.add(r.megaStone);
    const sets = recs.map((r) => {
      if (r.megaStone) { used.delete(r.megaStone); }
      return fillMon(r, U, used, `${path.basename(file)}:${side}:${r.species}`);
    });
    job[`${side}team`] = sets.map(exportSet).join('\n\n');
    job.meta[`${side}species`] = brought;
    job.meta[`${side}revealed`] = recs.map((r) => ({ moves: r.moves.length, item: !!r.item, ability: !!r.ability }));
  }
  job.id = crypto.createHash('sha1').update(job.meta.replay).digest('hex').slice(0, 10);
  // Deterministic sodium seed derived from the replay id.
  job.seed = 'sodium,' + crypto.createHash('sha256').update('seed:' + job.meta.replay).digest('hex').slice(0, 32);
  return job;
}

function main() {
  const args = process.argv.slice(2);
  const dir = args[0], outPath = args[1];
  if (!dir || !outPath) {
    console.error('usage: recon.js <replay_dir> <out.jsonl> [--limit N] [--min-turns T]');
    process.exit(2);
  }
  const opt = (name, dflt) => {
    const i = args.indexOf(name);
    return i >= 0 ? parseInt(args[i + 1], 10) : dflt;
  };
  const limit = opt('--limit', Infinity);
  const minTurns = opt('--min-turns', 3);
  const files = listReplays(dir);
  const parsed = [];
  for (const f of files) {
    try {
      const r = JSON.parse(fs.readFileSync(f, 'utf8'));
      parsed.push({ file: f, p: parseLog(r.log) });
    } catch (e) {
      process.stderr.write(`${f}: ${e.message}\n`);
    }
  }
  const U = buildUsage(parsed.map((x) => x.p));
  const out = fs.createWriteStream(outPath);
  let n = 0, skipped = 0;
  // Stable pseudo-random order (the sample itself is already random).
  for (const { file, p } of parsed) {
    if (n >= limit) break;
    if (p.turns < minTurns) { skipped++; continue; }
    const job = buildJob(file, p, U);
    out.write(JSON.stringify(job) + '\n');
    n++;
  }
  out.end();
  process.stderr.write(`recon: ${files.length} replays, ${n} jobs written, ${skipped} skipped (< ${minTurns} turns)\n`);
}

module.exports = { parseLog, buildUsage, buildJob, exportSet, FORMAT };

if (require.main === module) main();
