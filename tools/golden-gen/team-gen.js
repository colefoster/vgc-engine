#!/usr/bin/env node
// Random Showdown-export team generator
//
// Given a seed integer, prints a Showdown-export team string of 4-6 mons
// to stdout. Driven by PS's own gen-9 dex (species / moves / abilities /
// items / natures), filtered to entries that are unlikely to trip up the
// engine or PS team-validation.
//
// Output is plain Showdown export text — drop into `p1.team` / `p2.team`
// of a `tools/ps-golden-driver/` input job and the driver accepts it as-is.
//
// PS data sources (require'd from /tmp/pokemon-showdown-research/dist/sim):
//   * Dex.species.all() / Dex.species.getMovePool()   sim/dex-species.ts
//   * Dex.items.all()                                  sim/dex-items.ts
//   * Dex.natures.all()                                data/natures.ts
//
// Rules:
//   * Species: gen-9 dex, num <= 1025, not nonstandard, no battle-only /
//     alt-forme entries (those need preconditions PS won't accept in
//     `gen9customgame`). Mythicals/legends ARE allowed — gen9customgame
//     has no banlist.
//   * Move set: 4 random distinct moves from the species' move pool.
//     Move pool filtered to non-Z, non-Max, non-Hidden Power for sanity
//     (HP needs a base power calc PS won't auto-fill).
//   * Ability: uniform pick from `species.abilities` ({0, 1?, H?, S?}).
//   * Item: uniform pick from a curated competitive pool (no Mega Stones,
//     no Z-Crystals, no item-user-locked items).
//   * Nature: uniform from all 25.
//   * EVs: 2 random stats get 252, HP gets 4. Sums to 508 (PS legal).
//   * IVs: 31 across the board (random IV variation is low-value).
//   * Tera: uniform from the 18 types (Stellar excluded — it has special
//     handling in gen 9 that the engine may not implement uniformly).
//
// Usage:
//   node team-gen.js <seed> [size]
//     seed: integer (any 32-bit value)
//     size: number of mons (default 4; clamped to [1, 6])

'use strict';

const PS_PATH = process.env.PS_DIST || '/tmp/pokemon-showdown-research/dist/sim';
const Dex = require(PS_PATH + '/dex').Dex;
const g9 = Dex.forGen(9);

// --- seeded PRNG (mulberry32 — simple, deterministic, no deps) -----------

