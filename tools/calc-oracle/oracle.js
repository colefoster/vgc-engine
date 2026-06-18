#!/usr/bin/env node
// Smogon damage-calc oracle.
//
// Reads a scenario JSON describing one attacker + one defender + one move
// (gen-9 customgame singles) and prints the canonical damage array (16
// integer values, one per roll) plus a description string.
//
// This is the *spec oracle* — independent of PS battle implementation
// and PS RNG draw order. The engine's calculate_damage() is correct if,
// across N seeds, every observed damage value is a member of this set.
//
// Usage:
//   node tools/calc-oracle/oracle.js path/to/scenario.json > calc.json
//
// Input shape (JSON):
//   {
//     "name": "lucario_cc_lifeorb",
//     "attacker": {
//       "species": "Lucario", "level": 50, "item": "Life Orb",
//       "ability": "Steadfast", "nature": "Adamant",
//       "evs": {"hp": 4, "atk": 252, "spe": 252},
//       "ivs": {},
//       "tera_type": null, "terastallized": false,
//       "boosts": {}
//     },
//     "defender": { "species": "Garchomp", ... },
//     "move": "Close Combat",
//     "field": {
//       "weather": null,             // "Sun"|"Rain"|"Sand"|"Snow"|null
//       "terrain": null,             // "Electric"|"Grassy"|"Misty"|"Psychic"|null
//       "isSpread": false
//     }
//   }
//
// Output (stdout):
//   {
//     "name": "...",
//     "damage": [99,99,101,...],     // 16 values, sorted ascending
//     "range": [99, 117],
//     "desc": "<calc's full description>"
//   }
'use strict';

const fs = require('fs');
const { calculate, Generations, Pokemon, Move, Field } = require('@smogon/calc');

function loadScenario(path) {
  return JSON.parse(fs.readFileSync(path, 'utf8'));
}

function buildPokemon(gen, spec) {
  const opts = {
    level: spec.level || 50,
    item: spec.item || undefined,
    ability: spec.ability || undefined,
    nature: spec.nature || 'Hardy',
    evs: spec.evs || {},
    ivs: spec.ivs || {},
    boosts: spec.boosts || {},
  };
  if (spec.terastallized && spec.tera_type) {
    opts.teraType = spec.tera_type;
    opts.isTerastallized = true;
  } else if (spec.tera_type) {
    opts.teraType = spec.tera_type;
  }
  if (spec.status) opts.status = spec.status;
  return new Pokemon(gen, spec.species, opts);
}

function buildField(spec) {
  if (!spec) return new Field();
  const opts = {};
  if (spec.weather) opts.weather = spec.weather;
  if (spec.terrain) opts.terrain = spec.terrain;
  if (spec.isSpread) opts.gameType = 'Doubles';
  return new Field(opts);
}

function main() {
  const path = process.argv[2];
  if (!path) {
    process.stderr.write('usage: oracle.js scenario.json\n');
    process.exit(2);
  }
  const sc = loadScenario(path);
  const gen = Generations.get(9);

  const expandDamage = (raw) => {
    if (Array.isArray(raw)) {
      return (Array.isArray(raw[0]) ? raw.flat() : raw.slice())
        .map((x) => Math.floor(x))
        .sort((a, b) => a - b);
    }
    if (typeof raw === 'number') return [Math.floor(raw)];
    process.stderr.write('warn: unexpected damage shape: ' + JSON.stringify(raw) + '\n');
    return [0];
  };

  // Non-crit roll set.
  const atk1 = buildPokemon(gen, sc.attacker);
  const def1 = buildPokemon(gen, sc.defender);
  const mv1 = new Move(gen, sc.move, { isCrit: false });
  const r1 = calculate(gen, atk1, def1, mv1, buildField(sc.field));
  const dmg = expandDamage(r1.damage);

  // Crit roll set (engine produces these ~4% of trials at base crit
  // stage 0). Combine into one expected union so a basic Adamant-CC
  // observer can't false-fail on a crit roll.
  const atk2 = buildPokemon(gen, sc.attacker);
  const def2 = buildPokemon(gen, sc.defender);
  const mv2 = new Move(gen, sc.move, { isCrit: true });
  const r2 = calculate(gen, atk2, def2, mv2, buildField(sc.field));
  const dmgCrit = expandDamage(r2.damage);

  const union = Array.from(new Set([...dmg, ...dmgCrit])).sort((a, b) => a - b);

  const out = {
    name: sc.name,
    move: sc.move,
    damage: dmg,
    damage_crit: dmgCrit,
    damage_union: union,
    range: [dmg[0], dmg[dmg.length - 1]],
    range_crit: [dmgCrit[0], dmgCrit[dmgCrit.length - 1]],
    desc: (() => { try { return r1.fullDesc(); } catch (e) { return '(fullDesc unavailable: ' + e.message + ')'; } })(),
  };
  process.stdout.write(JSON.stringify(out, null, 2));
}

main();