function mulberry32(seed) {
  let a = (seed >>> 0) || 1;
  return function () {
    a = (a + 0x6D2B79F5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function pick(rand, arr) {
  return arr[Math.floor(rand() * arr.length)];
}

function shuffle(rand, arr) {
  const a = arr.slice();
  for (let i = a.length - 1; i > 0; i--) {
    const j = Math.floor(rand() * (i + 1));
    [a[i], a[j]] = [a[j], a[i]];
  }
  return a;
}

// --- filtered data tables ------------------------------------------------

const TYPES = [
  'Normal', 'Fire', 'Water', 'Electric', 'Grass', 'Ice', 'Fighting',
  'Poison', 'Ground', 'Flying', 'Psychic', 'Bug', 'Rock', 'Ghost',
  'Dragon', 'Dark', 'Steel', 'Fairy',
];

function validSpecies() {
  return g9.species.all().filter((s) => {
    if (s.num <= 0 || s.num > 1025) return false;
    if (s.isNonstandard) return false;
    if (s.battleOnly) return false;
    // Skip ALL alt formes — many are battle-only or require items/abilities
    // the team-validator will reject, and the base species gives us plenty
    // of coverage. (Drops Greninja-Bond, Urshifu-Rapid-Strike, Ogerpon-*,
    // etc. — acceptable trade for not having to hand-curate exceptions.)
    if (s.forme && s.forme !== '') return false;
    if (s.requiredItem) return false;
    if (s.requiredAbility) return false;
    if (s.requiredMove) return false;
    // Drop species PS treats as cosmetic-only (cap pikachus, etc.) — they
    // share base stats with the base but the dex still lists them and our
    // engine slug may not know them.
    if (s.cosmeticFormes) return false;
    return true;
  });
}

function validItems() {
  return g9.items.all().filter((i) => {
    if (i.isNonstandard) return false;
    if (i.megaStone) return false;
    if (i.zMove) return false;
    if (i.itemUser) return false;  // Thick Club / Light Ball / etc — species-locked
    if (i.onPrimal) return false;
    return true;
  });
}

function validMovesForSpecies(species) {
  // Dex.species.getMovePool returns a Set of move ids. Filter for
  // non-Z, non-Max, non-special-target moves the driver / engine can
  // accept verbatim.
  const pool = g9.species.getMovePool(species.id);
  const moves = [];
  for (const id of pool) {
    const m = g9.moves.get(id);
    if (!m || !m.exists) continue;
    if (m.isNonstandard) continue;
    if (m.isZ || m.isMax) continue;
    if (m.id === 'hiddenpower') continue;  // needs IV-derived type/power
    if (m.id.startsWith('hiddenpower')) continue;
    // Skip status-broken moves that PS may reject in customgame.
    if (m.realMove) continue;  // sketch-marker etc.
    moves.push(m.name);
  }
  return moves;
}

function pickAbility(rand, species) {
  const slots = Object.values(species.abilities).filter((a) => a);
  return pick(rand, slots);
}

function pickNature(rand) {
  return pick(rand, g9.natures.all().filter((n) => n.name)).name;
}

function pickEVs(rand) {
  const stats = ['HP', 'Atk', 'Def', 'SpA', 'SpD', 'Spe'];
  const two = shuffle(rand, stats).slice(0, 2);
  // 252 + 252 + 4 = 508 → PS-legal (<= 510).
  const ev = { HP: 0, Atk: 0, Def: 0, SpA: 0, SpD: 0, Spe: 0 };
  ev[two[0]] = 252;
  ev[two[1]] = 252;
  // Put leftover 4 into the first stat that's still 0.
  for (const s of stats) {
    if (ev[s] === 0) { ev[s] = 4; break; }
  }
  return ev;
}

function pickTera(rand) {
  return pick(rand, TYPES);
}

// --- export-text formatter -----------------------------------------------

function formatMon(mon) {
  const lines = [];
  lines.push(`${mon.species} @ ${mon.item}`);
  lines.push(`Ability: ${mon.ability}`);
  lines.push(`Level: 50`);
  lines.push(`Tera Type: ${mon.tera}`);
  const evParts = [];
  for (const s of ['HP', 'Atk', 'Def', 'SpA', 'SpD', 'Spe']) {
    if (mon.evs[s] > 0) evParts.push(`${mon.evs[s]} ${s}`);
  }
  if (evParts.length) lines.push(`EVs: ${evParts.join(' / ')}`);
  lines.push(`${mon.nature} Nature`);
  for (const mv of mon.moves) {
    lines.push(`- ${mv}`);
  }
  return lines.join('\n');
}

// --- team builder --------------------------------------------------------

function generateTeam(seed, size) {
  const rand = mulberry32(seed);
  const species = validSpecies();
  const items = validItems();
  if (!species.length) throw new Error('no valid species — PS data missing?');
  if (!items.length) throw new Error('no valid items — PS data missing?');

  const chosenSpecies = new Set();
  const team = [];
  let attempts = 0;
  while (team.length < size && attempts < size * 20) {
    attempts++;
    const sp = pick(rand, species);
    if (chosenSpecies.has(sp.id)) continue;
    const moves = validMovesForSpecies(sp);
    if (moves.length < 4) continue;  // need at least 4 distinct moves
    chosenSpecies.add(sp.id);

    const moveSet = shuffle(rand, moves).slice(0, 4);
    team.push({
      species: sp.name,
      ability: pickAbility(rand, sp),
      item: pick(rand, items).name,
      nature: pickNature(rand),
      evs: pickEVs(rand),
      tera: pickTera(rand),
      moves: moveSet,
    });
  }
  if (team.length < 1) throw new Error('could not assemble even one mon');
  return team;
}

function formatTeam(team) {
  return team.map(formatMon).join('\n\n') + '\n';
}

// --- CLI -----------------------------------------------------------------

if (require.main === module) {
  const seedArg = process.argv[2];
  if (!seedArg) {
    process.stderr.write('usage: team-gen.js <seed> [size]\n');
    process.exit(2);
  }
  const seed = parseInt(seedArg, 10);
  if (!Number.isFinite(seed)) {
    process.stderr.write(`bad seed: ${seedArg}\n`);
    process.exit(2);
  }
  let size = parseInt(process.argv[3] || '4', 10);
  if (!Number.isFinite(size)) size = 4;
  size = Math.max(1, Math.min(6, size));
  try {
    const team = generateTeam(seed, size);
    process.stdout.write(formatTeam(team));
  } catch (e) {
    process.stderr.write(String((e && e.stack) || e) + '\n');
    process.exit(1);
  }
}

module.exports = { generateTeam, formatTeam, mulberry32 };
